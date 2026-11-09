//! The HTTP probe (SPEC §8.9): a request to a loopback server on a fixed cadence, sent from strobe's own
//! threads on absolute deadlines. Slot k is due at start + k × cadence whatever the requests before it
//! are doing (open loop), so a server that stalls for 750 ms shows as a cohort of slow or timed-out
//! probes (750, 650, …, 50 ms), never as one slow sample and a gap. A slot strobe itself could not send
//! on time is marked missed, never sent late and never billed to the server. The authorization it sends
//! lives in memory only (`Secret`).

use crate::host;
use crate::model::{Error, Refusal, Result, ValidatorId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// The largest body `expect` reads (`bodyContains`, `jsonPath`); a longer one fails them.
pub const MAX_BODY: usize = 1 << 20;
/// The probe's own CPU (% of one core over its span) above which its readings are flagged, and above
/// which they are refused `OverheadBudget` (SPEC §19).
pub const CPU_BUDGET_PCT: f64 = 1.0;
pub const CPU_REFUSE_PCT: f64 = 2.0;
/// Settling time between the end of setup and the first idle probe.
pub const SETTLE: Duration = Duration::from_millis(100);
/// Calibration only (`scripts/dev/http-probe.bench.ts`): `<at_ms>,<for_ms>` stops the probe's scheduler
/// once, `at_ms` after its start, for `for_ms`: the slots due meanwhile must read as missed sends.
pub const PAUSE_VAR: &str = "STROBE_PROBE_PAUSE";

const KEYS: &[&str] = &["url", "method", "cadenceMs", "timeoutMs", "idleMs", "connection", "maxInFlight", "headers", "authorization", "expect"];
/// Headers that carry credentials: given only as `authorization`, never in `headers`.
const SECRET_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization", "x-api-key"];
/// Headers strobe writes itself.
const OWN_HEADERS: &[&str] = &["host", "connection", "content-length", "transfer-encoding", "user-agent"];

/// A value that grants access (the probe's `authorization`), in memory only: no `Serialize`, `Display`
/// or `Clone`, a `Debug` of `[REDACTED]`, and its bytes zeroed when it drops, so no result, recording,
/// log, report or panic message can carry it.
pub struct Secret(Vec<u8>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Zeroes `b` with writes the compiler keeps.
pub fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        // SAFETY: `x` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(x, 0) };
    }
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
}

/// Zeroes a string's bytes (a protocol line that held a `Secret`) before it is dropped.
pub fn wipe_string(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8.
    wipe(unsafe { s.as_mut_vec() });
}

/// Zeroes every string of a JSON value (options that held a `Secret`) before it is dropped.
pub fn wipe_value(v: &mut Value) {
    match v {
        Value::String(s) => wipe_string(s),
        Value::Array(a) => a.iter_mut().for_each(wipe_value),
        Value::Object(o) => o.values_mut().for_each(wipe_value),
        _ => {}
    }
}

/// What a probe sends that strobe never writes (`Config::secrets`), kept until the block ends. The
/// target gets them, so a block whose probe carries any takes no dump of the target's memory unless
/// asked (`--probes memory`), and the experiment's text logs (what the target and the hooks printed)
/// have each occurrence made `*`s (`scrub_logs`). Recordings are never rewritten.
#[derive(Debug, Default)]
pub struct Secrets(Vec<Secret>);

impl Secrets {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Makes every occurrence of `secrets` in the plain-text logs `logs` as many `*`s, in place (the same
/// length, so a process that still appends to a log keeps its file). Returns how many held one, and the
/// logs it could not write (left as they were and said, never removed).
pub fn scrub_logs(logs: &[std::path::PathBuf], secrets: &Secrets) -> (usize, Vec<String>) {
    use std::io::{Seek, SeekFrom};
    let (mut n, mut failed) = (0, Vec::new());
    if secrets.0.is_empty() {
        return (n, failed);
    }
    let needles: Vec<&[u8]> = secrets.0.iter().map(|s| s.0.as_slice()).collect();
    for path in logs.iter().filter(|p| p.is_file()) {
        let Ok(f) = std::fs::File::open(path) else { continue };
        let Ok(at) = find(f, &needles) else { continue };
        if at.is_empty() {
            continue;
        }
        n += 1;
        let written = (|| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
            for (off, len) in at {
                f.seek(SeekFrom::Start(off))?;
                f.write_all(&vec![b'*'; len])?;
            }
            f.sync_all()
        })();
        if let Err(e) = written {
            failed.push(format!("{}: {e}", path.display()));
        }
    }
    (n, failed)
}

/// Where `needles` occur in `r` (offset, length), across its chunks' edges too.
fn find(mut r: impl Read, needles: &[&[u8]]) -> std::io::Result<Vec<(u64, usize)>> {
    let keep = needles.iter().map(|n| n.len()).max().unwrap_or(1).saturating_sub(1);
    let (mut buf, mut chunk) = (Vec::with_capacity(1 << 20), vec![0u8; 1 << 20]);
    // The offset of `buf[0]` in the stream.
    let mut base = 0u64;
    let mut out: Vec<(u64, usize)> = Vec::new();
    loop {
        let n = match r.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        buf.extend_from_slice(&chunk[..n]);
        for nd in needles {
            for i in redact(&mut buf, nd) {
                let at = (base + i as u64, nd.len());
                if !out.contains(&at) {
                    out.push(at);
                }
            }
        }
        // The last bytes stay for the next chunk: an occurrence may run across the edge.
        let drop = if n == 0 { buf.len() } else { buf.len().saturating_sub(keep) };
        buf.drain(..drop);
        base += drop as u64;
        if n == 0 {
            return Ok(out);
        }
    }
}

/// Every occurrence of `needle` in `hay` made `*`s (so a later needle never matches it twice); where
/// each began.
fn redact(hay: &mut [u8], needle: &[u8]) -> Vec<usize> {
    let k = needle.len();
    let mut at = Vec::new();
    if k == 0 || hay.len() < k {
        return at;
    }
    let mut i = 0;
    while i + k <= hay.len() {
        if hay[i] == needle[0] && hay[i..i + k] == *needle {
            hay[i..i + k].fill(b'*');
            at.push(i);
            i += k;
        } else {
            i += 1;
        }
    }
    at
}

/// A header whose value is a `Secret`: its name may be said, its value never.
#[derive(Debug)]
pub struct SecretHeader {
    pub name: String,
    value: Secret,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Head,
    Post,
}

impl Method {
    fn name(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
        }
    }
}

/// `new`: a fresh connection per probe (what a client that connects for each request sees, and the
/// default); `keep-alive`: an idle connection of the probe's when one is free, else a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Connection {
    New,
    KeepAlive,
}

/// What a response must be to count (`expect`): its status in a range (200–299 unless given), and
/// optionally a string in its body or a JSON value at a path of it, read from a body of at most 1 MiB
/// that is never kept.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Expect {
    pub status: [u16; 2],
    #[serde(default, rename = "bodyContains", skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    #[serde(default, rename = "jsonPath", skip_serializing_if = "Option::is_none")]
    pub json_path: Option<JsonPath>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonPath {
    pub path: String,
    pub equals: Value,
}

/// A probe as `ctx.httpProbe` set it up. The request's path and query, its extra headers' values and
/// its authorization are sent, never written: `described` is what may be.
#[derive(Debug)]
pub struct Config {
    pub method: Method,
    /// The authority as the URL gave it (the `Host` header) and its loopback addresses.
    authority: String,
    addrs: Vec<SocketAddr>,
    /// The request target as sent (a query may hold a key).
    target: String,
    /// The URL with its query's values left out.
    pub url: String,
    pub cadence_ms: f64,
    pub timeout_ms: f64,
    pub idle_ms: f64,
    pub connection: Connection,
    pub max_in_flight: usize,
    headers: Vec<(String, String)>,
    authorization: Option<SecretHeader>,
    pub expect: Expect,
}

/// A probe's setup as recordings keep it: no query value, header value or authorization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Described {
    pub method: Method,
    pub url: String,
    pub cadence_ms: f64,
    pub timeout_ms: f64,
    pub idle_ms: f64,
    pub connection: Connection,
    pub max_in_flight: usize,
    /// Extra headers' names.
    pub headers: Vec<String>,
    pub has_authorization: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_header: Option<String>,
    pub predicate: Expect,
}

fn usage(why: impl std::fmt::Display) -> Error {
    Error::usage(format!("ctx.httpProbe: {why}"))
}

/// A header name: an HTTP token.
fn token(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

impl Config {
    /// The options `ctx.httpProbe` gave. Any mistake is a usage error that never repeats a header's or
    /// the authorization's value.
    pub fn parse(o: &Value) -> Result<Config> {
        let o = o.as_object().ok_or_else(|| usage("takes one options object"))?;
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(usage(format!("unknown option {k:?} (it takes {})", KEYS.join(", "))));
        }
        let given = |k: &str| o.get(k).filter(|v| !v.is_null());
        let url = given("url").and_then(Value::as_str).ok_or_else(|| usage("url: a loopback address (http://127.0.0.1:<port>/…, http://[::1]:<port>/…, http://localhost:<port>/…) is required"))?;
        let (authority, addrs, target, shown) = parse_url(url)?;
        let method = match given("method").map(|m| m.as_str().map(str::to_ascii_uppercase)) {
            None => Method::Get,
            Some(Some(m)) if m == "GET" => Method::Get,
            Some(Some(m)) if m == "HEAD" => Method::Head,
            Some(Some(m)) if m == "POST" => Method::Post,
            Some(_) => return Err(usage("method: GET, HEAD or POST")),
        };
        let num = |k: &str, default: f64| -> Result<f64> {
            match given(k) {
                None => Ok(default),
                Some(v) => v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| usage(format!("{k}: a number of milliseconds"))),
            }
        };
        let cadence_ms = num("cadenceMs", 100.0)?;
        if cadence_ms < 10.0 {
            return Err(usage(format!("cadenceMs {cadence_ms}: at least 10 (a probe every 10 ms is 100 requests a second)")));
        }
        let timeout_ms = num("timeoutMs", 1000.0)?;
        if !(1.0..=60_000.0).contains(&timeout_ms) {
            return Err(usage(format!("timeoutMs {timeout_ms}: from 1 to 60000")));
        }
        let idle_ms = num("idleMs", 500.0)?;
        if !(0.0..=60_000.0).contains(&idle_ms) {
            return Err(usage(format!("idleMs {idle_ms}: from 0 (no idle control) to 60000")));
        }
        let connection = match given("connection").map(Value::as_str) {
            None | Some(Some("new")) => Connection::New,
            Some(Some("keep-alive")) => Connection::KeepAlive,
            Some(_) => return Err(usage("connection: \"new\" or \"keep-alive\"")),
        };
        let max = num("maxInFlight", 64.0)?;
        if max.fract() != 0.0 || !(1.0..=1024.0).contains(&max) {
            return Err(usage(format!("maxInFlight {max}: a whole number from 1 to 1024")));
        }
        let max_in_flight = max as usize;
        // Open loop: a probe never waits for the ones before it, so as many as one timeout holds are open.
        let need = (timeout_ms / cadence_ms).ceil() as usize + 1;
        if need > max_in_flight {
            return Err(usage(format!(
                "maxInFlight {max_in_flight}: a probe every {cadence_ms} ms that may wait {timeout_ms} ms can have {need} open at once; raise maxInFlight to {need} or lower timeoutMs (the probe never holds a send back)"
            )));
        }
        let mut headers = Vec::new();
        if let Some(h) = given("headers") {
            let h = h.as_object().ok_or_else(|| usage("headers: an object of header names to string values"))?;
            for (k, v) in h {
                let lower = k.to_ascii_lowercase();
                if !token(k) {
                    return Err(usage(format!("headers: {k:?} is no header name")));
                }
                if SECRET_HEADERS.contains(&lower.as_str()) {
                    return Err(usage(format!("headers: {k} carries a credential: give it as authorization ({{ header: \"{k}\", value }}), which strobe never writes anywhere")));
                }
                if OWN_HEADERS.contains(&lower.as_str()) {
                    return Err(usage(format!("headers: strobe writes {k} itself")));
                }
                let v = v.as_str().filter(|v| !v.contains(['\r', '\n'])).ok_or_else(|| usage(format!("headers: {k}'s value must be one line of text")))?;
                headers.push((k.clone(), v.to_string()));
            }
        }
        let authorization = match given("authorization") {
            None => None,
            Some(Value::String(s)) => Some(("Authorization".to_string(), s.as_bytes().to_vec())),
            Some(Value::Object(a)) => {
                if let Some(k) = a.keys().find(|k| *k != "header" && *k != "value") {
                    return Err(usage(format!("authorization: unknown key {k:?} (it takes header and value)")));
                }
                let name = a.get("header").and_then(Value::as_str).filter(|n| token(n)).ok_or_else(|| usage("authorization.header: a header name"))?;
                if OWN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err(usage(format!("authorization.header: strobe writes {name} itself")));
                }
                let value = a.get("value").and_then(Value::as_str).ok_or_else(|| usage("authorization.value: a string"))?;
                Some((name.to_string(), value.as_bytes().to_vec()))
            }
            Some(_) => return Err(usage("authorization: a string (the Authorization header's value) or { header, value }")),
        };
        let authorization = match authorization {
            Some((name, mut value)) if value.is_empty() || value.iter().any(|b| *b == b'\r' || *b == b'\n') => {
                wipe(&mut value);
                return Err(usage(format!("authorization: {name}'s value must be one nonempty line")));
            }
            Some((name, value)) => Some(SecretHeader { name, value: Secret(value) }),
            None => None,
        };
        let expect = parse_expect(given("expect"))?;
        if method == Method::Head && (expect.body_contains.is_some() || expect.json_path.is_some()) {
            return Err(usage("expect: a HEAD response has no body to read"));
        }
        Ok(Config { method, authority, addrs, target, url: shown, cadence_ms, timeout_ms, idle_ms, connection, max_in_flight, headers, authorization, expect })
    }

    /// What it sends that strobe never writes: the authorization's value (and its credential alone, past a
    /// scheme such as `Bearer `), every extra header's value, and the URL's query values (a segment
    /// without `=` whole) of 8 characters or more, as sent and percent-decoded. Longest first, so a
    /// value is made `*`s whole before a part of it would be.
    pub fn secrets(&self) -> Secrets {
        let mut v: Vec<Vec<u8>> = Vec::new();
        if let Some(a) = &self.authorization {
            v.push(a.value.0.clone());
            if let Some(i) = a.value.0.iter().position(|b| *b == b' ')
                && a.value.0.len() - i > 1
            {
                v.push(a.value.0[i + 1..].to_vec());
            }
        }
        v.extend(self.headers.iter().filter(|(_, x)| !x.is_empty()).map(|(_, x)| x.as_bytes().to_vec()));
        if let Some((_, q)) = self.target.split_once('?') {
            for part in q.split('&') {
                let x = part.split_once('=').map_or(part, |(_, x)| x);
                if x.len() >= 8 {
                    v.push(x.as_bytes().to_vec());
                    v.push(percent_decoded(x));
                }
            }
        }
        v.sort_by_key(|x| std::cmp::Reverse(x.len()));
        v.dedup();
        Secrets(v.into_iter().map(Secret).collect())
    }

    pub fn describe(&self) -> Described {
        Described {
            method: self.method,
            url: self.url.clone(),
            cadence_ms: self.cadence_ms,
            timeout_ms: self.timeout_ms,
            idle_ms: self.idle_ms,
            connection: self.connection,
            max_in_flight: self.max_in_flight,
            headers: self.headers.iter().map(|h| h.0.clone()).collect(),
            has_authorization: self.authorization.is_some(),
            authorization_header: self.authorization.as_ref().map(|a| a.name.clone()),
            predicate: self.expect.clone(),
        }
    }

    /// The request's bytes; they hold the authorization, so the caller wipes them once written.
    fn request(&self, keep: bool) -> Vec<u8> {
        let mut r = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: strobe-http-probe\r\nAccept: */*\r\nConnection: {}\r\n",
            self.method.name(),
            self.target,
            self.authority,
            if keep { "keep-alive" } else { "close" }
        )
        .into_bytes();
        if self.method == Method::Post {
            r.extend_from_slice(b"Content-Length: 0\r\n");
        }
        for (k, v) in &self.headers {
            r.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
        if let Some(a) = &self.authorization {
            r.extend_from_slice(a.name.as_bytes());
            r.extend_from_slice(b": ");
            r.extend_from_slice(&a.value.0);
            r.extend_from_slice(b"\r\n");
        }
        r.extend_from_slice(b"\r\n");
        r
    }

    /// Why a response does not count, if it does not.
    fn judge(&self, a: &Answer) -> Option<String> {
        let [lo, hi] = self.expect.status;
        if a.status < lo || a.status > hi {
            return Some(if lo == hi { format!("HTTP {}, not {lo}", a.status) } else { format!("HTTP {}, not {lo}–{hi}", a.status) });
        }
        let reads = self.expect.body_contains.is_some() || self.expect.json_path.is_some();
        if reads && a.over {
            return Some("the body is over 1 MiB, more than expect reads".into());
        }
        if let Some(text) = &self.expect.body_contains
            && !a.body.windows(text.len().max(1)).any(|w| w == text.as_bytes())
        {
            return Some("the body lacks expect.bodyContains".into());
        }
        if let Some(p) = &self.expect.json_path {
            let Ok(doc) = serde_json::from_slice::<Value>(&a.body) else { return Some("the body is not JSON (expect.jsonPath)".into()) };
            return match json_at(&doc, &p.path) {
                None => Some(format!("the body has no {} (expect.jsonPath)", p.path)),
                Some(v) if *v != p.equals => Some(format!("{} differs from expect.jsonPath.equals", p.path)),
                Some(_) => None,
            };
        }
        None
    }
}

/// `%XX` escapes decoded (`+` stays: a form's space is the server's reading, not the URL's).
fn percent_decoded(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `http://<loopback>[:port]/path?query`: its authority, addresses, request target and the URL with its
/// query's values left out. TLS, credentials in the URL and any host but this computer's loopback are
/// usage errors.
fn parse_url(url: &str) -> Result<(String, Vec<SocketAddr>, String, String)> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Err(usage("url: https is not probed: the probe speaks plain HTTP to a loopback server (no TLS)"));
    }
    let rest = lower.starts_with("http://").then(|| &url[7..]).ok_or_else(|| usage("url: an http:// address on this computer's loopback"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return Err(usage("url: credentials in the URL (user:pass@) are never sent: give them as authorization, which strobe never writes anywhere"));
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (h, after) = v6.split_once(']').ok_or_else(|| usage("url: an IPv6 host needs its closing ]"))?;
            (h.to_string(), after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.to_string(), None),
        },
    };
    let port: u16 = match port {
        None => 80,
        Some(p) => p.parse().ok().filter(|p| *p > 0).ok_or_else(|| usage(format!("url: port {p:?} is no port")))?,
    };
    let loopback = |a: &SocketAddr| a.ip().is_loopback();
    let addrs: Vec<SocketAddr> = if host.eq_ignore_ascii_case("localhost") {
        (host.as_str(), port).to_socket_addrs().map(|a| a.filter(loopback).collect()).unwrap_or_default()
    } else {
        match host.parse::<IpAddr>() {
            Ok(ip) if ip.is_loopback() => vec![SocketAddr::new(ip, port)],
            _ => return Err(usage(format!("url: {host} is not this computer's loopback (127.0.0.1, [::1] or localhost): the probe measures a server on this computer, never another host"))),
        }
    };
    if addrs.is_empty() {
        return Err(usage("url: localhost names no loopback address here"));
    }
    let tail = tail.split('#').next().unwrap_or("");
    let target = if tail.is_empty() || tail.starts_with('?') { format!("/{tail}") } else { tail.to_string() };
    if target.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(usage("url: its path holds spaces or control characters: percent-encode them"));
    }
    let shown_target = match target.split_once('?') {
        Some((path, query)) => {
            // Keys stay, values go; a segment without `=` is all value (`?<token>`): it goes whole.
            let keys: Vec<String> = query.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').map_or("…".to_string(), |(k, _)| format!("{k}=…"))).collect();
            format!("{path}?{}", keys.join("&"))
        }
        None => target.clone(),
    };
    Ok((authority.to_string(), addrs, target, format!("http://{authority}{shown_target}")))
}

fn parse_expect(v: Option<&Value>) -> Result<Expect> {
    let mut e = Expect { status: [200, 299], body_contains: None, json_path: None };
    let Some(v) = v else { return Ok(e) };
    let o = v.as_object().ok_or_else(|| usage("expect: { status, bodyContains, jsonPath }"))?;
    if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "status" | "bodyContains" | "jsonPath")) {
        return Err(usage(format!("expect: unknown key {k:?} (it takes status, bodyContains and jsonPath)")));
    }
    let code = |v: &Value| v.as_u64().filter(|c| (100..=599).contains(c)).map(|c| c as u16);
    match o.get("status") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) if a.len() == 2 => {
            let (lo, hi) = code(&a[0]).zip(code(&a[1])).filter(|(l, h)| l <= h).ok_or_else(|| usage("expect.status: [lo, hi], two HTTP status codes"))?;
            e.status = [lo, hi];
        }
        Some(s) => {
            let c = code(s).ok_or_else(|| usage("expect.status: an HTTP status code or [lo, hi]"))?;
            e.status = [c, c];
        }
    }
    if let Some(b) = o.get("bodyContains").filter(|b| !b.is_null()) {
        e.body_contains = Some(b.as_str().filter(|s| !s.is_empty()).ok_or_else(|| usage("expect.bodyContains: nonempty text"))?.to_string());
    }
    if let Some(p) = o.get("jsonPath").filter(|p| !p.is_null()) {
        let path = p["path"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| usage("expect.jsonPath: { path: \"$.status\", equals: \"ok\" }"))?;
        if p.get("equals").is_none() {
            return Err(usage("expect.jsonPath.equals: the value the path must hold"));
        }
        e.json_path = Some(JsonPath { path: path.to_string(), equals: p["equals"].clone() });
    }
    Ok(e)
}

/// The value at `$.a.b[0].c` (or `a.b.0.c`) of `doc`.
fn json_at<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.strip_prefix('$').unwrap_or(path);
    let mut at = doc;
    for part in path.split(['.', '[', ']']).filter(|p| !p.is_empty()) {
        at = match at {
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            Value::Object(o) => o.get(part)?,
            _ => return None,
        };
    }
    Some(at)
}

/// How a probe ended.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// The response counted (`expect`).
    Ok,
    /// The server answered, not as `expect` asked (a 500, a body without the text).
    Bad { why: String },
    /// No whole response within `timeoutMs` of its dispatch.
    Timeout,
    /// The connection failed (refused, reset, closed mid-response).
    Error { why: String },
    /// strobe could not send it on time: its scheduler or a worker ran late by `lag_ms`. Never sent.
    SkippedDriverLate { lag_ms: f64 },
    /// strobe had no free connection (every probe it may hold open was open). Never sent.
    SkippedBusy,
    /// Still out when the probe stopped.
    #[default]
    Pending,
}

impl Outcome {
    /// Not sent: strobe's doing, not the server's.
    pub fn missed(&self) -> bool {
        matches!(self, Outcome::SkippedDriverLate { .. } | Outcome::SkippedBusy)
    }
}

/// Where in the block a probe was due: `idle` (the control after setup), `measured` (a measured run's
/// timed part), `untimed` (a run's `ctx.untimed` stretch), `warmup` (a run not measured), `outside`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    Idle,
    Measured,
    Untimed,
    Warmup,
    #[default]
    Outside,
}

/// One probe, on the host clock (ns).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sample {
    pub seq: u64,
    pub scheduled_ns: u64,
    /// Before its connection was taken or opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_ns: Option<u64>,
    /// Its request written and flushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_ns: Option<u64>,
    /// Its whole response read (or the connection failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub outcome: Outcome,
    #[serde(default)]
    pub when: When,
    /// The run it was due in (`run:<i>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

impl Sample {
    /// Dispatch to the whole response (ms): what a client sending at that moment waited, the driver's
    /// own lateness before the dispatch left out. Only for responses that counted.
    pub fn latency_ms(&self) -> Option<f64> {
        match (&self.outcome, self.dispatch_ns, self.completed_ns) {
            (Outcome::Ok, Some(a), Some(b)) => Some(b.saturating_sub(a) as f64 / 1e6),
            _ => None,
        }
    }
}

/// A response, its body kept up to `MAX_BODY`.
struct Answer {
    status: u16,
    body: Vec<u8>,
    over: bool,
    /// The connection may carry another request.
    reusable: bool,
}

enum Failed {
    Timeout,
    Error(String),
    /// A kept connection the server had closed: nothing came back on it. The probe sends again on a
    /// new connection, within the same deadline.
    Stale,
}

/// The response read from a connection, by its deadline (host clock).
struct Reader<'a> {
    s: &'a mut TcpStream,
    buf: Vec<u8>,
    pos: usize,
    deadline: u64,
    /// Bytes read from this connection for this request.
    got: usize,
}

impl Reader<'_> {
    /// Reads more; false at the end of the stream.
    fn fill(&mut self) -> std::result::Result<bool, Failed> {
        if self.pos > 64 * 1024 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let left = self.deadline.saturating_sub(host::now_ns());
            if left == 0 {
                return Err(Failed::Timeout);
            }
            let _ = self.s.set_read_timeout(Some(Duration::from_nanos(left.max(1_000_000))));
            match self.s.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.got += n;
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) if self.got == 0 && matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => return Err(Failed::Stale),
                Err(e) => return Err(Failed::Error(e.to_string())),
            }
        }
    }

    fn eof(&self) -> Failed {
        if self.got == 0 { Failed::Stale } else { Failed::Error("the server closed the connection mid-response".into()) }
    }

    /// One line, without its CRLF.
    fn line(&mut self) -> std::result::Result<String, Failed> {
        loop {
            if let Some(i) = self.buf[self.pos..].windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.buf[self.pos..self.pos + i]).to_string();
                self.pos += i + 2;
                return Ok(line);
            }
            if self.buf.len() - self.pos > 64 * 1024 {
                return Err(Failed::Error("a response line over 64 KiB".into()));
            }
            if !self.fill()? {
                return Err(self.eof());
            }
        }
    }

    /// `n` bytes of body into `body`.
    fn take(&mut self, mut n: usize, body: &mut Body) -> std::result::Result<(), Failed> {
        while n > 0 {
            if self.pos == self.buf.len() && !self.fill()? {
                return Err(Failed::Error("the server closed the connection mid-body".into()));
            }
            let k = n.min(self.buf.len() - self.pos);
            body.push(&self.buf[self.pos..self.pos + k]);
            self.pos += k;
            n -= k;
        }
        Ok(())
    }

    fn rest(&mut self, body: &mut Body) -> std::result::Result<(), Failed> {
        loop {
            body.push(&self.buf[self.pos..]);
            self.pos = self.buf.len();
            if !self.fill()? {
                return Ok(());
            }
        }
    }
}

#[derive(Default)]
struct Body {
    data: Vec<u8>,
    total: usize,
}

impl Body {
    fn push(&mut self, b: &[u8]) {
        self.total += b.len();
        let room = MAX_BODY.saturating_sub(self.data.len());
        self.data.extend_from_slice(&b[..b.len().min(room)]);
    }
}

/// Reads one response (skipping interim 1xx ones) by `deadline`.
fn read_response(s: &mut TcpStream, deadline: u64, head: bool) -> std::result::Result<Answer, Failed> {
    let mut r = Reader { s, buf: Vec::new(), pos: 0, deadline, got: 0 };
    loop {
        let status_line = r.line()?;
        let status: u16 =
            status_line.strip_prefix("HTTP/1.").and_then(|l| l.get(2..5)).and_then(|c| c.parse().ok()).ok_or_else(|| Failed::Error(format!("not an HTTP/1.x response ({:.40})", status_line)))?;
        let old = status_line.starts_with("HTTP/1.0");
        let mut fields: Vec<(String, String)> = Vec::new();
        loop {
            let l = r.line()?;
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                fields.push((k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase()));
            }
        }
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        let field = |name: &str| fields.iter().find(|f| f.0 == name).map(|f| f.1.as_str());
        let close = field("connection").is_some_and(|c| c.contains("close")) || (old && !field("connection").is_some_and(|c| c.contains("keep-alive")));
        let mut body = Body::default();
        let mut known = true;
        if head || status == 204 || status == 304 {
        } else if field("transfer-encoding").is_some_and(|t| t.contains("chunked")) {
            loop {
                let size = r.line()?;
                let n = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16).map_err(|_| Failed::Error("a bad chunk size".into()))?;
                if n == 0 {
                    // Trailers, then the empty line.
                    while !r.line()?.is_empty() {}
                    break;
                }
                r.take(n, &mut body)?;
                if !r.line()?.is_empty() {
                    return Err(Failed::Error("a chunk without its CRLF".into()));
                }
            }
        } else if let Some(n) = field("content-length").and_then(|n| n.parse::<usize>().ok()) {
            r.take(n, &mut body)?;
        } else {
            known = false;
            r.rest(&mut body)?;
        }
        return Ok(Answer { status, over: body.total > MAX_BODY, body: body.data, reusable: known && !close && r.pos == r.buf.len() });
    }
}

/// What the probe's threads share.
struct Shared {
    cfg: Config,
    /// When slot 0 was due (host clock), set as the scheduler starts.
    start_ns: AtomicU64,
    cadence_ns: u64,
    /// A slot whose dispatch comes later than this after it was due is missed (`SkippedDriverLate`):
    /// half the cadence, at most 10 ms.
    late_ns: u64,
    timeout_ns: u64,
    stop: AtomicBool,
    /// Workers waiting for a slot.
    idle: AtomicUsize,
    done: Mutex<Vec<Sample>>,
    /// Kept connections (`keep-alive`).
    kept: Mutex<Vec<TcpStream>>,
    /// The probe's threads' CPU so far (ms) and whether one could not read its own.
    cpu: Mutex<(f64, bool)>,
}

impl Shared {
    fn record(&self, s: Sample) {
        self.done.lock().unwrap_or_else(|e| e.into_inner()).push(s);
    }

    fn add_cpu(&self, ms: Option<f64>) {
        let mut c = self.cpu.lock().unwrap_or_else(|e| e.into_inner());
        match ms {
            Some(ms) => c.0 += ms,
            None => c.1 = true,
        }
    }

    /// One slot, on a worker: sent now, or missed when the hand-off made it late.
    fn send(&self, seq: u64, scheduled_ns: u64) -> Sample {
        let dispatch = host::now_ns();
        let mut s = Sample { seq, scheduled_ns, ..Sample::default() };
        let lag = dispatch.saturating_sub(scheduled_ns);
        if lag > self.late_ns {
            s.outcome = Outcome::SkippedDriverLate { lag_ms: lag as f64 / 1e6 };
            return s;
        }
        s.dispatch_ns = Some(dispatch);
        let deadline = dispatch + self.timeout_ns;
        let keep = self.cfg.connection == Connection::KeepAlive;
        let mut req = self.cfg.request(keep);
        let mut reused = if keep { self.kept.lock().unwrap_or_else(|e| e.into_inner()).pop() } else { None };
        let result = loop {
            let fresh = reused.is_none();
            let mut conn = match reused.take() {
                Some(c) => c,
                None => match self.connect(deadline) {
                    Ok(c) => c,
                    Err(f) => break Err(f),
                },
            };
            let left = deadline.saturating_sub(host::now_ns()).max(1_000_000);
            let _ = conn.set_write_timeout(Some(Duration::from_nanos(left)));
            let wrote = conn.write_all(&req).and_then(|_| conn.flush());
            match wrote {
                Ok(()) => s.sent_ns = Some(host::now_ns()),
                Err(_) if !fresh => continue,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break Err(Failed::Timeout),
                Err(e) => break Err(Failed::Error(e.to_string())),
            }
            match read_response(&mut conn, deadline, self.cfg.method == Method::Head) {
                // A kept connection the server had closed: a new one, within the same deadline.
                Err(Failed::Stale) if !fresh => continue,
                Err(Failed::Stale) => break Err(Failed::Error("the server closed the connection without a response".into())),
                Ok(a) => {
                    let done = host::now_ns();
                    if keep && a.reusable && !self.stop.load(Ordering::Relaxed) {
                        let mut k = self.kept.lock().unwrap_or_else(|e| e.into_inner());
                        if k.len() < self.cfg.max_in_flight {
                            k.push(conn);
                        }
                    }
                    break Ok((a, done));
                }
                Err(f) => break Err(f),
            }
        };
        let now = host::now_ns();
        wipe(&mut req);
        match result {
            Ok((a, done)) => {
                s.completed_ns = Some(done);
                s.status = Some(a.status);
                s.outcome = match self.cfg.judge(&a) {
                    None => Outcome::Ok,
                    Some(why) => Outcome::Bad { why },
                };
            }
            Err(Failed::Timeout) => s.outcome = Outcome::Timeout,
            Err(f) => {
                s.completed_ns = Some(now);
                s.outcome = Outcome::Error { why: if let Failed::Error(why) = f { why } else { "the server closed the connection without a response".into() } };
            }
        }
        s
    }

    fn connect(&self, deadline: u64) -> std::result::Result<TcpStream, Failed> {
        let mut last = Failed::Error("no address".into());
        for a in &self.cfg.addrs {
            let left = deadline.saturating_sub(host::now_ns());
            if left == 0 {
                return Err(Failed::Timeout);
            }
            match TcpStream::connect_timeout(a, Duration::from_nanos(left.max(1_000_000))) {
                Ok(c) => {
                    let _ = c.set_nodelay(true);
                    return Ok(c);
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => last = Failed::Timeout,
                Err(e) => last = Failed::Error(format!("connect: {e}")),
            }
        }
        Err(last)
    }

    fn samples(&self) -> Vec<Sample> {
        let mut v = self.done.lock().unwrap_or_else(|e| e.into_inner()).clone();
        v.sort_by_key(|s| s.seq);
        v
    }
}

/// The probe running: a scheduler thread that hands each slot to a free worker when it is due, and the
/// workers that send them, all on strobe's own CPUs. Dropped, it stops (its threads end by themselves).
pub struct Driver {
    shared: Arc<Shared>,
    scheduler: Option<std::thread::JoinHandle<()>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

/// What a stopped probe left.
pub struct Stopped {
    pub described: Described,
    pub start_ns: u64,
    pub end_ns: u64,
    pub samples: Vec<Sample>,
    /// The probe's threads' CPU (ms); None where one could not read its own.
    pub cpu_ms: Option<f64>,
}

impl Stopped {
    /// The probe's CPU as % of one core over its span.
    pub fn cpu_pct(&self) -> Option<f64> {
        let wall = self.end_ns.saturating_sub(self.start_ns) as f64 / 1e6;
        self.cpu_ms.filter(|_| wall > 0.0).map(|c| 100.0 * c / wall)
    }
}

impl Driver {
    /// Starts probing: slot 0 is due once every worker waits for a slot. `cpus`: the CPUs its threads
    /// run on (strobe's own, off the target's).
    pub fn start(cfg: Config, cpus: Option<u64>) -> Result<Driver> {
        let cadence_ns = (cfg.cadence_ms * 1e6) as u64;
        let timeout_ns = (cfg.timeout_ms * 1e6) as u64;
        // As many as one timeout can hold open, and two more; `parse` checked maxInFlight allows them.
        let workers = ((cfg.timeout_ms / cfg.cadence_ms).ceil() as usize + 2).min(cfg.max_in_flight);
        let pause = std::env::var(PAUSE_VAR).ok().and_then(|v| {
            let (a, b) = v.split_once(',')?;
            Some((a.trim().parse::<u64>().ok()? * 1_000_000, b.trim().parse::<u64>().ok()? * 1_000_000))
        });
        let shared = Arc::new(Shared {
            cfg,
            start_ns: AtomicU64::new(0),
            cadence_ns,
            late_ns: (cadence_ns / 2).min(10_000_000),
            timeout_ns,
            stop: AtomicBool::new(false),
            idle: AtomicUsize::new(0),
            done: Mutex::new(Vec::new()),
            kept: Mutex::new(Vec::new()),
            cpu: Mutex::new((0.0, false)),
        });
        let (tx, rx) = mpsc::channel::<(u64, u64)>();
        let rx = Arc::new(Mutex::new(rx));
        let mut d = Driver { shared: shared.clone(), scheduler: None, workers: Vec::new() };
        for i in 0..workers {
            let (sh, rx) = (shared.clone(), rx.clone());
            let h = std::thread::Builder::new()
                .name(format!("strobe-http-probe-{i}"))
                .stack_size(256 * 1024)
                .spawn(move || {
                    if let Some(m) = cpus {
                        host::set_thread_affinity(m);
                    }
                    let cpu = crate::probe::video::ThreadCpu::start();
                    loop {
                        sh.idle.fetch_add(1, Ordering::SeqCst);
                        let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        sh.idle.fetch_sub(1, Ordering::SeqCst);
                        let Ok((seq, due)) = job else { break };
                        let s = sh.send(seq, due);
                        sh.record(s);
                    }
                    sh.add_cpu(cpu.ms());
                })
                .map_err(|e| Error::msg(format!("start the HTTP probe: {e}")))?;
            d.workers.push(h);
        }
        let ready = host::now_ns() + 1_000_000_000;
        while shared.idle.load(Ordering::SeqCst) < workers && host::now_ns() < ready {
            std::thread::sleep(Duration::from_millis(1));
        }
        let start = host::now_ns();
        shared.start_ns.store(start, Ordering::SeqCst);
        let sh = shared.clone();
        let h = std::thread::Builder::new()
            .name("strobe-http-probe".into())
            .spawn(move || {
                if let Some(m) = cpus {
                    host::set_thread_affinity(m);
                }
                let cpu = crate::probe::video::ThreadCpu::start();
                let mut pause = pause;
                let mut k = 0u64;
                'slots: while !sh.stop.load(Ordering::SeqCst) {
                    let due = start + k * sh.cadence_ns;
                    loop {
                        let now = host::now_ns();
                        if now >= due {
                            break;
                        }
                        if sh.stop.load(Ordering::SeqCst) {
                            break 'slots;
                        }
                        // One sleep to the slot (each wake costs the probe CPU); a stop waits at most 200 ms.
                        std::thread::sleep(Duration::from_nanos((due - now).min(200_000_000)));
                    }
                    // Calibration: the scheduler held up, as a busy machine would hold it.
                    if let Some((_, len)) = pause.filter(|(at, _)| due >= start + at) {
                        pause = None;
                        std::thread::sleep(Duration::from_nanos(len));
                    }
                    let lag = host::now_ns().saturating_sub(due);
                    if lag > sh.late_ns {
                        // Never sent late, never a burst to catch up: each slot it missed is said.
                        sh.record(Sample { seq: k, scheduled_ns: due, outcome: Outcome::SkippedDriverLate { lag_ms: lag as f64 / 1e6 }, ..Sample::default() });
                    } else if sh.idle.load(Ordering::SeqCst) == 0 {
                        sh.record(Sample { seq: k, scheduled_ns: due, outcome: Outcome::SkippedBusy, ..Sample::default() });
                    } else if tx.send((k, due)).is_err() {
                        break;
                    }
                    k += 1;
                }
                drop(tx);
                sh.add_cpu(cpu.ms());
            })
            .map_err(|e| Error::msg(format!("start the HTTP probe: {e}")))?;
        d.scheduler = Some(h);
        Ok(d)
    }

    fn start_ns(&self) -> u64 {
        self.shared.start_ns.load(Ordering::SeqCst)
    }

    /// Whether every slot before `n` has ended (answered, failed, timed out or missed).
    fn ended(&self, n: u64) -> bool {
        self.shared.done.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|s| s.seq < n).count() as u64 >= n
    }

    /// The idle control (SPEC §8.9): probes for `idleMs` from the start, with nothing else running, and
    /// waits for each answer. A server that fails a probe here fails it in every run: the block is
    /// refused `RunIncomplete` before its warmup. Returns the idle window (none with `idleMs: 0`).
    pub fn idle(&self) -> Result<Option<(u64, u64)>> {
        let cfg = &self.shared.cfg;
        let idle_ns = (cfg.idle_ms * 1e6) as u64;
        if idle_ns == 0 {
            return Ok(None);
        }
        let (start, cadence) = (self.start_ns(), self.shared.cadence_ns);
        let end = start + idle_ns;
        while host::now_ns() < end {
            std::thread::sleep(Duration::from_nanos((end - host::now_ns()).min(20_000_000)));
        }
        let n = idle_ns.div_ceil(cadence);
        let wait = end + self.shared.timeout_ns + 1_000_000_000;
        while !self.ended(n) && host::now_ns() < wait {
            std::thread::sleep(Duration::from_millis(5));
        }
        let done: Vec<Sample> = self.shared.samples().into_iter().filter(|s| s.seq < n).collect();
        let failed: Vec<&Sample> = done.iter().filter(|s| !s.outcome.missed() && s.outcome != Outcome::Ok).collect();
        let fix = format!("check the url, its authorization and expect against the server with nothing running ({} {})", cfg.method.name(), cfg.url);
        if !failed.is_empty() {
            let why = Error::refused(
                "RunIncomplete",
                format!("the HTTP probe's idle control: the server failed {} of {} probes after setup, before any run ({})", failed.len(), done.len(), tally(failed.iter().copied())),
                fix,
            );
            return Err(why);
        }
        if !done.iter().any(|s| s.outcome == Outcome::Ok) {
            return Err(Error::refused(
                "RunIncomplete",
                format!("the HTTP probe's idle control: no probe answered in {} ms after setup (strobe sent {} of {})", cfg.idle_ms, done.iter().filter(|s| !s.outcome.missed()).count(), n),
                "raise idleMs to a few cadences, and keep other work off strobe's CPUs",
            ));
        }
        Ok(Some((start, end)))
    }

    /// Stops sending, waits (up to a timeout) for the probes out, and hands over what it read.
    pub fn stop(mut self) -> Stopped {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.scheduler.take() {
            let _ = crate::probe::join_within(h, Duration::from_millis(500));
        }
        let wait = Duration::from_nanos(self.shared.timeout_ns) + Duration::from_secs(2);
        for h in self.workers.drain(..) {
            let _ = crate::probe::join_within(h, wait);
        }
        self.shared.kept.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let end_ns = host::now_ns();
        let cpu = *self.shared.cpu.lock().unwrap_or_else(|e| e.into_inner());
        Stopped { described: self.shared.cfg.describe(), start_ns: self.start_ns(), end_ns, samples: self.shared.samples(), cpu_ms: (!cpu.1).then_some(cpu.0) }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

/// Probes that did not count, by what happened: `HTTP 500 ×3, timeout ×1`.
fn tally<'a>(samples: impl Iterator<Item = &'a Sample>) -> String {
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for s in samples {
        let k = match &s.outcome {
            Outcome::Bad { why } => why.clone(),
            Outcome::Error { why } => why.clone(),
            Outcome::Timeout => "timeout".into(),
            Outcome::Pending => "still out at the end".into(),
            Outcome::SkippedDriverLate { .. } => "missed (strobe late)".into(),
            Outcome::SkippedBusy => "missed (no free connection)".into(),
            Outcome::Ok => "ok".into(),
        };
        *by.entry(k).or_default() += 1;
    }
    by.into_iter().map(|(k, n)| if n == 1 { k } else { format!("{k} ×{n}") }).collect::<Vec<_>>().join(", ")
}

/// A run as the probe sees it: its span and `ctx.untimed` stretches on the host clock.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Span {
    pub key: String,
    pub t0_ns: u64,
    pub t1_ns: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untimed: Vec<(u64, u64)>,
    /// Its `ctx.phase` spans (name, start, end), for `query <rec> http --phase`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<(String, u64, u64)>,
    pub measured: bool,
}

/// Sets each sample's `when` and `run`: where in the block it was due.
pub fn classify(samples: &mut [Sample], idle: Option<(u64, u64)>, runs: &[Span]) {
    for s in samples {
        let t = s.scheduled_ns;
        let run = runs.iter().find(|r| t >= r.t0_ns && t < r.t1_ns);
        s.run = run.map(|r| r.key.clone());
        s.when = match run {
            _ if idle.is_some_and(|(a, b)| t >= a && t < b) => When::Idle,
            Some(r) if r.untimed.iter().any(|(a, b)| t >= *a && t < *b) => When::Untimed,
            Some(r) if r.measured => When::Measured,
            Some(_) => When::Warmup,
            None => When::Outside,
        };
    }
}

/// The recording's file (owner-only): the probe's setup, its window and every probe.
pub const FILE: &str = "http_probe.json.gz";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Data {
    pub config: Described,
    pub start_ns: u64,
    pub end_ns: u64,
    #[serde(default)]
    pub cpu_ms: Option<f64>,
    #[serde(default)]
    pub idle: Option<(u64, u64)>,
    #[serde(default)]
    pub runs: Vec<Span>,
    pub samples: Vec<Sample>,
}

pub fn write(dir: &std::path::Path, data: &Data) -> Result<()> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&serde_json::to_vec(data)?)?;
    let path = dir.join(FILE);
    // Owner-only from its creation, as everything of the target's (SPEC rule 9).
    let mut f = crate::store::create(&path)?;
    f.write_all(&gz.finish()?)?;
    f.commit().map_err(|e| Error::msg(format!("write {}: {e}", path.display())))
}

pub fn read(dir: &std::path::Path) -> Option<Data> {
    let mut text = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(dir.join(FILE)).ok()?).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// The `http.*` metrics declared in `names`.
pub fn metric(name: &str) -> bool {
    name.starts_with("http.")
}

/// The latency metrics and their quantiles.
pub const LATENCIES: [(&str, f64); 3] = [("http.latency.p50", 0.5), ("http.latency.p95", 0.95), ("http.latency.max", 1.0)];

/// A quantile of latencies where each timeout ranks above every answer (right-censored at the
/// timeout): its value, or None when it falls on a timeout (or between one and an answer), where the
/// true latency is past the timeout and unknown. Never the timeout itself, never the answers alone.
pub fn censored_quantile(answers: &[f64], timeouts: usize, p: f64) -> Option<f64> {
    let n = answers.len() + timeouts;
    if n == 0 {
        return Some(f64::NAN);
    }
    let mut v: Vec<f64> = answers.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let i = (n - 1) as f64 * p;
    let (lo, hi) = (i.floor() as usize, i.ceil() as usize);
    if hi >= v.len() {
        return None;
    }
    Some(v[lo] + (v[hi] - v[lo]) * (i - lo as f64))
}

/// What the probe's own CPU says of a block's latencies: over `CPU_REFUSE_PCT` of a core it competed with
/// what it timed (a timed block refuses them, a `--check` warns, an attribution block says nothing).
pub fn cpu_refusal(pct: f64) -> Option<Refusal> {
    (pct > CPU_REFUSE_PCT).then(|| {
        Refusal::new(ValidatorId::OverheadBudget, format!("the HTTP probe used {pct:.1}% of a core (budget {CPU_BUDGET_PCT}%, refused over {CPU_REFUSE_PCT}%)"))
            .fix("probe less often (cadenceMs), or use connection: \"keep-alive\"")
    })
}

/// The block's `http.*` values and refusals from what the probe read in its measured runs (`measured`
/// of them), and the summary result.json and meta.json keep.
pub struct Reading {
    pub values: BTreeMap<String, f64>,
    pub refused: BTreeMap<String, Refusal>,
    pub summary: Value,
}

pub fn reading(d: &Data, measured: usize) -> Reading {
    let mut values = BTreeMap::new();
    let mut refused: BTreeMap<String, Refusal> = BTreeMap::new();
    let counted: Vec<&Sample> = d.samples.iter().filter(|s| s.when == When::Measured).collect();
    let missed: Vec<&Sample> = counted.iter().copied().filter(|s| s.outcome.missed()).collect();
    let answers: Vec<f64> = counted.iter().filter_map(|s| s.latency_ms()).collect();
    let timeouts = counted.iter().filter(|s| s.outcome == Outcome::Timeout).count();
    let failed: Vec<&Sample> = counted.iter().copied().filter(|s| matches!(s.outcome, Outcome::Bad { .. } | Outcome::Error { .. } | Outcome::Pending)).collect();
    let runs = measured.max(1) as f64;
    values.insert("http.probes".to_string(), counted.len() as f64 / runs);
    values.insert("http.timeouts".to_string(), timeouts as f64 / runs);
    values.insert("http.errors".to_string(), failed.len() as f64 / runs);
    values.insert("http.missed_sends".to_string(), missed.len() as f64 / runs);
    values.insert("http.latency.n".to_string(), (answers.len() + timeouts) as f64);
    let idle: Vec<f64> = d.samples.iter().filter(|s| s.when == When::Idle).filter_map(Sample::latency_ms).collect();
    if !idle.is_empty() {
        values.insert("http.idle.p50".to_string(), crate::metrics::median(&idle));
    }
    let mut censored = Vec::new();
    for (name, p) in LATENCIES {
        match censored_quantile(&answers, timeouts, p) {
            Some(v) if v.is_finite() => {
                values.insert(name.to_string(), v);
            }
            Some(_) => {
                refused.insert(
                    name.to_string(),
                    Refusal::new(ValidatorId::TailTooShort, format!("{name}: no probe was due in a measured run's timed part (a probe every {} ms)", d.config.cadence_ms))
                        .fix("make the runs longer than the cadence, or probe more often (cadenceMs)"),
                );
            }
            None => {
                censored.push(name);
                refused.insert(
                    name.to_string(),
                    Refusal::new(
                        ValidatorId::RunIncomplete,
                        format!(
                            "{name}: right-censored: {timeouts} of {} probes in the measured runs timed out at {} ms, so this quantile lies past the timeout, where its value is unknown",
                            answers.len() + timeouts,
                            d.config.timeout_ms
                        ),
                    )
                    .fix("raise timeoutMs above the longest stall to read it; http.timeouts counts them"),
                );
            }
        }
    }
    let latency = |r: &mut BTreeMap<String, Refusal>, why: Refusal| {
        for (name, _) in LATENCIES {
            r.insert(name.to_string(), Refusal { summary: format!("{name}: {}", why.summary), ..why.clone() });
        }
    };
    if !failed.is_empty() {
        latency(
            &mut refused,
            Refusal::new(
                ValidatorId::RunIncomplete,
                format!(
                    "{} of {} probes in the measured runs got no response that counts ({}): a fast error is no fast answer",
                    failed.len(),
                    counted.len() - missed.len(),
                    tally(failed.iter().copied())
                ),
            )
            .fix("fix what the server answered, or what expect asks for; http.errors counts them"),
        );
    }
    if missed.len() > 1.max(counted.len() / 20) {
        let lag = missed.iter().filter_map(|s| if let Outcome::SkippedDriverLate { lag_ms } = s.outcome { Some(lag_ms) } else { None }).fold(0.0, f64::max);
        latency(
            &mut refused,
            Refusal::new(
                ValidatorId::OverheadBudget,
                format!(
                    "strobe missed {} of {} probes due in the measured runs (its scheduler ran up to {lag:.0} ms late): a stall then would go unseen; the server is not to blame",
                    missed.len(),
                    counted.len()
                ),
            )
            .fix("keep other work off strobe's CPUs (--pin), or probe less often (cadenceMs)"),
        );
    }
    // The probe's own CPU refuses only a timed block's latencies (`cpu_refusal`, the caller's).
    let cpu_pct = Stopped { described: d.config.clone(), start_ns: d.start_ns, end_ns: d.end_ns, samples: Vec::new(), cpu_ms: d.cpu_ms }.cpu_pct();
    for name in refused.keys() {
        values.remove(name);
    }
    let q = |p: f64| censored_quantile(&answers, timeouts, p).filter(|v| v.is_finite());
    let idle_q = |p: f64| censored_quantile(&idle, 0, p).filter(|v| v.is_finite());
    let mut statuses: BTreeMap<String, usize> = BTreeMap::new();
    for s in &counted {
        if let Some(c) = s.status {
            *statuses.entry(c.to_string()).or_default() += 1;
        }
    }
    let summary = json!({
        "config": d.config,
        "start_ns": d.start_ns,
        "end_ns": d.end_ns,
        "cpu_ms": d.cpu_ms,
        "cpu_pct": cpu_pct,
        "probes": d.samples.len(),
        "idle": d.idle.map(|_| json!({ "n": idle.len(), "p50": idle_q(0.5), "p95": idle_q(0.95), "max": idle_q(1.0) })),
        "measured": {
            "runs": measured,
            "due": counted.len(),
            "ok": answers.len(),
            "timeouts": timeouts,
            "errors": failed.len(),
            "missed": missed.len(),
            "statuses": statuses,
            "p50": q(0.5),
            "p95": q(0.95),
            "max": q(1.0),
            "censored": censored,
        },
    });
    Reading { values, refused, summary }
}
