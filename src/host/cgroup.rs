//! Linux: each launch in a cgroup (v2) of its own, whose `cpu.stat` keeps counting what every process
//! that ran in it used, ended ones too (`usage_usec`, with the processes they started, reaped or
//! orphaned). A process that ends inside a run takes its own CPU clock with it; the cgroup's count
//! stays (`EndedMarks::dead_since`).
//!
//! Where strobe may make cgroups, found once, at its first launch: under its own cgroup when that was
//! delegated to it (the cgroup it was handed, its own up through cgroups strobes made, has systemd's
//! `user.delegate` mark, or is the cgroup of the unit it runs in and that unit has `Delegate=yes`:
//! `strobe up`'s unit does); else, under systemd, a scope of its own that systemd
//! delegates (`StartTransientUnit` with `Delegate=true` and strobe's pid, over the system bus as root,
//! the user's bus otherwise), which moves strobe out of the cgroup it was started in (never out of a
//! CPU or memory quota set on it or above it: then none); without systemd,
//! its own cgroup when it may write it. strobe then moves itself into `host/` of that cgroup (systemd
//! hands over only inner nodes) and makes `launch-<pid>-<start>-<n>` beside it per launch. A launched
//! child moves itself into its cgroup before its exec (or its spawn fails), so everything it starts
//! is in it (a program
//! that moves itself out later, as Chromium's browser process into a systemd scope of its own, is
//! outside from then). No controller is enabled: `cpu.stat`'s usage is counted in every cgroup.
//! Nowhere to make one (no cgroup v2, no systemd scope, a cgroup it may not write): launches have
//! none, and an ended process's CPU stays unknown.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const ROOT: &str = "/sys/fs/cgroup";
const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// Where this strobe makes its launches' cgroups (None: nowhere), found at the first call: strobe may
/// move into a scope of its own then.
pub(super) fn parent() -> Option<&'static Path> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(find_parent).as_deref()
}

/// This process's cgroup (the unified hierarchy's `0::` line), as a folder.
fn own() -> Option<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = text.lines().find_map(|l| l.strip_prefix("0::"))?;
    Some(Path::new(ROOT).join(rel.trim().trim_start_matches('/')))
}

fn c_path(p: &Path) -> Option<std::ffi::CString> {
    std::ffi::CString::new(p.as_os_str().as_bytes()).ok()
}

/// strobe may make cgroups in `dir` and move processes into them.
fn writable(dir: &Path) -> bool {
    let ok = |p: &Path| c_path(p).is_some_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0);
    ok(dir) && ok(&dir.join("cgroup.procs"))
}

/// The cgroup strobe's own one is in, up through the ones strobes make (`host`, `launch-…`): what
/// was handed to strobe, or to the strobe that launched it.
fn handed(own: &Path) -> &Path {
    let made = |d: &Path| d.file_name().is_some_and(|n| n == "host" || n.to_string_lossy().starts_with("launch-"));
    own.ancestors().find(|&d| !made(d)).unwrap_or(own)
}

/// systemd's mark on a cgroup it delegated (`user.delegate` = 1; a user manager marks none).
fn marked(dir: &Path) -> bool {
    let (Some(path), Ok(name)) = (c_path(dir), std::ffi::CString::new("user.delegate")) else { return false };
    let mut b = [0u8; 8];
    let n = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), b.as_mut_ptr() as *mut libc::c_void, b.len()) };
    n == 1 && b[0] == b'1'
}

/// The systemd manager strobe's units are under: the system's for root, the user's otherwise.
fn bus() -> Option<super::dbus::Conn> {
    let c = if unsafe { libc::geteuid() } == 0 { super::dbus::Conn::system() } else { super::dbus::Conn::session() }.ok()?;
    c.set_timeout(Duration::from_secs(2)).ok()?;
    Some(c)
}

/// `dir` is the cgroup of the unit strobe runs in, started with `Delegate=yes`. Not a cgroup below a
/// delegated unit's: under a user manager's `user@.service`, systemd delegated the cgroup to that
/// manager, whose units are its own.
fn delegated_unit(c: &mut super::dbus::Conn, dir: &Path) -> bool {
    use super::dbus::{Arg, Value};
    let Some(unit) = c.call(SYSTEMD, "/org/freedesktop/systemd1", MANAGER, "GetUnitByPID", &[Arg::U32(std::process::id())]).ok().and_then(|r| r.first()?.as_str().map(str::to_string)) else {
        return false;
    };
    ["org.freedesktop.systemd1.Service", "org.freedesktop.systemd1.Scope"].iter().any(|i| {
        let mut at = |name: &str| c.get_property(SYSTEMD, &unit, i, name).ok();
        at("Delegate") == Some(Value::Bool(true)) && at("ControlGroup").as_ref().and_then(Value::as_str).is_some_and(|g| Path::new(ROOT).join(g.trim_start_matches('/')) == dir)
    })
}

/// A CPU or memory quota (`cpu.max`, `memory.max` not `max`) on `own` or a cgroup above it that a scope
/// of strobe's own would leave: every one below the root cgroup for root (its scope goes to
/// `system.slice`), below the user's `user-<uid>.slice` otherwise (to its user manager). Moving out
/// would escape the limit someone set on strobe and what it launches.
fn under_quota(own: &Path) -> bool {
    let top = if unsafe { libc::geteuid() } == 0 { PathBuf::from(ROOT) } else { Path::new(ROOT).join("user.slice").join(format!("user-{}.slice", unsafe { libc::getuid() })) };
    let limited = |d: &Path| ["cpu.max", "memory.max"].iter().any(|f| std::fs::read_to_string(d.join(f)).is_ok_and(|t| t.split_whitespace().next().is_some_and(|v| v != "max")));
    own.ancestors().take_while(|d| *d != top && *d != Path::new(ROOT) && d.starts_with(ROOT)).any(limited)
}

/// A scope systemd delegates to strobe, with strobe moved into it; its folder once strobe is there.
fn scope(c: &mut super::dbus::Conn) -> Option<PathBuf> {
    use super::dbus::Arg;
    let name = format!("strobe-{}.scope", me());
    let prop = |k: &'static str, v: Arg<'static>| Arg::Struct(vec![Arg::Str(k), Arg::Var(Box::new(v))]);
    let props = vec![
        prop("Description", Arg::Str("strobe and the processes it launches")),
        prop("PIDs", Arg::Array("u", vec![Arg::U32(std::process::id())])),
        prop("Delegate", Arg::Bool(true)),
        // Removed once empty, failed or not.
        prop("CollectMode", Arg::Str("inactive-or-failed")),
    ];
    c.call(SYSTEMD, "/org/freedesktop/systemd1", MANAGER, "StartTransientUnit", &[Arg::Str(&name), Arg::Str("fail"), Arg::Array("(sv)", props), Arg::Array("(sa(sv))", Vec::new())]).ok()?;
    // systemd moves strobe when the scope's start job runs.
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let dir = own()?;
        if dir.file_name().is_some_and(|n| n.as_bytes() == name.as_bytes()) {
            return Some(dir);
        }
        if Instant::now() > until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn find_parent() -> Option<PathBuf> {
    if !Path::new(ROOT).join("cgroup.controllers").exists() {
        return None;
    }
    let own = own()?;
    let systemd = Path::new("/run/systemd/system").exists();
    let dir = if !systemd || marked(handed(&own)) {
        own
    } else {
        let mut c = bus()?;
        if delegated_unit(&mut c, handed(&own)) {
            own
        } else if under_quota(&own) {
            return None;
        } else {
            scope(&mut c)?
        }
    };
    if !writable(&dir) {
        return None;
    }
    // strobe leaves the cgroup it makes launches in for `host/` beside them (systemd hands over only
    // inner nodes), whatever happens to its launches.
    let host = dir.join("host");
    let _ = std::fs::create_dir(&host);
    std::fs::write(host.join("cgroup.procs"), "0").ok()?;
    sweep(&dir);
    if let Some(up) = dir.parent() {
        sweep(up);
    }
    Some(dir)
}

/// `<pid>-<start time>` of this strobe, as its scope's and launches' names carry it.
fn me() -> String {
    let me = std::process::id();
    format!("{me}-{}", super::start_time(me).unwrap_or(0))
}

/// Ends what strobes that are gone (killed: no drop ran) left in `dir`: their launches' cgroups
/// (`launch-<pid>-<start>-<n>`, what still runs in them killed, as a job ends with its last handle on
/// Windows), there and in their scopes (`strobe-<pid>-<start>.scope`), which systemd then removes once
/// empty. Not a scope's `host/`: what a strobe started outside its launches (its daemon) is not a
/// launch.
fn sweep(dir: &Path) {
    let gone = |owner: &str| {
        let mut f = owner.split('-').map(|x| x.parse::<u64>().ok());
        match (f.next().flatten(), f.next().flatten()) {
            (Some(pid), Some(start)) => super::start_time(pid as u32) != Some(start),
            _ => false,
        }
    };
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(owner) = name.strip_prefix("launch-")
            && gone(owner)
        {
            end(&e.path());
        } else if let Some(owner) = name.strip_prefix("strobe-").and_then(|n| n.strip_suffix(".scope"))
            && gone(owner)
        {
            for l in std::fs::read_dir(e.path()).into_iter().flatten().flatten().filter(|l| l.file_name().to_string_lossy().starts_with("launch-")) {
                end(&l.path());
            }
        }
    }
}

fn populated(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("cgroup.events")).map_or(true, |t| t.lines().any(|l| l == "populated 1"))
}

/// Kills what runs in a cgroup and below it, waits (2 s at most) for it to end, and removes the
/// cgroups, deepest first. Before Linux 5.14 (no `cgroup.kill`) each process listed is sent SIGKILL
/// until none is left (one forked meanwhile is listed in the next round).
fn end(dir: &Path) {
    let killed = std::fs::write(dir.join("cgroup.kill"), "1").is_ok();
    let until = Instant::now() + Duration::from_secs(2);
    while populated(dir) && Instant::now() < until {
        if !killed {
            // Each only while it is still in this cgroup (`host::kill`): a pid that exited since the list
            // and went to a new process names one in its parent's cgroup, not strobe's.
            for pid in pids(dir).unwrap_or_default() {
                super::kill::signal_if(pid, libc::SIGKILL, || in_cgroup(pid, dir));
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    remove(dir);
}

/// Whether process `pid` is in cgroup `dir` or one below it now (`/proc/<pid>/cgroup`'s v2 line).
fn in_cgroup(pid: u32, dir: &Path) -> bool {
    let Ok(rel) = dir.strip_prefix(ROOT) else { return false };
    let want = Path::new("/").join(rel);
    std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("0::").map(|p| Path::new(p.trim()).starts_with(&want)))).unwrap_or(false)
}

/// The processes in a cgroup and the ones below it.
fn pids(dir: &Path) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let procs = std::fs::read_to_string(dir.join("cgroup.procs")).ok()?;
        out.extend(procs.lines().filter_map(|l| l.trim().parse::<u32>().ok()));
        stack.extend(std::fs::read_dir(&dir).ok()?.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()));
    }
    Some(out)
}

fn remove(dir: &Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            remove(&e.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Launch cgroups not yet removed: the launch's root pid, the folder it was launched for
/// (`spawn::home_key`) and the cgroup.
static LAUNCHES: std::sync::Mutex<Vec<(u32, Option<String>, PathBuf)>> = std::sync::Mutex::new(Vec::new());

/// A launch's cgroup: removed when dropped, with whatever still runs in it ended.
pub struct Launch {
    path: PathBuf,
}

/// A new launch cgroup, and its `cgroup.procs` open for writing (closed on exec): a child that writes
/// `0` into it between fork and exec moves itself there.
pub fn create() -> Option<(Launch, std::fs::File)> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = parent()?.join(format!("launch-{}-{}", me(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir(&path).ok()?;
    let launch = Launch { path };
    let procs = std::fs::OpenOptions::new().write(true).open(launch.path.join("cgroup.procs")).ok()?;
    Some((launch, procs))
}

impl Launch {
    /// Lists the launch for `read` by its root `pid` and the folder it was launched for, once its child
    /// spawned: the child moved itself in before its exec, or its spawn failed (`spawn::into_cgroup`).
    /// Nothing is read back, so a launcher that handed over and exited at once keeps its cgroup.
    pub fn adopt(self, pid: u32, home: Option<&Path>) -> Launch {
        LAUNCHES.lock().unwrap_or_else(|e| e.into_inner()).push((pid, home.map(super::spawn::home_key), self.path.clone()));
        self
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        LAUNCHES.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, _, p)| *p != self.path);
        end(&self.path);
    }
}

/// The CPU of a set's launch cgroups at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The cgroups read.
    pub launches: BTreeSet<PathBuf>,
    /// The CPU of everything that ran in them (ns, from µs).
    pub usage_ns: u64,
    /// The processes running in them by pid and start time (`host::start_time`; not a zombie), each
    /// with its CPU so far (ns), read just before `usage_ns`.
    pub members: BTreeMap<(u32, u64), u64>,
}

impl Reading {
    /// The CPU of the processes that ran in the cgroups and no longer run (ns).
    pub fn ended(&self) -> u64 {
        self.usage_ns.saturating_sub(self.members.values().sum())
    }
}

/// The launch cgroups of a set (those of launches for `home`, or rooted at one of `roots` for a launch
/// for no folder, as Windows' jobs), read now; None without any, or when one could not be read.
pub fn read(roots: &[u32], home: Option<&Path>) -> Option<Reading> {
    let k = home.map(super::spawn::home_key);
    let launches: BTreeSet<PathBuf> =
        LAUNCHES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(root, h, _)| (k.is_some() && *h == k) || (h.is_none() && roots.contains(root))).map(|(_, _, p)| p.clone()).collect();
    if launches.is_empty() {
        return None;
    }
    // Every process in them (a strobe it launched may have made cgroups below), then their clocks, then
    // the cgroups' counts: what runs between the reads counts in the count alone.
    let mut all = Vec::new();
    for dir in &launches {
        all.extend(pids(dir)?);
    }
    let running: Vec<(u32, u64)> = all.into_iter().filter_map(|p| Some((p, super::start_time(p)?))).collect();
    let members = running.into_iter().filter_map(|k| Some((k, super::process_cycles(k.0)?))).collect();
    let mut usage_ns = 0u64;
    for dir in &launches {
        let stat = std::fs::read_to_string(dir.join("cpu.stat")).ok()?;
        usage_ns += stat.lines().find_map(|l| l.strip_prefix("usage_usec "))?.trim().parse::<u64>().ok()? * 1000;
    }
    Some(Reading { launches, usage_ns, members })
}
