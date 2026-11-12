//! Perfetto trace_processor: the only place that runs it (SPEC §15).

use crate::host;
use crate::model::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

pub const VERSION: &str = "v59.1";

/// Release asset for this OS and CPU: its name, its pinned sha256, and the pinned sha256 of the
/// trace_processor_shell it holds (what `ensure` writes and checks before running it).
fn asset() -> Option<(&'static str, &'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => Some(("windows-amd64.zip", "f7ce9e3f43cb2c0c0525e79861054aa1a50a800bb88023b990d767247c80ac88", "3acc5aca1c764541033a82ba3bb0a47524b5c3c0aa194eea8d9a7b0cdb346bdc")),
        ("linux", "x86_64") => Some(("linux-amd64.zip", "7deb8c2572c6ae7ec80cd719d388041079827b0c44a1659694d5d88faa4f501e", "2434129a3af4dc9e2034a476f7b76447b490975213e301769310d3bd0d5d6458")),
        ("linux", "aarch64") => Some(("linux-arm64.zip", "8f59e2cf7197c8de388ab4765a650eaf1774a405b1753c54a9741910b6f11350", "a08e4836171c87be3ba5834eef3489aa8803476d35f292ed7feaccae252747d0")),
        ("macos", "x86_64") => Some(("mac-amd64.zip", "392cb203372f8904a138d7f5e0d5c6163b1657cb772b6036807bbb8247bcdadd", "df38039845244c353c399994748599345cf08f740491b5832e7feb9a4dbcc476")),
        ("macos", "aarch64") => Some(("mac-arm64.zip", "91b2cb63c706526a2c067c219115f9f044fa8467459868757bae15c97906037f", "8488edbefadfc4be152cda1d42d9bfcede8b47638295eb85eb3bad8cd148fdf4")),
        _ => None,
    }
}

pub const UI_ASSET: (&str, &str) = ("perfetto-ui.zip", "df4433f5d8d98d2c490249870944c819ff85206f61e55e5983bbcc2338c83904");

fn exe_name() -> &'static str {
    if cfg!(windows) { "trace_processor_shell.exe" } else { "trace_processor_shell" }
}

pub fn path() -> PathBuf {
    host::paths::engine().join(VERSION).join(exe_name())
}

/// Whether the engine is there and is the pinned build (`verified`).
pub fn installed() -> bool {
    let exe = path();
    exe.is_file() && verified(&exe)
}

/// The size and modification time of the engine file this process hashed and found the pinned build.
type Stamp = (u64, Option<std::time::SystemTime>);
static VERIFIED: std::sync::Mutex<Option<Stamp>> = std::sync::Mutex::new(None);

fn stamp(exe: &Path) -> Option<Stamp> {
    std::fs::metadata(exe).ok().map(|m| (m.len(), m.modified().ok()))
}

/// Whether `exe` is the pinned trace_processor_shell (`asset`), hashed once per process while its size
/// and modification time stay as they were then (a daemon lives for days): a file that is not (a write
/// cut short, a file something else put there) is never run, and `ensure` extracts it again.
fn verified(exe: &Path) -> bool {
    let Some(now) = stamp(exe) else { return false };
    let mut seen = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    if *seen == Some(now) {
        return true;
    }
    let ok = asset().is_some_and(|(_, _, want)| sha256(exe).is_ok_and(|got| got == want));
    // The stamp read before the hash: a file changed while it was read is hashed again next time.
    *seen = ok.then_some(now);
    ok
}

/// The pinned digest of this OS and CPU's trace_processor_shell, which `ensure` checked.
pub fn digest() -> Option<&'static str> {
    asset().map(|a| a.2)
}

/// Downloads and verifies a release asset into the engine folder; returns the archive path.
pub fn download(name: &str, sha: &str) -> Result<PathBuf> {
    let dir = host::paths::engine().join(VERSION);
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(name);
    // Another strobe may remove it between a look and a read: then it is downloaded again.
    if sha256(&archive).is_ok_and(|got| got == sha) {
        return Ok(archive);
    }
    let url = format!("https://github.com/google/perfetto/releases/download/{VERSION}/{name}");
    let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
    // Into a name of its own, checked, then renamed: another strobe downloading the same asset at
    // the same time never reads half of this one.
    let part = dir.join(format!("{name}.{}-{}.part", std::process::id(), host::paths::csprng_hex(4)?));
    let status = Command::new(curl).args(["-fsSL", "--retry", "2", "-o"]).arg(&part).arg(&url).status()?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(Error::unreachable(format!("download {url} failed")));
    }
    let got = sha256(&part)?;
    if got != sha {
        let _ = std::fs::remove_file(&part);
        return Err(Error::msg(format!("{name}: sha256 {got} does not match the pinned {sha}")));
    }
    if let Err(e) = std::fs::rename(&part, &archive) {
        let _ = std::fs::remove_file(&part);
        // Another strobe put the same archive there first (Windows refuses a rename over a file that one
        // reads): it is the pinned one, or this download fails.
        if sha256(&archive).is_ok_and(|got| got == sha) {
            return Ok(archive);
        }
        return Err(Error::msg(format!("{name}: {e}")));
    }
    Ok(archive)
}

/// One install at a time in this process: a report's views query in parallel, and each would
/// download the engine on a new device.
static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The engine, the pinned build (`verified`): downloaded and extracted when missing, extracted again
/// when the file there is not it.
pub fn ensure() -> Result<PathBuf> {
    let exe = path();
    if exe.is_file() && verified(&exe) {
        return Ok(exe);
    }
    let _one = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    if exe.is_file() && verified(&exe) {
        return Ok(exe);
    }
    let (name, sha, want) = asset().ok_or_else(|| Error::msg("no trace_processor build for this OS and CPU"))?;
    // Another strobe installing it at the same time: whatever step of this one fails, the engine it wrote
    // is as good (`verified`).
    let theirs = |e: Error| if exe.is_file() && verified(&exe) { Ok(exe.clone()) } else { Err(e) };
    let archive = match download(name, sha) {
        Ok(a) => a,
        Err(e) => return theirs(e),
    };
    let bytes = match std::fs::read(&archive) {
        Ok(b) => b,
        // It installed it and removed the archive meanwhile.
        Err(e) => return theirs(e.into()),
    };
    let body = unzip_one(&bytes, |n| n.ends_with(exe_name()))?.ok_or_else(|| Error::msg(format!("{name} has no {}", exe_name())))?;
    let got: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err(Error::msg(format!("{name}'s {}: sha256 {got} does not match the pinned {want}", exe_name())));
    }
    // Whole and runnable or not at all, over a file that was not the pinned build.
    if let Err(e) = host::paths::write_executable(&exe, &body) {
        return theirs(Error::msg(format!("write {}: {e}", exe.display())));
    }
    // The bytes just checked are the file now.
    *VERIFIED.lock().unwrap_or_else(|e| e.into_inner()) = stamp(&exe);
    let _ = std::fs::remove_file(&archive);
    Ok(exe)
}

/// The Perfetto UI of the same release (§15), unpacked once: what `strobe open` serves.
pub fn ui_dir() -> PathBuf {
    host::paths::engine().join(VERSION).join("ui")
}

pub fn ensure_ui() -> Result<PathBuf> {
    let dir = ui_dir();
    if dir.join("index.html").is_file() {
        return Ok(dir);
    }
    let _one = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    if dir.join("index.html").is_file() {
        return Ok(dir);
    }
    let archive = download(UI_ASSET.0, UI_ASSET.1)?;
    let bytes = std::fs::read(&archive)?;
    let tmp = dir.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    for (name, body) in unzip(&bytes, |n| !n.contains("..") && !n.starts_with('/'))? {
        let path = tmp.join(&name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, body)?;
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir)?;
    let _ = std::fs::remove_file(&archive);
    Ok(dir)
}

/// The UI's version directory (`v59.1-1e9c61e98`), which must match this trace_processor's build.
pub fn ui_version() -> Option<String> {
    std::fs::read_dir(ui_dir()).ok()?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).find(|n| n.starts_with(VERSION))
}

/// Starts `trace_processor_shell server http` on 127.0.0.1:`port` for the Perfetto UI served at
/// `origin`, with no trace loaded (the UI streams one to it), detached; it ends itself after an hour
/// without requests. Returns once it answers.
pub fn httpd(port: u16, origin: &str) -> Result<()> {
    let exe = ensure()?;
    let port_s = port.to_string();
    let args = ["server", "http", "--ip-address", "127.0.0.1", "--port", &port_s, "--additional-cors-origins", origin, "--idle-timeout", "60m", "--quiet"];
    #[cfg(windows)]
    host::win::spawn_detached(&exe, &args).map_err(|e| Error::msg(format!("start trace_processor: {e}")))?;
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        Command::new(&exe).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn().map_err(|e| Error::msg(format!("start trace_processor: {e}")))?;
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(100)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(Error::msg(format!("trace_processor did not answer on 127.0.0.1:{port}")))
}

fn u16le(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

fn u32le(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

/// Every file in a zip whose name matches, as (name, contents). Stored and deflate entries only.
pub fn unzip(bytes: &[u8], want: impl Fn(&str) -> bool) -> Result<Vec<(String, Vec<u8>)>> {
    let eocd = (0..bytes.len().saturating_sub(21)).rev().find(|&i| bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or_else(|| Error::msg("zip: no end of central directory"))?;
    let count = u16le(bytes, eocd + 10);
    let mut at = u32le(bytes, eocd + 16);
    let mut out = Vec::new();
    for _ in 0..count {
        if bytes.get(at..at + 4) != Some(&[0x50, 0x4b, 0x01, 0x02]) {
            return Err(Error::msg("zip: bad central directory"));
        }
        let method = u16le(bytes, at + 10);
        let csize = u32le(bytes, at + 20);
        let (nlen, elen, clen) = (u16le(bytes, at + 28), u16le(bytes, at + 30), u16le(bytes, at + 32));
        let local = u32le(bytes, at + 42);
        let name = String::from_utf8_lossy(&bytes[at + 46..at + 46 + nlen]).to_string();
        at += 46 + nlen + elen + clen;
        if name.ends_with('/') || !want(&name) {
            continue;
        }
        let data_at = local + 30 + u16le(bytes, local + 26) + u16le(bytes, local + 28);
        let data = &bytes[data_at..data_at + csize];
        let body = match method {
            0 => data.to_vec(),
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(data).read_to_end(&mut v)?;
                v
            }
            m => return Err(Error::msg(format!("zip: {name} uses compression method {m}"))),
        };
        out.push((name, body));
    }
    Ok(out)
}

fn unzip_one(bytes: &[u8], want: impl Fn(&str) -> bool) -> Result<Option<Vec<u8>>> {
    Ok(unzip(bytes, want)?.into_iter().next().map(|x| x.1))
}

fn sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// A query result: column names and rows of cells (`None` for SQL NULL).
#[derive(Debug, Default, serde::Serialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl Table {
    pub fn col(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }
}

/// How long one query may take: two minutes, plus a second per 4 MB of trace to load (a gzipped
/// trace counted as ten times its size). trace_processor loads well over that rate, so only a hung
/// one meets it.
fn deadline(trace: &Path) -> Duration {
    let mut mb = std::fs::metadata(trace).map(|m| m.len() / 1_000_000).unwrap_or(0);
    if trace.extension().is_some_and(|e| e == "gz") {
        mb *= 10;
    }
    Duration::from_secs(120 + mb / 4)
}

/// Runs SQL against a trace and returns the last statement's result. A query that outlives its
/// deadline is killed and fails (exit 124) instead of holding its caller (a report, the daemon).
pub fn query(trace: &Path, sql: &str) -> Result<Table> {
    let exe = ensure()?;
    // A counter beside the time: a report's views query in parallel, and two of them once took the same
    // name in one clock tick, so one removed the other's SQL before trace_processor read it.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = host::paths::run().join(format!("q-{}-{}-{n}.sql", std::process::id(), host::now_ns()));
    std::fs::write(&file, sql)?;
    let out = run_within(Command::new(exe).arg("-q").arg(&file).arg(trace), deadline(trace));
    let _ = std::fs::remove_file(&file);
    let out = out?.ok_or_else(|| {
        Error::timeout(format!("trace_processor did not finish a query on {} in {} s: strobe stopped it", trace.display(), deadline(trace).as_secs()))
            .hint("run it again; a query that hangs twice on one trace points at a damaged trace: record it again")
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let why = stderr.lines().filter(|l| !l.starts_with("Trace loaded") && !l.starts_with("Query executed")).collect::<Vec<_>>().join("\n");
        return Err(Error::msg(format!("trace_processor: {}", why.trim())));
    }
    Ok(parse_csv(&stdout))
}

/// Runs `cmd` to its end and returns its output, or kills it at `limit` and returns `None`. Its
/// stdout and stderr are read on their own threads, so neither pipe fills while it runs.
fn run_within(cmd: &mut Command, limit: Duration) -> Result<Option<Output>> {
    // No console window from the daemon (strobew has none of its own to share).
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(cmd, 0x0800_0000);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let err = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.as_mut().map(|p| p.read_to_end(&mut v));
        v
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.as_mut().map(|p| p.read_to_end(&mut v));
        let _ = tx.send(v);
    });
    // Its stdout closes when it exits.
    let stdout = match rx.recv_timeout(limit) {
        Ok(v) => v,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
    };
    let status = child.wait()?;
    Ok(Some(Output { status, stdout, stderr: err.join().unwrap_or_default() }))
}

fn parse_csv(text: &str) -> Table {
    // Result sets are separated by blank lines; keep the last non-empty one.
    let blocks: Vec<&str> = text.split("\n\n").map(str::trim).filter(|b| !b.is_empty()).collect();
    let Some(last) = blocks.last() else { return Table::default() };
    let mut lines = last.lines();
    let columns = lines.next().map(|l| cells(l).into_iter().map(Option::unwrap_or_default).collect()).unwrap_or_default();
    let rows = lines.map(cells).collect();
    Table { columns, rows }
}

fn cells(line: &str) -> Vec<Option<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    let mut chars = line.trim_end_matches('\r').chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => {
                quoted = !quoted;
                was_quoted = true;
            }
            ',' if !quoted => {
                out.push(cell(std::mem::take(&mut cur), was_quoted));
                was_quoted = false;
            }
            _ => cur.push(c),
        }
    }
    out.push(cell(cur, was_quoted));
    out
}

fn cell(s: String, quoted: bool) -> Option<String> {
    if s == "[NULL]" || (!quoted && s.is_empty()) { None } else { Some(s) }
}
