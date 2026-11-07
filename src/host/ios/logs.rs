//! Process-allowlisted iOS device logs (`com.apple.os_trace_relay` with fallback to
//! `com.apple.syslog_relay`) and strictly read-only crash/jetsam collection
//! (`com.apple.crashreportmover` + `com.apple.crashreportcopymobile` AFC).
//!
//! Privacy is enforced in memory BEFORE anything is stored: only processes belonging to the
//! inspected browser or WKWebView (`MobileSafari` + `com.apple.WebKit.*` for Safari; the host app +
//! `com.apple.WebKit.*` for a WKWebView) pass `Allowlist::allows`. Every other personal app's log
//! lines and Jetsam process-table rows are dropped immediately on the reader thread.

use super::{Device, Service, session_for};
use crate::model::{Error, Result};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_LOG_ENTRIES: usize = 5_000;
const MAX_PACKET: usize = 1 << 20;
const MAX_CRASH_BYTES: usize = 2 << 20;
const TRACE_HEADER_LEN: usize = 129;

/// Process allowlist checked in memory before keeping any device log entry or crash report.
#[derive(Clone, Debug)]
pub struct Allowlist {
    safari: bool,
    bundle_id: String,
    host_exe: Option<String>,
    pids: HashSet<u32>,
}

impl Allowlist {
    pub fn new(bundle_id: &str, app_pid: Option<u32>) -> Self {
        let safari = bundle_id.is_empty() || bundle_id == "com.apple.mobilesafari";
        let host_exe = (!safari).then(|| bundle_id.rsplit('.').next().unwrap_or(bundle_id).to_string()).filter(|s| !s.is_empty());
        let mut pids = HashSet::new();
        if let Some(p) = app_pid.filter(|&p| p > 0) {
            pids.insert(p);
        }
        Allowlist { safari, bundle_id: bundle_id.to_string(), host_exe, pids }
    }

    pub fn add_pid(&mut self, pid: u32) {
        if pid > 0 {
            self.pids.insert(pid);
        }
    }

    /// Whether a process name or bundle identifier belongs to the inspected browser/WebView or WebKit.
    /// `webinspectord` is excluded here and counted separately as observer noise (`is_observer_noise`).
    pub fn allows_name(&self, raw: &str) -> bool {
        let base = raw.rsplit('/').next().unwrap_or(raw).trim();
        if base.is_empty() {
            return false;
        }
        if base.starts_with("com.apple.WebKit") || matches!(base, "WebContent" | "WebKit") {
            return true;
        }
        if self.safari {
            return matches!(base, "MobileSafari" | "SafariViewService" | "com.apple.mobilesafari");
        }
        base == self.bundle_id || self.host_exe.as_deref() == Some(base)
    }

    pub fn allows(&self, pid: Option<u32>, process: &str) -> bool {
        pid.is_some_and(|p| self.pids.contains(&p)) || self.allows_name(process)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceLogEntry {
    pub at_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub process: String,
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subsystem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceLogs {
    pub source: String,
    pub entries: Vec<DeviceLogEntry>,
    pub dropped: u64,
    /// Web Inspector relay / message-trace lines dropped in memory so strobe does not log its own IPC.
    #[serde(default)]
    pub observer_dropped: u64,
}

/// Whether a device log line is Web Inspector's own IPC / message-trace observer traffic.
fn is_observer_noise(process: &str, subsystem: Option<&str>, category: Option<&str>, message: &str) -> bool {
    if process == "webinspectord" {
        return true;
    }
    if subsystem.is_some_and(|s| s.starts_with("com.apple.WebInspector") || s.starts_with("com.apple.webinspectord")) {
        return true;
    }
    if category == Some("message-trace") {
        return true;
    }
    message.contains("message-trace")
        || message.contains("_rpc_forwardSocketData:")
        || message.contains("_rpc_applicationSentData:")
        || message.contains("WIRSocketDataKey")
        || message.contains("WIRMessageDataKey")
}

pub struct LogStream {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<DeviceLogs>>,
}

impl LogStream {
    /// Starts streaming allowlisted live device logs (`wall_s >= wall0_s - 1.0`, no historical buffer)
    /// on a background thread. If neither log relay is available, returns an empty capture on stop.
    pub fn start(device: &Device, allowlist: Allowlist, t0_ns: u64, wall0_s: f64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let dev = device.clone();
        let handle = std::thread::spawn(move || run_log_stream(&dev, allowlist, t0_ns, wall0_s, &stop2));
        LogStream { stop, handle: Some(handle) }
    }

    pub fn stop(mut self) -> DeviceLogs {
        self.stop.store(true, Ordering::SeqCst);
        let Some(h) = self.handle.take() else { return DeviceLogs::default() };
        let deadline = Instant::now() + Duration::from_millis(1500);
        while !h.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if h.is_finished() { h.join().unwrap_or_default() } else { DeviceLogs::default() }
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn run_log_stream(dev: &Device, mut allow: Allowlist, t0_ns: u64, wall0_s: f64, stop: &AtomicBool) -> DeviceLogs {
    let setup_deadline = Some(Instant::now() + Duration::from_secs(3));
    let Ok(mut l) = session_for(dev, setup_deadline) else { return DeviceLogs::default() };
    // Enrich the allowlist with live PIDs from PidList when os_trace_relay supports it.
    if let Ok(mut s) = l.start_service("com.apple.os_trace_relay") {
        s.set_deadline(Some(Instant::now() + Duration::from_millis(800)));
        if let Ok(pids) = ostrace_pid_list(&mut s) {
            for (pid, name) in pids {
                if allow.allows_name(&name) {
                    allow.add_pid(pid);
                }
            }
        }
        s.close();
    }
    if stop.load(Ordering::Relaxed) {
        return DeviceLogs::default();
    }
    if let Ok(mut s) = l.start_service("com.apple.os_trace_relay")
        && ostrace_start(&mut s).is_ok()
    {
        let out = read_ostrace(&mut s, &allow, t0_ns, wall0_s, stop);
        s.close();
        return out;
    }
    if let Ok(mut s) = l.start_service("com.apple.syslog_relay") {
        let out = read_syslog(&mut s, &allow, t0_ns, stop);
        s.close();
        return out;
    }
    DeviceLogs::default()
}

fn ostrace_recv_plist(s: &mut Service) -> Result<Dictionary> {
    let mut tag = [0u8; 1];
    s.read_exact(&mut tag).map_err(|e| Error::msg(e.to_string()))?;
    let mut raw_len = [0u8; 4];
    s.read_exact(&mut raw_len).map_err(|e| Error::msg(e.to_string()))?;
    let len = match tag[0] {
        1 => u32::from_be_bytes(raw_len) as usize,
        2 => u32::from_le_bytes(raw_len) as usize,
        other => return Err(Error::msg(format!("os_trace_relay tag {other}"))),
    };
    if len > MAX_PACKET {
        return Err(Error::msg("os_trace_relay reply too large"));
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).map_err(|e| Error::msg(e.to_string()))?;
    Value::from_reader(Cursor::new(&body)).map_err(|e| Error::msg(e.to_string()))?.into_dictionary().ok_or_else(|| Error::msg("os_trace_relay reply is not a dictionary"))
}

fn ostrace_pid_list(s: &mut Service) -> Result<Vec<(u32, String)>> {
    let mut req = Dictionary::new();
    req.insert("Request".into(), "PidList".into());
    s.send(&Value::Dictionary(req), true)?;
    let reply = ostrace_recv_plist(s)?;
    if reply.get("Status").and_then(Value::as_string) != Some("RequestSuccessful") {
        return Err(Error::msg("PidList refused"));
    }
    let mut out = Vec::new();
    if let Some(payload) = reply.get("Payload").and_then(Value::as_dictionary) {
        for (k, v) in payload {
            if let (Ok(pid), Some(name)) = (k.parse::<u32>(), v.as_dictionary().and_then(|d| d.get("ProcessName")).and_then(Value::as_string)) {
                out.push((pid, name.to_string()));
            }
        }
    }
    Ok(out)
}

fn ostrace_start(s: &mut Service) -> Result<()> {
    s.set_deadline(Some(Instant::now() + Duration::from_secs(2)));
    let mut req = Dictionary::new();
    req.insert("Request".into(), "StartActivity".into());
    req.insert("Pid".into(), (-1i64).into());
    req.insert("MessageFilter".into(), 0xffffu64.into());
    // PAYLOAD (0x04) | CALLSTACK (0x10) | DEBUG (0x20) = 0x34: deliberately omits HISTORICAL (0x08)
    // so pre-recording personal logs from before strobe attached are never streamed.
    req.insert("StreamFlags".into(), 0x34u64.into());
    s.send(&Value::Dictionary(req), true)?;
    let reply = ostrace_recv_plist(s)?;
    if reply.get("Status").and_then(Value::as_string) == Some("RequestSuccessful") { Ok(()) } else { Err(Error::msg("os_trace_relay StartActivity refused")) }
}

enum ParsedLog {
    Keep(DeviceLogEntry),
    ObserverNoise,
    Filtered,
}

fn read_ostrace(s: &mut Service, allow: &Allowlist, t0_ns: u64, wall0_s: f64, stop: &AtomicBool) -> DeviceLogs {
    let mut logs = DeviceLogs { source: "os_trace_relay".into(), entries: Vec::new(), dropped: 0, observer_dropped: 0 };
    while !stop.load(Ordering::Relaxed) {
        s.set_deadline(Some(Instant::now() + Duration::from_millis(150)));
        let mut tag = [0u8; 1];
        match s.read_exact(&mut tag) {
            Ok(()) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => break,
        }
        if tag[0] != 0x02 {
            break;
        }
        s.set_deadline(Some(Instant::now() + Duration::from_millis(500)));
        let mut raw_len = [0u8; 4];
        if s.read_exact(&mut raw_len).is_err() {
            break;
        }
        let len = u32::from_le_bytes(raw_len) as usize;
        if len > MAX_PACKET {
            break;
        }
        let mut pkt = vec![0u8; len];
        if s.read_exact(&mut pkt).is_err() {
            break;
        }
        // Filter in memory BEFORE allocating or retaining non-target or observer-noise log messages.
        match parse_ostrace_entry(&pkt, allow, t0_ns, wall0_s) {
            ParsedLog::Keep(entry) => {
                if logs.entries.len() < MAX_LOG_ENTRIES {
                    logs.entries.push(entry);
                } else {
                    logs.dropped += 1;
                }
            }
            ParsedLog::ObserverNoise => logs.observer_dropped += 1,
            ParsedLog::Filtered => {}
        }
    }
    logs
}

fn parse_ostrace_entry(pkt: &[u8], allow: &Allowlist, t0_ns: u64, wall0_s: f64) -> ParsedLog {
    if pkt.len() < TRACE_HEADER_LEN {
        return ParsedLog::Filtered;
    }
    let pid = u32::from_le_bytes([pkt[9], pkt[10], pkt[11], pkt[12]]);
    let seconds = i64::from_le_bytes([pkt[55], pkt[56], pkt[57], pkt[58], pkt[59], pkt[60], pkt[61], pkt[62]]) as f64;
    let micros = u32::from_le_bytes([pkt[63], pkt[64], pkt[65], pkt[66]]) as f64;
    let wall_s = seconds + micros * 1e-6;
    if wall0_s > 0.0 && wall_s > 0.0 && wall_s < wall0_s - 1.0 {
        return ParsedLog::Filtered;
    }
    let level_byte = pkt[68];
    let image_len = u16::from_le_bytes([pkt[107], pkt[108]]) as usize;
    let msg_len_u32 = u32::from_le_bytes([pkt[109], pkt[110], pkt[111], pkt[112]]) as usize;
    let msg_len_u16 = u16::from_le_bytes([pkt[109], pkt[110]]) as usize;
    let sub_len = u32::from_le_bytes([pkt[117], pkt[118], pkt[119], pkt[120]]) as usize;
    let cat_len = u32::from_le_bytes([pkt[121], pkt[122], pkt[123], pkt[124]]) as usize;

    let rest = &pkt[TRACE_HEADER_LEN..];
    let Some(fn_end) = rest.iter().position(|&b| b == 0) else { return ParsedLog::Filtered };
    let Ok(filename) = std::str::from_utf8(&rest[..fn_end]) else { return ParsedLog::Filtered };
    let process = filename.rsplit('/').next().unwrap_or(filename);
    if process == "webinspectord" {
        return ParsedLog::ObserverNoise;
    }
    if !allow.allows(Some(pid), process) {
        return ParsedLog::Filtered;
    }
    let after_fn = &rest[fn_end + 1..];
    let label_len = if sub_len > 0 && cat_len > 0 { sub_len.saturating_add(cat_len) } else { 0 };
    // `message_size` at +109 is u32 on 64-bit Darwin (matching `after_fn.len()` for >= 64 KiB messages);
    // fall back to u16 only when +111..+113 holds non-zero flags on a shorter packet.
    let msg_len = if image_len.checked_add(msg_len_u32).and_then(|n| n.checked_add(label_len)).is_some_and(|total| total <= after_fn.len()) {
        msg_len_u32
    } else if image_len.checked_add(msg_len_u16).and_then(|n| n.checked_add(label_len)).is_some_and(|total| total <= after_fn.len()) {
        msg_len_u16
    } else {
        return ParsedLog::Filtered;
    };
    let msg_slice = &after_fn[image_len..image_len + msg_len];
    let msg_trimmed = msg_slice.strip_suffix(&[0]).unwrap_or(msg_slice);
    let message = String::from_utf8_lossy(msg_trimmed).trim().to_string();
    if message.is_empty() {
        return ParsedLog::Filtered;
    }
    let tail = &after_fn[image_len + msg_len..];
    let (subsystem, category) = if sub_len > 0 && cat_len > 0 && tail.len() >= label_len {
        let sub = String::from_utf8_lossy(tail[..sub_len].strip_suffix(&[0]).unwrap_or(&tail[..sub_len])).into_owned();
        let cat = String::from_utf8_lossy(tail[sub_len..sub_len + cat_len].strip_suffix(&[0]).unwrap_or(&tail[sub_len..sub_len + cat_len])).into_owned();
        (Some(sub).filter(|s| !s.is_empty()), Some(cat).filter(|s| !s.is_empty()))
    } else {
        (None, None)
    };
    if is_observer_noise(process, subsystem.as_deref(), category.as_deref(), &message) {
        return ParsedLog::ObserverNoise;
    }
    let level = match level_byte {
        0x11 => "fault",
        0x10 => "error",
        0x02 => "debug",
        0x01 => "info",
        _ => "notice",
    }
    .to_string();
    let at_ms = crate::host::now_ns().saturating_sub(t0_ns) as f64 / 1e6;
    ParsedLog::Keep(DeviceLogEntry { at_ms, wall_s: Some(wall_s), pid: Some(pid), process: process.to_string(), level, subsystem, category, message })
}

fn read_syslog(s: &mut Service, allow: &Allowlist, t0_ns: u64, stop: &AtomicBool) -> DeviceLogs {
    let mut logs = DeviceLogs { source: "syslog_relay".into(), entries: Vec::new(), dropped: 0, observer_dropped: 0 };
    let mut pending = Vec::new();
    let mut buf = [0u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        s.set_deadline(Some(Instant::now() + Duration::from_millis(150)));
        let n = match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => break,
        };
        pending.extend_from_slice(&buf[..n]);
        while let Some(pos) = pending.iter().position(|&b| b == b'\n' || b == 0) {
            let line_bytes: Vec<u8> = pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            match parse_syslog_line(line.trim_matches(['\0', '\r', '\n', ' ']), allow, t0_ns) {
                ParsedLog::Keep(entry) => {
                    if logs.entries.len() < MAX_LOG_ENTRIES {
                        logs.entries.push(entry);
                    } else {
                        logs.dropped += 1;
                    }
                }
                ParsedLog::ObserverNoise => logs.observer_dropped += 1,
                ParsedLog::Filtered => {}
            }
        }
        if pending.len() > 64 * 1024 {
            pending.clear();
        }
    }
    logs
}

/// Parses `Nov  8 06:15:02 Device ProcessName[512] <Notice>: message`, parsing proc/level from the right
/// of `: ` so device or process names containing spaces never shift columns.
fn parse_syslog_line(line: &str, allow: &Allowlist, t0_ns: u64) -> ParsedLog {
    if line.len() < 16 {
        return ParsedLog::Filtered;
    }
    let Some(rest) = line.get(16..).map(str::trim_start) else { return ParsedLog::Filtered };
    let Some(colon) = rest.find(": ") else { return ParsedLog::Filtered };
    let head = rest[..colon].trim_end();
    let message = rest[colon + 2..].trim();
    if message.is_empty() {
        return ParsedLog::Filtered;
    }
    let (before_level, level_part) = match head.strip_suffix('>').and_then(|h| h.rsplit_once(" <")) {
        Some((b, l)) => (b.trim_end(), l),
        None => (head, "notice"),
    };
    let proc_part = before_level.rsplit_once(' ').map_or(before_level, |(_, p)| p);
    let (process, pid) = match proc_part.strip_suffix(']').and_then(|p| p.rsplit_once('[')) {
        Some((name, id)) => (name, id.parse::<u32>().ok()),
        None => (proc_part, None),
    };
    // Often `MobileSafari(WebKit)[512]`: strip library suffix in parens for the process check.
    let base_proc = process.split('(').next().unwrap_or(process);
    if base_proc == "webinspectord" {
        return ParsedLog::ObserverNoise;
    }
    if !allow.allows(pid, base_proc) {
        return ParsedLog::Filtered;
    }
    if is_observer_noise(base_proc, None, None, message) {
        return ParsedLog::ObserverNoise;
    }
    let level = match level_part.to_ascii_lowercase().as_str() {
        "fault" | "emergency" | "alert" | "critical" => "fault",
        "error" | "err" => "error",
        "warning" | "warn" => "warning",
        "debug" => "debug",
        "info" => "info",
        _ => "notice",
    }
    .to_string();
    let at_ms = crate::host::now_ns().saturating_sub(t0_ns) as f64 / 1e6;
    ParsedLog::Keep(DeviceLogEntry { at_ms, wall_s: None, pid, process: base_proc.to_string(), level, subsystem: None, category: None, message: message.to_string() })
}

// Strictly read-only AFC client for com.apple.crashreportcopymobile.
// Only READ_DIR, GET_FILE_INFO, FILE_OPEN (RDONLY=1), FILE_READ, and FILE_CLOSE exist here:
// no write, rename or remove opcode is defined anywhere in strobe.

const AFC_MAGIC: &[u8; 8] = b"CFA6LPAA";
const AFC_HEADER_LEN: usize = 40;
const AFC_OP_STATUS: u64 = 0x01;
const AFC_OP_DATA: u64 = 0x02;
const AFC_OP_READ_DIR: u64 = 0x03;
const AFC_OP_GET_FILE_INFO: u64 = 0x0a;
const AFC_OP_FILE_OPEN: u64 = 0x0d;
const AFC_OP_FILE_OPEN_RES: u64 = 0x0e;
const AFC_OP_FILE_READ: u64 = 0x0f;
const AFC_OP_FILE_CLOSE: u64 = 0x14;
const AFC_FOPEN_RDONLY: u64 = 0x01;

struct ReadOnlyAfc {
    svc: Service,
    seq: u64,
}

impl ReadOnlyAfc {
    fn new(svc: Service) -> Self {
        ReadOnlyAfc { svc, seq: 0 }
    }

    fn call(&mut self, op: u64, header_data: &[u8]) -> Result<(u64, Vec<u8>)> {
        self.seq += 1;
        let total = (AFC_HEADER_LEN + header_data.len()) as u64;
        let mut pkt = Vec::with_capacity(total as usize);
        pkt.extend_from_slice(AFC_MAGIC);
        pkt.extend_from_slice(&total.to_le_bytes());
        pkt.extend_from_slice(&total.to_le_bytes());
        pkt.extend_from_slice(&self.seq.to_le_bytes());
        pkt.extend_from_slice(&op.to_le_bytes());
        pkt.extend_from_slice(header_data);
        self.svc.write_all(&pkt).map_err(|e| Error::msg(format!("AFC write: {e}")))?;
        self.svc.flush().map_err(|e| Error::msg(format!("AFC flush: {e}")))?;

        let mut head = [0u8; AFC_HEADER_LEN];
        self.svc.read_exact(&mut head).map_err(|e| Error::msg(format!("AFC read: {e}")))?;
        if &head[..8] != AFC_MAGIC {
            return Err(Error::msg("invalid AFC magic"));
        }
        let entire = u64::from_le_bytes(head[8..16].try_into().unwrap()) as usize;
        let resp_op = u64::from_le_bytes(head[32..40].try_into().unwrap());
        if entire < AFC_HEADER_LEN || entire - AFC_HEADER_LEN > MAX_CRASH_BYTES + 4096 {
            return Err(Error::msg("AFC response size out of bounds"));
        }
        let body_len = entire - AFC_HEADER_LEN;
        let mut body = vec![0u8; body_len];
        if body_len > 0 {
            self.svc.read_exact(&mut body).map_err(|e| Error::msg(format!("AFC payload: {e}")))?;
        }
        if resp_op == AFC_OP_STATUS {
            let code = if body.len() >= 8 { u64::from_le_bytes(body[..8].try_into().unwrap()) } else { 1 };
            if code != 0 {
                return Err(Error::msg(format!("AFC status {code}")));
            }
        }
        Ok((resp_op, body))
    }

    fn read_dir(&mut self, path: &str) -> Result<Vec<String>> {
        let mut arg = path.as_bytes().to_vec();
        arg.push(0);
        let (_, body) = self.call(AFC_OP_READ_DIR, &arg)?;
        Ok(body.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).filter(|s| s != "." && s != "..").collect())
    }

    fn stat(&mut self, path: &str) -> Result<HashMap<String, String>> {
        let mut arg = path.as_bytes().to_vec();
        arg.push(0);
        let (_, body) = self.call(AFC_OP_GET_FILE_INFO, &arg)?;
        let parts: Vec<String> = body.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
        let mut map = HashMap::new();
        for kv in parts.as_chunks::<2>().0 {
            map.insert(kv[0].clone(), kv[1].clone());
        }
        Ok(map)
    }

    fn read_file(&mut self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let mut arg = Vec::with_capacity(8 + path.len() + 1);
        arg.extend_from_slice(&AFC_FOPEN_RDONLY.to_le_bytes());
        arg.extend_from_slice(path.as_bytes());
        arg.push(0);
        let (op, body) = self.call(AFC_OP_FILE_OPEN, &arg)?;
        if op != AFC_OP_FILE_OPEN_RES || body.len() < 8 {
            return Err(Error::msg("AFC open returned no handle"));
        }
        let handle = u64::from_le_bytes(body[..8].try_into().unwrap());
        let mut out = Vec::new();
        while out.len() < max_bytes {
            let want = (max_bytes - out.len()).min(64 * 1024) as u64;
            let mut rarg = [0u8; 16];
            rarg[..8].copy_from_slice(&handle.to_le_bytes());
            rarg[8..].copy_from_slice(&want.to_le_bytes());
            match self.call(AFC_OP_FILE_READ, &rarg) {
                Ok((AFC_OP_DATA, chunk)) if !chunk.is_empty() => out.extend_from_slice(&chunk),
                _ => break,
            }
        }
        let _ = self.call(AFC_OP_FILE_CLOSE, &handle.to_le_bytes());
        Ok(out)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrashReport {
    pub kind: String,
    pub process: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_pages: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<String>,
}

/// Flushes staged crash reports (`crashreportmover`), then reads `.ips` reports in
/// `[window_start_s - 2s, window_end_s + 5s]` over read-only AFC and filters them in memory through
/// `allow` so personal apps' crashes and Jetsam rows never touch disk.
pub fn collect_crashes(dev: &Device, allow: &Allowlist, window_start_s: f64, window_end_s: f64, deadline: Option<Instant>) -> Vec<CrashReport> {
    let Ok(mut l) = session_for(dev, deadline) else { return Vec::new() };
    if let Ok(mut mover) = l.start_service("com.apple.crashreportmover") {
        mover.set_deadline(Some(Instant::now() + Duration::from_millis(1500)));
        let mut ping = [0u8; 5];
        let _ = mover.read(&mut ping);
    }
    let Ok(mut svc) = l.start_service("com.apple.crashreportcopymobile") else { return Vec::new() };
    svc.set_deadline(deadline.or_else(|| Some(Instant::now() + Duration::from_secs(4))));
    let mut afc = ReadOnlyAfc::new(svc);
    let lo_ns = ((window_start_s - 2.0).max(0.0) * 1e9) as u64;
    let hi_ns = ((window_end_s + 5.0).max(0.0) * 1e9) as u64;
    let mut reports = Vec::new();
    for dir in [".", "Retired"] {
        let Ok(entries) = afc.read_dir(dir) else { continue };
        for name in entries {
            if !name.ends_with(".ips") {
                continue;
            }
            let path = if dir == "." { name.clone() } else { format!("{dir}/{name}") };
            let Ok(info) = afc.stat(&path) else { continue };
            if info.get("st_ifmt").map(String::as_str) != Some("S_IFREG") {
                continue;
            }
            let size = info.get("st_size").and_then(|s| s.parse::<usize>().ok()).unwrap_or(usize::MAX);
            if size == 0 || size > MAX_CRASH_BYTES {
                continue;
            }
            let mtime = info.get("st_mtime").or_else(|| info.get("st_birthtime")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            if mtime < lo_ns || mtime > hi_ns {
                continue;
            }
            let Ok(raw) = afc.read_file(&path, MAX_CRASH_BYTES) else { continue };
            if let Ok(text) = std::str::from_utf8(&raw) {
                reports.extend(parse_ips(text, &name, allow));
            }
        }
    }
    reports
}

fn parse_ips(text: &str, filename: &str, allow: &Allowlist) -> Vec<CrashReport> {
    let Some((head_line, body_text)) = text.split_once('\n') else { return Vec::new() };
    let Ok(head) = serde_json::from_str::<serde_json::Value>(head_line) else { return Vec::new() };
    let Ok(body) = serde_json::from_str::<serde_json::Value>(body_text.trim()) else { return Vec::new() };
    let bug_type = head["bug_type"].as_str().unwrap_or("");
    let timestamp = head["timestamp"].as_str().or_else(|| body["captureTime"].as_str()).map(str::to_string);

    // JetsamEvent (bug_type 298): inspect `processes` in memory and keep ONLY allowlisted jettisoned entries.
    if bug_type == "298" || filename.starts_with("JetsamEvent") {
        let page_size = body["pageSize"].as_u64().unwrap_or(16384);
        let mut out = Vec::new();
        for p in body["processes"].as_array().into_iter().flatten() {
            let reason = p["reason"].as_str().unwrap_or("").trim();
            if reason.is_empty() {
                continue;
            }
            let name = p["name"].as_str().unwrap_or("");
            let pid = p["pid"].as_u64().map(|n| n as u32);
            if !allow.allows(pid, name) {
                continue;
            }
            let rpages = p["rpages"].as_u64();
            let mb = rpages.map(|r| (r * page_size) as f64 / (1024.0 * 1024.0));
            let summary = match mb {
                Some(m) => format!("jetsam killed {name} ({reason}, {m:.1} MB resident)"),
                None => format!("jetsam killed {name} ({reason})"),
            };
            out.push(CrashReport {
                kind: "jetsam".into(),
                process: name.to_string(),
                pid,
                timestamp: timestamp.clone(),
                summary,
                exception: None,
                termination: Some(reason.to_string()),
                memory_pages: rpages,
                frames: Vec::new(),
            });
        }
        return out;
    }

    let proc_name = body["procName"].as_str().or_else(|| head["app_name"].as_str()).or_else(|| head["name"].as_str()).unwrap_or("");
    let bundle_id = body["bundleInfo"]["CFBundleIdentifier"].as_str().or_else(|| head["bundleID"].as_str()).unwrap_or("");
    let pid = body["pid"].as_u64().map(|n| n as u32);
    if !allow.allows(pid, proc_name) && !allow.allows(pid, bundle_id) {
        return Vec::new();
    }
    let exc_type = body["exception"]["type"].as_str().map(str::to_string);
    let exc_sub = body["exception"]["subtype"].as_str().or_else(|| body["exception"]["codes"].as_str());
    let exception = match (&exc_type, exc_sub) {
        (Some(t), Some(s)) => Some(format!("{t} ({s})")),
        (Some(t), None) => Some(t.clone()),
        _ => None,
    };
    let termination = body["termination"]["indicator"].as_str().or_else(|| body["termination"]["namespace"].as_str()).map(str::to_string);
    let images: Vec<&serde_json::Value> = body["usedImages"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let mut frames = Vec::new();
    if let Some(ft) = body["faultingThread"].as_u64().and_then(|i| body["threads"].get(i as usize)) {
        for fr in ft["frames"].as_array().into_iter().flatten().take(15) {
            let img = fr["imageIndex"].as_u64().and_then(|i| images.get(i as usize)).and_then(|im| im["name"].as_str()).unwrap_or("?");
            if let Some(sym) = fr["symbol"].as_str() {
                frames.push(format!("{img}: {sym}"));
            } else if let Some(off) = fr["imageOffset"].as_u64() {
                frames.push(format!("{img} + 0x{off:x}"));
            }
        }
    }
    let process = if proc_name.is_empty() { bundle_id } else { proc_name }.to_string();
    let why = exception.as_deref().or(termination.as_deref()).unwrap_or("crashed");
    vec![CrashReport { kind: "crash".into(), summary: format!("{process} {why}"), process, pid, timestamp, exception, termination, memory_pages: None, frames }]
}

const SECRET_PARAMS: &[&str] = &[
    "token",
    "access_token",
    "id_token",
    "refresh_token",
    "api_key",
    "apikey",
    "key",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "pwd",
    "auth",
    "authorization",
    "code",
    "session",
    "session_id",
    "sessionid",
    "sid",
    "sig",
    "signature",
    "jwt",
    "bearer",
    "cookie",
];

fn scrub_kv_pairs(s: &str) -> String {
    s.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_secret_key(k.trim_start_matches(['/', '?'])) => format!("{k}=<redacted>"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Scrubs credentials (`user:pass@`) and secret query or `#fragment` parameters in a URL unless `reveal` is set.
pub fn scrub_url(url: &str, reveal: bool) -> String {
    if reveal {
        return url.to_string();
    }
    let (base_and_query, frag) = match url.split_once('#') {
        Some((a, f)) => (a, Some(f)),
        None => (url, None),
    };
    let (base, query) = match base_and_query.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (base_and_query, None),
    };
    let clean_base = if let Some((scheme, rest)) = base.split_once("://") {
        let host_end = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..host_end];
        if let Some((_, host)) = authority.rsplit_once('@') { format!("{scheme}://<redacted>@{host}{}", &rest[host_end..]) } else { base.to_string() }
    } else {
        base.to_string()
    };
    let mut out = clean_base;
    if let Some(q) = query {
        out.push('?');
        out.push_str(&scrub_kv_pairs(q));
    }
    if let Some(f) = frag {
        out.push('#');
        let scrubbed = match f.split_once('?') {
            Some((route, fq)) => format!("{}?{}", scrub_kv_pairs(route), scrub_kv_pairs(fq)),
            None => scrub_kv_pairs(f),
        };
        out.push_str(&redact_bearer(&scrubbed));
    }
    out
}

fn is_secret_key(k: &str) -> bool {
    let lower = k.trim().to_ascii_lowercase();
    SECRET_PARAMS.contains(&lower.as_str()) || lower.ends_with("_token") || lower.ends_with("_secret") || lower.ends_with("_key")
}

/// Scrubs `Cookie`, `Set-Cookie`, `Authorization`, `Bearer <token>`, and URL secrets inside free-form
/// log/console text while preserving Apple's `<private>`.
pub fn redact_text(s: &str, reveal: bool) -> String {
    if reveal || s.is_empty() {
        return s.to_string();
    }
    // Redact URLs embedded in log messages first.
    let mut tokens_out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("http://").or_else(|| rest.find("https://")) {
        tokens_out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let end = after.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '>' | ',' | ';')).unwrap_or(after.len());
        tokens_out.push_str(&scrub_url(&after[..end], false));
        rest = &after[end..];
    }
    tokens_out.push_str(rest);

    let mut out = redact_header_like(&tokens_out, "authorization");
    out = redact_header_like(&out, "proxy-authorization");
    out = redact_header_like(&out, "cookie");
    out = redact_header_like(&out, "set-cookie");
    out = redact_header_like(&out, "x-api-key");
    redact_bearer(&out)
}

fn redact_header_like(s: &str, header_lower: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let pat = format!("{header_lower}:");
    let Some(mut idx) = lower.find(&pat) else { return s.to_string() };
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    loop {
        // Ensure boundary before header name.
        let boundary = idx == 0 || !s.as_bytes()[idx - 1].is_ascii_alphanumeric() && s.as_bytes()[idx - 1] != b'-';
        let after_colon = idx + pat.len();
        if boundary {
            out.push_str(&s[cursor..after_colon]);
            let val = &s[after_colon..];
            let trim_ws = val.len() - val.trim_start_matches([' ', '\t']).len();
            out.push_str(&val[..trim_ws]);
            let rest_val = &val[trim_ws..];
            let end = rest_val.find(['\r', '\n', '"', '\'']).unwrap_or(rest_val.len());
            out.push_str("<redacted>");
            cursor = after_colon + trim_ws + end;
        } else {
            out.push_str(&s[cursor..after_colon]);
            cursor = after_colon;
        }
        match lower[cursor..].find(&pat) {
            Some(next) => idx = cursor + next,
            None => {
                out.push_str(&s[cursor..]);
                break;
            }
        }
    }
    out
}

fn redact_bearer(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let pat = "bearer ";
    let Some(mut idx) = lower.find(pat) else { return s.to_string() };
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    loop {
        let boundary = idx == 0 || !s.as_bytes()[idx - 1].is_ascii_alphanumeric();
        let after = idx + pat.len();
        if boundary {
            out.push_str(&s[cursor..after]);
            let tok = &s[after..];
            let len = tok.bytes().take_while(|b| b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_' | b'.' | b'=' | b'+' | b'/')).count();
            if len >= 6 {
                out.push_str("<redacted>");
                cursor = after + len;
            } else {
                cursor = after;
            }
        } else {
            out.push_str(&s[cursor..after]);
            cursor = after;
        }
        match lower[cursor..].find(pat) {
            Some(next) => idx = cursor + next,
            None => {
                out.push_str(&s[cursor..]);
                break;
            }
        }
    }
    out
}
