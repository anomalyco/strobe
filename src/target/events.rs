//! The app hook stream (SPEC §9.6 "App events"): a launch gets `STROBE_EVENTS`, a path it appends
//! one JSON line per event to (`{"strobe":"state","name":"done"}`, `{"strobe":"count","name":"ops","n":42}`,
//! `{"strobe":"metric","name":"parse_ms","value":3.2}`, `{"strobe":"mark","name":"frame"}`), and strobe
//! stamps each line on its host clock as it arrives. Unix: a FIFO strobe holds open, read by a thread
//! that blocks on it (a line wakes it). Windows: a file alone in its folder, read when the folder's
//! change notification fires. No polling either way. Each line must reach the path whole and at once
//! (one append, flushed: an append-and-close, or a write and a flush). A line may carry `t_ns`, the
//! app's own reading of the host clock (Linux `CLOCK_MONOTONIC`, macOS `mach_absolute_time` in ns,
//! Windows `QueryPerformanceCounter` in ns): strobe holds its arrival within the transport's
//! delivery (`SYNC_NS`) of it, else refuses `ClockUnsynced`, and within it takes `t_ns` as the
//! event's time (the writer's, which no delivery delays).

use crate::host;
use crate::model::{Error, Refusal, Result, ValidatorId};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// The variable that names the stream's path.
pub const VAR: &str = "STROBE_EVENTS";

/// How far a line's arrival may be from the `t_ns` it carries: the transport's delivery. A FIFO
/// wakes its reader at once (1 ms); a file's folder notification on Windows took a 6.3 ms round trip
/// (a key in, the writer's work and two appends, the wait woken) on adais, so 10 ms there.
pub const SYNC_NS: u64 = if cfg!(windows) { 10_000_000 } else { 1_000_000 };

/// Events and bad lines a stream keeps at most: past them it counts what it dropped (`lost`).
const MAX_EVENTS: usize = 1_000_000;
const MAX_BAD: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// The app reached a state (`until: { state }` waits for it).
    State,
    /// Adds `n` to the run's `count.<name>`.
    Count(f64),
    /// Adds `value` to the run's `app.<name>`.
    Metric(f64),
    /// A point the app marks (its own reaction or frame begin), kept with the run's events.
    Mark,
}

#[derive(Clone, Debug)]
pub struct Event {
    /// When it happened (host clock): its `t_ns` when it carries one within `SYNC_NS` of its
    /// arrival, else its arrival.
    pub at_ns: u64,
    /// When strobe read it (host clock).
    pub arrived_ns: u64,
    pub kind: Kind,
    pub name: String,
    /// The app's own clock reading, when it gave one.
    pub t_ns: Option<u64>,
    pub line: String,
}

/// One line of the stream, or why it is none.
fn parse(line: &str, at_ns: u64) -> std::result::Result<Event, String> {
    let v: Value = serde_json::from_str(line).map_err(|_| format!("not JSON: {line}"))?;
    let name = v["name"].as_str().filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).ok_or_else(|| format!("no name (letters, digits, _ - .): {line}"))?;
    let num = |k: &str| v[k].as_f64().filter(|f| f.is_finite());
    let kind = match v["strobe"].as_str() {
        Some("state") => Kind::State,
        Some("count") => Kind::Count(if v["n"].is_null() { 1.0 } else { num("n").ok_or_else(|| format!("n is no number: {line}"))? }),
        Some("metric") => Kind::Metric(num("value").ok_or_else(|| format!("a metric needs a numeric value: {line}"))?),
        Some("mark") => Kind::Mark,
        _ => return Err(format!("\"strobe\" is state, count, metric or mark: {line}")),
    };
    let t_ns = v["t_ns"].as_u64();
    let when = t_ns.filter(|t| t.abs_diff(at_ns) <= SYNC_NS).unwrap_or(at_ns);
    Ok(Event { at_ns: when, arrived_ns: at_ns, kind, name: name.to_string(), t_ns, line: line.to_string() })
}

#[derive(Default)]
struct Inner {
    events: Vec<Event>,
    /// Lines that are no event, with why.
    bad: Vec<String>,
    /// Events and bad lines past the caps, dropped.
    lost: usize,
    partial: String,
}

struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
}

impl Shared {
    fn take(&self, bytes: &[u8]) {
        let at = host::now_ns();
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        st.partial.push_str(&String::from_utf8_lossy(bytes));
        while let Some(i) = st.partial.find('\n') {
            let line = st.partial[..i].trim_end_matches('\r').trim().to_string();
            st.partial.drain(..=i);
            if line.is_empty() {
                continue;
            }
            match parse(&line, at) {
                Ok(e) if st.events.len() < MAX_EVENTS => st.events.push(e),
                Err(why) if st.bad.len() < MAX_BAD => st.bad.push(why),
                _ => st.lost += 1,
            }
        }
        drop(st);
        self.changed.notify_all();
    }
}

/// A launch's event stream: its path (the launch's `STROBE_EVENTS`) and the reader that stamps it.
pub struct Events {
    pub path: PathBuf,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    wake: isize,
}

impl Events {
    /// A stream for a launch into `home`: `<home>/.strobe/events`, its reader on `reader_cpus` when
    /// given (as the terminal's reader).
    pub fn open(home: &Path, reader_cpus: Option<u64>) -> Result<Events> {
        let dir = home.join(".strobe");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("events");
        let _ = std::fs::remove_file(&path);
        let shared = Arc::new(Shared { inner: Mutex::new(Inner::default()), changed: Condvar::new() });
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| Error::msg("events: a path with a NUL"))?;
            if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
                return Err(Error::msg(format!("events: mkfifo {}: {}", path.display(), std::io::Error::last_os_error())));
            }
            // Read and write: open never waits for a writer, and a writer closing is no end of it.
            let fifo = std::fs::OpenOptions::new().read(true).write(true).open(&path)?;
            let (s, flag) = (shared.clone(), stop.clone());
            let reader = std::thread::Builder::new().name("strobe-events".into()).spawn(move || {
                use std::io::Read;
                if let Some(m) = reader_cpus {
                    host::set_thread_affinity(m);
                }
                let mut fifo = fifo;
                let mut buf = [0u8; 8192];
                while !flag.load(Ordering::Relaxed) {
                    match fifo.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => s.take(&buf[..n]),
                    }
                }
            })?;
            Ok(Events { path, shared, stop, reader: Some(reader) })
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification};
            use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForMultipleObjects};
            std::fs::write(&path, b"")?;
            let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let change = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE) };
            if change.is_null() || change as isize == -1 {
                return Err(Error::msg(format!("events: watch {}: {}", dir.display(), std::io::Error::last_os_error())));
            }
            // One handle for the stream's life, shared every way, so a writer's open (append, write,
            // even a delete) never collides with it.
            use std::os::windows::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new().read(true).share_mode(0x7).open(&path)?;
            let wake = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            let (s, flag, change_h, wake_h) = (shared.clone(), stop.clone(), change as isize, wake as isize);
            let reader = std::thread::Builder::new().name("strobe-events".into()).spawn(move || {
                use std::io::Read;
                if let Some(m) = reader_cpus {
                    host::set_thread_affinity(m);
                }
                loop {
                    // The handle's position stays where the last read ended.
                    let mut buf = Vec::new();
                    if file.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                        s.take(&buf);
                    }
                    if flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let handles = [change_h as _, wake_h as _];
                    let r = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    if r != 0 {
                        break;
                    }
                    unsafe { FindNextChangeNotification(change_h as _) };
                }
                unsafe { FindCloseChangeNotification(change_h as _) };
            })?;
            Ok(Events { path, shared, stop, reader: Some(reader), wake: wake as isize })
        }
    }

    /// The variable a launch gets.
    pub fn env(&self) -> (String, String) {
        (VAR.to_string(), self.path.display().to_string())
    }

    /// The first `state` named `name` that arrived at or after `after_ns`, waited for until `deadline`
    /// (woken by each line): its arrival.
    pub fn wait_state(&self, name: &str, after_ns: u64, deadline: Instant) -> Option<u64> {
        let mut st = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(e) = st.events.iter().find(|e| e.kind == Kind::State && e.name == name && e.at_ns >= after_ns) {
                return Some(e.at_ns);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            st = self.shared.changed.wait_timeout(st, left).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// The events that arrived in `[t0, t1]`.
    pub fn between(&self, t0: u64, t1: u64) -> Vec<Event> {
        self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()).events.iter().filter(|e| e.at_ns >= t0 && e.at_ns <= t1).cloned().collect()
    }

    /// Lines that were no event, with why.
    pub fn bad(&self) -> Vec<String> {
        self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()).bad.clone()
    }

    /// Events and bad lines dropped past the stream's caps.
    pub fn lost(&self) -> usize {
        self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()).lost
    }

    /// `ClockUnsynced` when an event among `events` that carries `t_ns` arrived more than `SYNC_NS`
    /// from it: the app's clock is not the host's, or the stream was late.
    pub fn sync(events: &[Event]) -> Result<()> {
        let worst = events.iter().filter_map(|e| Some((e, e.arrived_ns.abs_diff(e.t_ns?)))).max_by_key(|x| x.1);
        match worst {
            Some((e, d)) if d > SYNC_NS => Err(Refusal::new(ValidatorId::ClockUnsynced, format!("app event {:?} arrived {:.2} ms from the t_ns it carries (more than {} ms, this OS's delivery)", e.name, d as f64 / 1e6, SYNC_NS / 1_000_000))
                .fix("give t_ns from the host's monotonic clock (Linux CLOCK_MONOTONIC, macOS mach_absolute_time in ns, Windows QueryPerformanceCounter in ns), or leave it out; write each line in one append")
                .into()),
            _ => Ok(()),
        }
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        #[cfg(unix)]
        {
            // A byte wakes the reader blocked on the FIFO.
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut w) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&self.path) {
                let _ = w.write_all(b"\n");
            }
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.wake as _);
        }
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.wake as _);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
