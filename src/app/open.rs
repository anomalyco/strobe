//! `strobe open`: a recording, or an experiment's block, in the Perfetto UI with everything strobe
//! knows on one timeline (SPEC §11.4). Local only: the UI (the engine's own release), the trace and
//! trace_processor stay on 127.0.0.1, and the UI's telemetry is switched off.

use super::{Output, fmt, progress};
use crate::cli::Args;
use crate::host::{http, paths};
use crate::model::{Error, Result};
use crate::{engine, perfetto, store};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn run(args: &Args) -> Result<Output> {
    let handle = args.pos.first().ok_or_else(|| Error::usage("open <rec_…|exp_…[/head|base|<block>|<case>]>: a recording or an experiment's block in the Perfetto UI"))?;
    if args.has("no-server") && args.has("no-browser") {
        return Err(Error::usage("--no-server opens nothing, so --no-browser says nothing more: give one"));
    }
    let (dir, id) = recording(handle)?;
    let p = prepare(&dir, &id)?;
    // A scripted check of what the trace holds: built and read by trace_processor, which then ends.
    if args.has("no-server") {
        return Ok(Output::new(p.text, p.json));
    }
    let mut text = p.text;
    let mut out = p.json;
    let browse = !args.has("no-browser");
    let ui = ui()?;
    // The daemon serves the UI when it is this build's; otherwise this command does, until Ctrl-C.
    let daemon = super::mcp::running() && get(super::mcp::port(), "/perfetto/version").is_some_and(|v| v == version());
    let (port, own) = if daemon {
        (super::mcp::port(), None)
    } else {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        (port, Some(listener))
    };
    let session = start(&p.trace, &id, port)?;
    let url = format!("http://127.0.0.1:{port}/perfetto/s/{}", session.token);
    text += &format!(
        "{} {url}\n  {} · the Perfetto UI {} and the trace stay on this machine\n",
        if browse { "opened" } else { "open" },
        match session.rpc {
            Some(rpc) => format!("trace_processor on 127.0.0.1:{rpc} (ends after an hour unused)"),
            None => "the UI's own trace_processor (WebAssembly)".to_string(),
        },
        ui
    );
    if let Some(note) = &session.note {
        text += &format!("  ! {note}\n");
    }
    out["url"] = json!(url);
    out["rpc_port"] = json!(session.rpc);
    if browse {
        super::report::open(&url);
    }
    if let Some(listener) = own {
        // An older daemon (or none): serve here. `strobe up` lets the daemon serve it instead.
        print!("{text}");
        println!("serving the Perfetto UI here until Ctrl-C (the running daemon {}; `strobe up` lets it serve)", if super::mcp::running() { "is another build" } else { "is not running" });
        // Only this session's pages: no `open/` (its token is the only key to a trace here).
        let handler: std::sync::Arc<dyn Fn(http::Request) -> http::Response + Send + Sync> =
            std::sync::Arc::new(move |req: http::Request| refused(&req, port).or_else(|| serve(&req, port, false)).unwrap_or_else(|| http::Response::text(404, "not found")));
        http::serve(listener, handler);
    }
    Ok(Output::new(text, out))
}

/// A handle as the recording to open: `rec_…`, or an experiment's attribution recording (`head`,
/// `base`), block (`<n>`, counted over its series in order) or series (`<case>`).
pub fn recording(handle: &str) -> Result<(PathBuf, String)> {
    recording_by(handle, store::resolve)
}

/// `recording`, each id (the handle's, and one its experiment's result names) found by `find`: the
/// store's prefix match for a command, the exact id alone for a request (`exact`).
fn recording_by(handle: &str, find: impl Fn(&str) -> Result<PathBuf>) -> Result<(PathBuf, String)> {
    let (h, pick) = handle.split_once('/').map_or((handle, None), |(a, b)| (a, Some(b)));
    // A request's or a command's handle reaches the store only as a plain id, never as a path.
    if !store::plain_id(h) {
        return Err(Error::usage(format!("{h}: a handle is an id (rec_…, exp_…) or its first letters: letters, digits, _ and -")));
    }
    let dir = find(h)?;
    if !store::is_experiment(&dir) {
        if pick.is_some() {
            return Err(Error::usage(format!("{h} is a recording: strobe open {h}")));
        }
        let id = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        return Ok((dir, id));
    }
    let r = store::read_json(&dir.join("result.json"))?;
    let exp = r["id"].as_str().unwrap_or(h).to_string();
    // (series case or label, role, recording)
    let mut why: Vec<(String, String, String)> = Vec::new();
    let mut blocks: Vec<(String, Option<String>)> = Vec::new();
    for s in r["series"].as_array().into_iter().flatten() {
        let case = s["case"].as_str().or(s["label"].as_str()).unwrap_or("").to_string();
        match &s["attribution"] {
            Value::Object(o) => {
                for role in ["head", "base"] {
                    if let Some(rec) = o.get(role).and_then(Value::as_str) {
                        why.push((case.clone(), role.to_string(), rec.to_string()));
                    }
                }
            }
            Value::String(rec) => why.push((case.clone(), "head".into(), rec.clone())),
            _ => {}
        }
        if let Some(rec) = s["recording"].as_str() {
            why.push((case.clone(), "check".into(), rec.to_string()));
        }
        for b in s["blocks"].as_array().into_iter().flatten() {
            blocks.push((format!("{case} {} pair {}", b["variant"].as_str().unwrap_or(""), b["pair"]), b["recording"].as_str().map(str::to_string)));
        }
    }
    let choices = || {
        let mut c: Vec<String> = why.iter().map(|(case, role, rec)| format!("{exp}/{role} ({case}: {rec})")).collect();
        let kept: Vec<String> = blocks.iter().enumerate().filter(|(_, b)| b.1.is_some()).map(|(i, _)| i.to_string()).collect();
        if !kept.is_empty() {
            c.push(format!("{exp}/<block> (kept: {})", kept.join(", ")));
        }
        if c.is_empty() { "none: this experiment kept no recording (bench --why always, or --keep)".to_string() } else { c.join(", ") }
    };
    let rec = match pick {
        None => why.iter().find(|w| w.1 != "base").or(why.first()).map(|w| w.2.clone()).or_else(|| blocks.iter().find_map(|b| b.1.clone())),
        Some(role @ ("head" | "base" | "check")) => why.iter().find(|w| w.1 == role).map(|w| w.2.clone()),
        Some(n) if n.parse::<usize>().is_ok() => {
            let i: usize = n.parse().unwrap_or(0);
            match blocks.get(i) {
                Some((_, Some(rec))) => Some(rec.clone()),
                Some((label, None)) => return Err(Error::usage(format!("block {i} ({label}) kept no recording (bench --keep keeps every block); this experiment has {}", choices()))),
                None => None,
            }
        }
        Some(case) => why.iter().find(|w| w.0 == case && w.1 != "base").map(|w| w.2.clone()),
    };
    let rec = rec.ok_or_else(|| Error::usage(format!("{handle}: no such recording; {exp} has {}", choices())))?;
    // An id the result names is a plain id before it reaches the store or the session's page.
    if !store::plain_id(&rec) {
        return Err(Error::usage(format!("{handle}: {exp}'s result names a recording by something other than an id, which strobe does not open")));
    }
    let dir = find(&rec).map_err(|_| Error::usage(format!("{rec} (from {handle}) is no longer stored")))?;
    Ok((dir, rec))
}

/// A stored recording or experiment by its exact id, as a request names one: a plain id, never a
/// prefix (a prefix, and an "ambiguous" answer, would let any local caller list the store).
fn exact(h: &str) -> Result<PathBuf> {
    let roots = if h.starts_with("exp_") {
        vec![paths::experiments()]
    } else if h.starts_with("rec_") {
        vec![paths::recordings()]
    } else {
        vec![paths::recordings(), paths::experiments()]
    };
    roots.into_iter().map(|r| r.join(h)).find(|d| store::plain_id(h) && d.is_dir()).ok_or_else(|| Error::usage("not found"))
}

/// A request's handle as a stored folder (`exact`): `/report/<id>`, `/video/<id>`.
pub fn stored(h: &str) -> Option<PathBuf> {
    exact(h).ok()
}

struct Prepared {
    trace: PathBuf,
    text: String,
    json: Value,
}

/// Builds the recording's trace and checks it with trace_processor before anything opens it.
fn prepare(dir: &Path, id: &str) -> Result<Prepared> {
    let trace = dir.join("perfetto.pftrace");
    progress(format!("building {id}'s Perfetto trace"));
    let b = perfetto::build(dir, &trace)?;
    progress("checking it with trace_processor");
    let table = engine::query(
        &trace,
        "select 'samples' as k, source as a, count(*) as b, null as c from stack_sample group by source
union all select 'frames', count(*), sum(coalesce(s.name, f.name, '') <> ''), sum(s.source_file is not null)
  from stack_profile_frame f left join stack_profile_symbol s on s.symbol_set_id = f.symbol_set_id
union all select 'tracks', count(*), null, null from track
union all select 'stat', name, value, severity from stats where value > 0 and severity in ('error', 'data_loss')",
    )?;
    let num = |c: &Option<String>| c.as_deref().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) as u64;
    let (mut read, mut frames, mut named, mut located, mut tracks) = (std::collections::BTreeMap::new(), 0, 0, 0, 0);
    let mut errors: Vec<String> = Vec::new();
    for row in &table.rows {
        match row[0].as_deref() {
            Some("samples") => {
                read.insert(row[1].clone().unwrap_or_default(), num(&row[2]));
            }
            Some("frames") => (frames, named, located) = (num(&row[1]), num(&row[2]), num(&row[3])),
            Some("tracks") => tracks = num(&row[1]),
            Some("stat") => errors.push(format!("{} {}", row[1].as_deref().unwrap_or(""), num(&row[2]))),
            _ => {}
        }
    }
    let mut text = format!("{id} → {} · {}\n", trace.display(), fmt::bytes(b.bytes as f64));
    if b.chrome {
        text += "  chrome    the recording's Chrome trace, as recorded\n";
    }
    let mut stacks = Vec::new();
    let mut short = Vec::new();
    for (source, n, what) in
        [("javascript", b.js_samples, "JavaScript CPU"), ("javascript.allocations", b.alloc_samples, "allocation"), ("native", b.native_samples, "native"), ("firefox", b.gecko_samples, "Firefox CPU")]
    {
        if n == 0 {
            continue;
        }
        stacks.push(format!("{} {what} samples", fmt::value(n as f64, "count")));
        let got = read.get(source).copied().unwrap_or(0);
        if got != n {
            short.push(format!("{source}: {got} of {n}"));
        }
    }
    if !stacks.is_empty() {
        text += &format!(
            "  stacks    {} · {} thread{} · {} frames, {} with file:line (flame graphs: select a range on a Callstacks track)\n",
            stacks.join(", "),
            b.sampled_threads,
            if b.sampled_threads == 1 { "" } else { "s" },
            fmt::value(b.frames as f64, "count"),
            fmt::value(b.located_frames as f64, "count")
        );
    }
    if !b.tracks.is_empty() {
        text += &format!("  tracks    {}\n", b.tracks.iter().map(|(k, n)| format!("{k} {}", fmt::value(*n as f64, "count"))).collect::<Vec<_>>().join(" · "));
    }
    text += &format!(
        "  checked   trace_processor {}: {} samples, {} frames ({} named, {} with a source file), {} tracks, {}\n",
        engine::VERSION,
        fmt::value(read.values().sum::<u64>() as f64, "count"),
        fmt::value(frames as f64, "count"),
        fmt::value(named as f64, "count"),
        fmt::value(located as f64, "count"),
        fmt::value(tracks as f64, "count"),
        if errors.is_empty() { "no import errors".to_string() } else { format!("its error stats: {}", errors.join(", ")) }
    );
    if !short.is_empty() {
        text += &format!("  ! trace_processor read fewer samples than were written ({})\n", short.join("; "));
    }
    for n in &b.notes {
        text += &format!("  i {n}\n");
    }
    let json = json!({
        "recording": id, "trace": trace.display().to_string(), "bytes": b.bytes, "built": b,
        "checked": { "samples": read, "frames": frames, "named_frames": named, "located_frames": located, "tracks": tracks, "errors": errors },
    });
    Ok(Prepared { trace, text, json })
}

/// The Perfetto UI on disk, downloaded and unpacked once.
fn ui() -> Result<String> {
    if engine::ui_version().is_none() {
        progress(format!("downloading the Perfetto UI {} (17 MB, once)", engine::VERSION));
    }
    engine::ensure_ui()?;
    engine::ui_version().ok_or_else(|| Error::msg("the Perfetto UI archive holds no version folder"))
}

struct Session {
    token: String,
    rpc: Option<u16>,
    note: Option<String>,
}

/// A trace_processor for the UI on a port of its own, and a token that lets the UI fetch the trace
/// (16 bytes from the OS's secure random source: it is the only key to the trace).
fn start(trace: &Path, id: &str, ui_port: u16) -> Result<Session> {
    let token = paths::csprng_hex(16)?;
    let free = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port());
    let (rpc, note) = match free.map_err(Error::from).and_then(|p| engine::httpd(p, &format!("http://127.0.0.1:{ui_port}")).map(|_| p)) {
        Ok(p) => (Some(p), None),
        Err(e) => (None, Some(format!("{}; the UI loads the trace itself", e.message))),
    };
    // A session file is the key to its trace: it is the user's alone, in the user-only run folder.
    let dir = paths::run().join("perfetto");
    let _ = std::fs::create_dir_all(&dir);
    // Sessions from earlier days are gone; the traces stay in their recordings.
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 86_400) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let file = dir.join(format!("{token}.json"));
    paths::write_owner_only(&file, json!({ "trace": trace.display().to_string(), "recording": id, "rpc_port": rpc }).to_string().as_bytes())?;
    Ok(Session { token, rpc, note })
}

fn session(token: &str) -> Option<Value> {
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    store::read_json(&paths::run().join("perfetto").join(format!("{token}.json"))).ok()
}

/// The page a session opens at: it turns on the UI flag that lets it reach a trace_processor on its
/// own port and turns the UI's telemetry off (both in this origin's storage), then opens the trace
/// with strobe's runs and the sampled threads' Callstacks tracks pinned to the top.
fn session_page(token: &str, s: &Value) -> String {
    let rec = s["recording"].as_str().unwrap_or("trace");
    let rpc = s["rpc_port"].as_u64().map(|p| format!(" + '&rpc_port={p}'")).unwrap_or_default();
    format!(
        r#"<!doctype html><meta charset="utf-8"><title>strobe · {rec} in Perfetto</title>
<p style="font:13px system-ui;color:#888">Opening {rec} in Perfetto…</p>
<script>
const edit = (key, change) => {{ let v = {{}}; try {{ v = JSON.parse(localStorage.getItem(key) || '{{}}') || {{}}; }} catch (e) {{}} change(v); localStorage.setItem(key, JSON.stringify(v)); }};
edit('perfettoFeatureFlags', (f) => {{ f.cspAllowAnyWebsocketPort = 'OVERRIDE_TRUE'; }});
edit('perfettoSettings', (s) => {{ s.analyticsEnable = false; }});
const commands = [
  {{ id: 'dev.perfetto.PinTracksByRegex', args: ['^runs and phases$|Callstacks', 'name'] }},
  {{ id: 'dev.perfetto.ExpandTracksByRegex', args: ['^strobe ', 'name'] }},
];
location.replace('/perfetto/#!/?url=' + encodeURIComponent(location.origin + '/perfetto/t/{token}/{rec}.pftrace') + '&startupCommands=' + encodeURIComponent(JSON.stringify(commands)){rpc});
</script>
"#
    )
}

/// A file of the Perfetto UI a request names (`/perfetto/<file>`): only a relative path of plain
/// steps (letters, digits, `_`, `-`, `.`, `+`, `@`; no empty step, no `.` or `..`, no `\`, drive or
/// stream), that is, once every link is followed, a regular file inside the UI's folder. Anything else
/// is no file of the UI: a request never reaches a file outside it (a second `/` once made `join`
/// replace the folder with the disk's root).
fn ui_file(file: &str) -> Option<PathBuf> {
    let plain = |step: &str| !step.is_empty() && step != "." && step != ".." && step.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-.+@".contains(&c));
    if !file.split('/').all(plain) {
        return None;
    }
    let root = std::fs::canonicalize(engine::ui_dir()).ok()?;
    let found = std::fs::canonicalize(root.join(file)).ok()?;
    (found.starts_with(&root) && found.is_file()).then_some(found)
}

/// A request the viewer's servers answer at all: one addressed to this machine's loopback by the name
/// a browser on it uses (`Host: 127.0.0.1:<port>` or `localhost:<port>`, so a page whose name was
/// rebound to 127.0.0.1 is refused), and, with an `Origin`, a GET from these pages themselves (or the
/// `null` origin of a file or sandboxed page, which a response without CORS headers lets read
/// nothing). The daemon and `strobe open`'s own server ask the same.
pub fn refused(req: &http::Request, port: u16) -> Option<http::Response> {
    let host = req.headers.get("host").map(String::as_str).unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Some(http::Response::text(403, "bad host"));
    }
    let same = |o: &str| o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}");
    // A `null` origin may GET (it reads nothing back), never send anything else.
    if let Some(origin) = req.headers.get("origin")
        && !(req.method == "GET" && (origin == "null" || same(origin)))
    {
        return Some(http::Response::text(403, "cross-origin requests are not allowed"));
    }
    None
}

/// `/perfetto/…` on the daemon (or on `strobe open` itself): the UI's files, a session's page and
/// trace, and on the daemon (`daemon`) `open/<id>`, which builds a stored recording's trace and opens
/// it (the report's link). GET alone.
pub fn serve(req: &http::Request, port: u16, daemon: bool) -> Option<http::Response> {
    if req.method != "GET" {
        return None;
    }
    let path = req.path.split(['?', '#']).next().unwrap_or("/");
    let rest = path.strip_prefix("/perfetto")?;
    let html = "text/html; charset=utf-8";
    if rest.is_empty() {
        return Some(http::Response::new(200, html, r#"<meta http-equiv="refresh" content="0;url=/perfetto/">"#));
    }
    let rest = rest.strip_prefix('/')?;
    if rest == "version" {
        return Some(http::Response::text(200, &version()));
    }
    if let Some(t) = rest.strip_prefix("s/") {
        return Some(match session(t) {
            Some(s) => http::Response::new(200, html, session_page(t, &s)),
            None => http::Response::text(404, "no such session: run strobe open again"),
        });
    }
    if let Some(t) = rest.strip_prefix("t/") {
        let token = t.split('/').next().unwrap_or("");
        let body = session(token).and_then(|s| std::fs::read(s["trace"].as_str()?).ok());
        return Some(match body {
            Some(b) => http::Response::new(200, "application/octet-stream", b),
            None => http::Response::text(404, "no such trace"),
        });
    }
    if let Some(handle) = rest.strip_prefix("open/").filter(|_| daemon) {
        // It builds a trace and starts a trace_processor: only from these pages (a report's Open
        // button) or a person typing the address, never because another site's page asked.
        if req.headers.get("sec-fetch-site").is_some_and(|s| s != "same-origin" && s != "none") {
            return Some(http::Response::text(403, "cross-site requests are not allowed"));
        }
        return Some(open_request(handle, port));
    }
    let file = if rest.is_empty() { "index.html" } else { rest };
    let Some(on_disk) = ui_file(file) else { return Some(http::Response::text(404, "not found")) };
    let body = std::fs::read(&on_disk).ok()?;
    // The kind and the script policy of the file served, by its own name (`INDEX.HTML` is index.html).
    let ext = on_disk.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let csp = Some(policy((ext == "html").then(|| String::from_utf8_lossy(&body).to_string()).as_deref()));
    let kind = match ext.as_str() {
        "html" => html,
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" | "map" => "application/json",
        "woff2" => "font/woff2",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    Some(http::Response { csp, ..http::Response::new(200, kind, body) })
}

/// The sessions the daemon opened for requests, by recording (its id, the session's token and
/// trace_processor port): one per recording, reused while its trace_processor answers.
static OPENED: std::sync::Mutex<Vec<(String, String, Option<u16>)>> = std::sync::Mutex::new(Vec::new());

/// The most recordings the daemon keeps open for requests at once (each a trace_processor that ends
/// after an hour unused): any local caller may ask, so it is bounded.
const MAX_OPENED: usize = 8;

/// `GET /perfetto/open/<id>[/<pick>]` on the daemon: the session page of a stored recording named by its
/// exact id. One open at a time; a recording already open (its session and trace_processor still there)
/// answers with that session, building and starting nothing. Every refusal says only `not found` (or
/// that too many are open): nothing of the request or the store.
fn open_request(handle: &str, port: u16) -> http::Response {
    let html = "text/html; charset=utf-8";
    let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    let Ok((dir, id)) = recording_by(handle, exact) else { return http::Response::text(404, "not found") };
    let alive = |token: &str, rpc: Option<u16>| {
        session(token).is_some() && rpc.is_none_or(|p| std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], p)), std::time::Duration::from_millis(300)).is_ok())
    };
    // A view that opened marks what it opened used (and the experiment it was named through): never a
    // failed or refused one.
    let stamp = || {
        store::used(&dir);
        if let Ok(top) = exact(handle.split('/').next().unwrap_or(handle))
            && store::is_experiment(&top)
        {
            store::used(&top);
        }
    };
    opened.retain(|(_, t, rpc)| alive(t, *rpc));
    if let Some((_, token, rpc)) = opened.iter().find(|(r, _, _)| *r == id) {
        stamp();
        return http::Response::new(200, html, session_page(token, &json!({ "recording": id, "rpc_port": rpc })));
    }
    if opened.len() >= MAX_OPENED {
        return http::Response::text(503, "too many recordings open: close some Perfetto tabs, or wait an hour");
    }
    let started = (|| -> Result<Session> {
        let p = prepare(&dir, &id)?;
        engine::ensure_ui()?;
        start(&p.trace, &id, port)
    })();
    match started {
        Ok(s) => {
            opened.push((id.clone(), s.token.clone(), s.rpc));
            stamp();
            http::Response::new(200, html, session_page(&s.token, &json!({ "recording": id, "rpc_port": s.rpc })))
        }
        Err(_) => http::Response::text(500, "it could not be opened: strobe open in a terminal says why"),
    }
}

/// What the UI may reach: this origin and a trace_processor on 127.0.0.1. Its own policy also lets
/// it load Google analytics, an internal extension server and a userscript; a browser enforces both
/// policies, so nothing leaves the machine. A page's inline scripts are allowed by their hashes.
fn policy(html: Option<&str>) -> String {
    use base64::Engine;
    use sha2::Digest;
    let mut script = String::from("'self' 'unsafe-eval' 'wasm-unsafe-eval' blob:");
    let mut rest = html.unwrap_or("");
    while let Some(at) = rest.find("<script") {
        let Some(open) = rest[at..].find('>') else { break };
        let tag = &rest[at..at + open];
        let body = &rest[at + open + 1..];
        let Some(close) = body.find("</script>") else { break };
        if !tag.contains("src=") {
            script += &format!(" 'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(&body.as_bytes()[..close])));
        }
        rest = &body[close..];
    }
    format!(
        "default-src 'self'; script-src {script}; worker-src 'self' blob:; connect-src 'self' http://127.0.0.1:* ws://127.0.0.1:* blob: data:; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; object-src 'none'"
    )
}

/// The UI and the revision of the pages strobe serves around it (bump `PAGES` when `serve` changes):
/// a daemon of another strobe may serve other pages. Not the build hash: the daemon runs strobew.
fn version() -> String {
    const PAGES: u32 = 3;
    format!("{} pages {PAGES}", engine::ui_version().unwrap_or_default())
}

/// A GET to the daemon, its body when it answers 200.
fn get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), std::time::Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(2))).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let mut text = String::new();
    let _ = s.read_to_string(&mut text);
    let (head, body) = text.split_once("\r\n\r\n")?;
    head.starts_with("HTTP/1.1 200").then(|| body.trim().to_string())
}

/// The line a pull request body can carry under its rerun command: how to see the profiled
/// launches on one timeline with flame graphs. Kept apart so the PR layout can place it.
pub fn pr_line(r: &Value) -> Option<String> {
    let exp = r["id"].as_str()?;
    let has = r["series"].as_array()?.iter().any(|s| s["attribution"].is_object() || s["attribution"].is_string());
    has.then(|| format!("Timeline and flame graphs of the profiled launches (local Perfetto UI): `strobe open {exp}/head`, `strobe open {exp}/base`.\n"))
}
