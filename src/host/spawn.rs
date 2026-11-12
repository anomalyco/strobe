//! What strobe starts and owns. A child starts on given CPUs without moving strobe (the child has the
//! mask before it runs any code, and strobe's own threads keep theirs), in a group strobe ends whole
//! (a job object on Windows, its own process group on Unix, and on Linux a cgroup of its own that
//! also counts the CPU of what ended, `cgroup`), and stamped with the folder it was
//! launched for, which whatever it starts inherits: what left the group (a detached service, a
//! double-forked daemon) still carries the stamp.

use std::path::Path;
use std::process::{Child, Command};

/// Every process strobe launches (an isolated app, a browser, a command target) carries the folder it
/// was launched for (its home or browser profile) in this variable, and whatever it starts inherits
/// it. Read from the environment only, never the command line: a person's shell or search whose
/// arguments name a strobe folder is not strobe's.
pub const HOME_VAR: &str = "STROBE_HOME";
/// And the strobe that launched it: `<pid>:<start time>`, so a reused pid is not that strobe.
pub const OWNER_VAR: &str = "STROBE_OWNER";

/// This strobe as `OWNER_VAR` names it.
pub fn owner_stamp() -> &'static str {
    static STAMP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STAMP.get_or_init(|| format!("{}:{}", std::process::id(), super::start_time(std::process::id()).unwrap_or(0)))
}

/// A folder as a key: case and separators folded on Windows.
pub fn home_key(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) { s.to_lowercase().replace('/', "\\") } else { s }
}

/// A process's stamp: the folder it was launched for (`home_key`), and its strobe's pid and start time.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamp {
    pub home: String,
    pub pid: u32,
    pub start: u64,
}

/// `<pid>:<start time>`, as `OWNER_VAR` and a folder's owner file hold it.
pub fn parse_owner(s: &str) -> Option<(u32, u64)> {
    let (p, t) = s.trim().split_once(':')?;
    Some((p.parse().ok()?, t.parse().ok()?))
}

/// A process's stamp, read from its environment now. None both when the environment cannot be read
/// and when it carries no stamp: a caller that must tell the two apart (the second proves the process
/// is not strobe's) reads `env_of` and `stamp_in`.
pub fn stamp_of(pid: u32) -> Option<Stamp> {
    stamp_in(&super::env_of(pid)?)
}

/// The stamp an environment (`NAME=value` strings) carries, if it carries one.
pub fn stamp_in(env: &[String]) -> Option<Stamp> {
    // Windows reads variable names in any case.
    let var = |name: &str| env.iter().find_map(|e| e.split_once('=').filter(|(k, _)| if cfg!(windows) { k.eq_ignore_ascii_case(name) } else { *k == name }).map(|(_, v)| v.to_string()));
    let (owner, start) = parse_owner(&var(OWNER_VAR)?)?;
    Some(Stamp { home: home_key(Path::new(&var(HOME_VAR)?)), pid: owner, start })
}

/// A process strobe stamped, as a scan found it: its pid, parent and start time (`start_time`'s units)
/// from one process list, and the stamp read from that same process (its start time read again after
/// the stamp). With its pid and start time, what `host::kill_opened` needs to end it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamped {
    pub pid: u32,
    pub parent: u32,
    pub start: u64,
    pub stamp: Stamp,
}

/// Every running process strobe stamped, but this strobe. A process's environment is read once (a
/// process is its pid and start time), so a scan reads only the processes that started since the last
/// one. A process that shows no stamp is read again while it is under 2 s old (by its own start time):
/// one that has just started may not have its environment yet (a Windows process before its
/// parameters are written, a Unix one between fork and exec). The processes that ran before strobe
/// first looked are read once: re-reading every one of them for 2 s after the first scan had made the
/// OS sampler 1% of a core in a 5 s block on adais.
pub fn stamped() -> Vec<Stamped> {
    stamped_in(&super::process_list())
}

/// `stamped` over a process list (pid, parent, start time) already taken. A stamp is kept only when the
/// process still has the list's start time after it was read: one that exited and whose pid went to
/// another process meanwhile is read again in the next scan, never given the other's stamp.
pub(super) fn stamped_in(list: &[(u32, u32, u64)]) -> Vec<Stamped> {
    use std::collections::HashMap;
    /// Each process's stamp (or none), by its pid and start time.
    type Seen = HashMap<(u32, u64), Option<Stamp>>;
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    let me = std::process::id();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let old = seen.take().unwrap_or_default();
    let mut now = HashMap::with_capacity(old.len());
    let mut out = Vec::new();
    for &(pid, parent, start) in list {
        if pid == 0 || pid == me {
            continue;
        }
        let read = || {
            let s = stamp_of(pid);
            (super::start_time(pid) == Some(start)).then_some(s)
        };
        let entry = match old.get(&(pid, start)) {
            Some(None) if age(start).is_some_and(|a| a < std::time::Duration::from_secs(2)) => read(),
            Some(e) => Some(e.clone()),
            None => read(),
        };
        let Some(entry) = entry else { continue };
        if let Some(s) = &entry {
            out.push(Stamped { pid, parent, start, stamp: s.clone() });
        }
        now.insert((pid, start), entry);
    }
    *seen = Some(now);
    out
}

/// How long ago a process started, from its start time in `process_list`'s units: Windows' creation
/// time (100 ns since 1601), macOS's start (µs since 1970), Linux's start (clock ticks since boot).
fn age(start: u64) -> Option<std::time::Duration> {
    use std::time::Duration;
    #[cfg(windows)]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_nanos() as u64 / 100 + 116_444_736_000_000_000;
        Some(Duration::from_nanos(now.checked_sub(start)?.saturating_mul(100)))
    }
    #[cfg(target_os = "macos")]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_micros() as u64;
        Some(Duration::from_micros(now.checked_sub(start)?))
    }
    #[cfg(target_os = "linux")]
    {
        let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
        if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } != 0 {
            return None;
        }
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        let now = ts.tv_sec as u64 * hz + ts.tv_nsec as u64 * hz / 1_000_000_000;
        Some(Duration::from_secs_f64(now.checked_sub(start)? as f64 / hz as f64))
    }
}

/// The processes launched for `dir` (stamped with it), whoever launched them.
pub fn pids_in_home(dir: &Path) -> Vec<u32> {
    pids_in(&super::process_list(), dir)
}

/// `pids_in_home` over a process list already taken.
pub(super) fn pids_in(list: &[(u32, u32, u64)], dir: &Path) -> Vec<u32> {
    let k = home_key(dir);
    stamped_in(list).into_iter().filter(|s| s.stamp.home == k).map(|s| s.pid).collect()
}

/// The processes stamped with `dir` now, but this strobe and `except`: pid, start time and name.
fn stamped_with(dir: &Path, except: u32) -> Vec<(u32, u64, String)> {
    let k = home_key(dir);
    stamped().into_iter().filter(|s| s.stamp.home == k && s.pid != except).map(|s| (s.pid, s.start, name_of(s.pid))).collect()
}

/// A process's executable's file name, or nothing.
fn name_of(pid: u32) -> String {
    super::exe_of(pid).and_then(|e| e.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default()
}

/// A child strobe launched, with everything it starts as one group: on Windows a job object that
/// holds the child and all it starts, what asks to break away too (`assign`), which the OS ends when
/// strobe exits however it exits; on Unix the child's own process group,
/// which what it starts joins unless it makes its own (a daemon's `setsid`).
///
/// The child is never reaped before its group is ended (`exited` reads its status without reaping
/// it), so on Unix the group's id, the child's pid, cannot name another process when strobe ends the
/// group. Dropping it ends it all (`end`).
///
/// macOS: an app Launch Services started for strobe (`open_owned`) is owned the same way though it is
/// launchd's child: its tree with the processes responsible to it, and everything stamped with its
/// home, by its pid and start time (as `target::webkit` owns the Safari it gets started).
pub struct Owned {
    process: Proc,
    /// Taken by `end`.
    group: Option<Group>,
    /// macOS: why an app bundle's launch is not its own responsible process, so its XPC services
    /// are not in its process set (`macos::unattributed`); None everywhere else.
    pub unattributed: Option<String>,
    /// The child's start time, read at the launch: what roots its leftovers once it exited (macOS
    /// gives no information of an exited, unreaped process).
    start: Option<u64>,
    /// The child's main thread, bound at the launch (Windows: before it first ran).
    main: Result<super::MainThread, super::MainLost>,
    /// When the child was released to run (`released_ns`).
    released_ns: u64,
    #[cfg(windows)]
    started: Started,
    /// Windows: the OS creating the process (`CreateProcessW`), apart from strobe's setup of it.
    #[cfg(windows)]
    spawned: Spawned,
    /// Linux: the most of strobe's resident set the child can carry into its `ru_maxrss` (read just
    /// before the spawn, `peak_floor`).
    #[cfg(target_os = "linux")]
    floor: Option<u64>,
    /// Linux: the launch's cgroup, which counts the CPU of everything it started, ended processes too
    /// (`cgroup`); None where strobe may make none.
    #[cfg(target_os = "linux")]
    cgroup: Option<super::cgroup::Launch>,
}

/// The process an `Owned` launch started.
enum Proc {
    /// strobe's own child.
    Child(Child),
    /// macOS: an app Launch Services started (`open_owned`), launchd's child.
    #[cfg(target_os = "macos")]
    Opened { pid: u32 },
}

/// How an `Owned` child ended (`Owned::ended`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ended {
    /// It exited with this code (Windows: a process ended from outside has the code it was given).
    Code(i32),
    /// Unix: a signal ended it (its number).
    #[cfg(unix)]
    Signal(i32),
    /// It ended, and no status reached strobe: an app Launch Services started is launchd's child, and
    /// only launchd reads how it ended (macOS).
    Unreported,
}

impl Ended {
    /// Its exit code, when it exited with one strobe read: None for a signal or an unreported end
    /// (never a made-up -1).
    pub fn code(self) -> Option<i32> {
        match self {
            Ended::Code(c) => Some(c),
            _ => None,
        }
    }
}

impl std::fmt::Display for Ended {
    /// `exited (code 3)`, `ended by signal 9`, or `exited (its status went to launchd, not strobe)`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ended::Code(c) => write!(f, "exited (code {c})"),
            #[cfg(unix)]
            Ended::Signal(s) => write!(f, "ended by signal {s}"),
            Ended::Unreported => f.write_str("exited (its status went to launchd, not strobe)"),
        }
    }
}

/// Linux: what a child's `ru_maxrss` starts from. The kernel folds the resident high-water mark of the
/// address space a child had before its exec into its `ru_maxrss` (`exec_mmap`). A child forked (a
/// `pre_exec` closure makes Rust fork) has a copy of strobe's resident pages, its high-water mark set
/// to that count: strobe's resident set now, plus slack for what strobe's other threads touch before
/// the fork and the child before its exec, capped at strobe's own peak. One started with `posix_spawn`
/// (`CLONE_VM`) has strobe's own address space before its exec: strobe's own peak.
#[cfg(target_os = "linux")]
fn peak_floor(forks: bool) -> Option<u64> {
    let mut me: libc::rusage = unsafe { std::mem::zeroed() };
    let peak = (unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut me) } == 0).then_some(me.ru_maxrss as u64 * 1024)?;
    if !forks {
        return Some(peak);
    }
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    let resident = std::fs::read_to_string("/proc/self/statm").ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok());
    Some(resident.map_or(peak, |r| (r * page + (1 << 20)).min(peak)))
}

/// Windows: the cycle clock and the wall clock (as an `Instant` and on `host::now_ns`'s clock), read
/// together just before the child first runs. `run_to_exit` measures the cycle clock's rate from here
/// to the exit, a span as long as the child's life even when it exited before strobe began to wait.
#[cfg(windows)]
type Started = (Option<u64>, std::time::Instant, u64);

/// The OS creating a child (Windows' `CreateProcessW`, the `spawn` call alone): the calling thread's
/// cycles and the cycle clock just before and just after it, and its wall time. What a person's
/// launch of the command costs too, unlike strobe's job, completion port and CPU mask around it.
#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct Spawned {
    before: (u64, Option<u64>),
    after: (u64, Option<u64>),
    wall: std::time::Duration,
}

#[cfg(windows)]
impl Spawned {
    fn around<T>(f: impl FnOnce() -> T) -> (T, Spawned) {
        let before = (super::thread_cycles(), super::tsc());
        let at = std::time::Instant::now();
        let r = f();
        let wall = at.elapsed();
        (r, Spawned { before, after: (super::thread_cycles(), super::tsc()), wall })
    }
}

struct Group {
    #[cfg(windows)]
    job: Option<isize>,
    /// The group's id: the child's pid.
    #[cfg(unix)]
    pgid: u32,
}

/// One launch `end_launches` would end: its root's pid and start time, and whether the root is
/// strobe's own child (never reaped before its end, so its pid and its process group stay its own) or
/// an app launchd started for strobe (`open_owned`), reaped by launchd as it exits.
#[derive(Clone, Copy, Debug)]
struct Live {
    pid: u32,
    start: Option<u64>,
    child: bool,
}

/// Every `Owned` launch not yet ended: what `end_launches` ends on a stop. A child launch registers
/// while this is held from its spawn on, and `end` takes it out under it, so a stop and a close never
/// race over one launch.
static LIVE: std::sync::Mutex<Vec<Live>> = std::sync::Mutex::new(Vec::new());

/// A process strobe did not end, or a record it did not remove, because it could not prove it its own
/// (`host::kill`'s standard): reported at the end of the invocation (`left_alone`).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LeftAlone {
    /// The process, when there is one (a request Launch Services had not answered has none).
    pub pid: Option<u32>,
    /// What it is, in a few words (never a local path or another app's name).
    pub what: String,
    /// Why strobe did not end it.
    pub why: String,
}

impl std::fmt::Display for LeftAlone {
    /// `pid 7000, what: why`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.pid {
            Some(p) => write!(f, "pid {p}, {}: {}", self.what, self.why),
            None => write!(f, "{}: {}", self.what, self.why),
        }
    }
}

/// What strobe left alone, by the invocation it happened in (`left_scope`; 0: a thread outside every
/// invocation, such as a stop's or the daemon's own clean-up).
static LEFT: std::sync::Mutex<Vec<(u64, LeftAlone)>> = std::sync::Mutex::new(Vec::new());

thread_local! {
    /// The invocation this thread runs (`left_scope`), 0 outside one.
    static LEFT_SCOPE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// An invocation's scope for `left_alone` on this thread, from `left_scope` until dropped. Nested
/// invocations (a bisect's steps, a track's benches) share the outermost one's, which reports them.
pub struct LeftScope {
    id: u64,
    outermost: bool,
}

impl LeftScope {
    /// Whether this is the outermost invocation on its thread: the one that reports (`left_alone`).
    pub fn outermost(&self) -> bool {
        self.outermost
    }

    /// What strobe left alone in this invocation, taken: each reported once. In a process that runs one
    /// invocation (the CLI) also what was left outside any (a stop's clean-up thread); in one that runs
    /// many (the daemon, `strobe mcp`: `shared_process`) never: such a note belongs to no call, and is
    /// in the log (`left_alone_log`).
    pub fn take(&self) -> Vec<LeftAlone> {
        let shared = SHARED.load(std::sync::atomic::Ordering::Relaxed);
        let mut left = LEFT.lock().unwrap_or_else(|e| e.into_inner());
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *left).into_iter().partition(|(s, _)| *s == self.id || (*s == 0 && !shared));
        *left = rest;
        mine.into_iter().map(|(_, l)| l).collect()
    }
}

impl Drop for LeftScope {
    fn drop(&mut self) {
        if self.outermost {
            LEFT_SCOPE.with(|s| s.set(0));
            SEEN.lock().unwrap_or_else(|e| e.into_inner()).retain(|(s, _)| *s != self.id);
            // Not reported (an invocation that unwound before its report): said on stderr where strobe
            // runs one invocation, and in the log either way; not kept for another one.
            let mut left = LEFT.lock().unwrap_or_else(|e| e.into_inner());
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *left).into_iter().partition(|(s, _)| *s == self.id);
            *left = rest;
            let mine: Vec<LeftAlone> = mine.into_iter().map(|(_, l)| l).collect();
            if !mine.is_empty() && !SHARED.load(std::sync::atomic::Ordering::Relaxed) {
                eprint!("{}", left_alone_text(&mine));
            }
        }
    }
}

/// Whether this process runs many invocations at once (the daemon, `strobe mcp`): `shared_process`.
static SHARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This process serves many invocations (the daemon, `strobe mcp`): a note left outside every
/// invocation goes to the log only, never to an unrelated call's result (`LeftScope::take`).
pub fn shared_process() {
    SHARED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The log every note goes to the moment it is made (`leave`), so none is lost to a panic, an abort, a
/// second Ctrl-C or an MCP run that outlived its call: `<state>/log/left-alone.log`, a line each:
/// seconds since 1970, then the note as a run prints it (a pid and fixed words, nothing else).
pub fn left_alone_log() -> std::path::PathBuf {
    super::paths::state_dir().join("log").join("left-alone.log")
}

/// How many notes the log holds from the last `within` (`strobe status`), in it and in the file it was
/// last rotated to.
pub fn left_alone_recent(within: std::time::Duration) -> usize {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let since = now.saturating_sub(within.as_secs());
    let log = left_alone_log();
    [log.with_extension("log.old"), log]
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default().lines().filter(|l| l.split(' ').next().and_then(|s| s.parse::<u64>().ok()).is_some_and(|t| t >= since)).count())
        .sum()
}

/// Enters an invocation's scope on this thread (`app::dispatch`), or the enclosing one's.
pub fn left_scope() -> LeftScope {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    LEFT_SCOPE.with(|s| match s.get() {
        0 => {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            s.set(id);
            LeftScope { id, outermost: true }
        }
        id => LeftScope { id, outermost: false },
    })
}

/// Notes what strobe left alone, in this thread's invocation (`left_scope`).
/// A note made again in the same invocation (a clean-up that retries every few ms) is recorded once.
pub fn leave(pid: Option<u32>, what: impl Into<String>, why: impl Into<String>) {
    let scope = LEFT_SCOPE.with(|s| s.get());
    let note = LeftAlone { pid, what: what.into(), why: why.into() };
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.iter().any(|(s, n)| *s == scope && *n == note) {
            return;
        }
        seen.push((scope, note.clone()));
    }
    let log = left_alone_log();
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Rotated at 1 MB, as the helper's log is.
    if std::fs::metadata(&log).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&log, log.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        use std::io::Write as _;
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        // One write per line: strobes writing at once append whole lines.
        let _ = f.write_all(format!("{secs} {note}\n").as_bytes());
    }
    // A note outside every invocation in a process that serves many is in the log only (`take`).
    if scope != 0 || !SHARED.load(std::sync::atomic::Ordering::Relaxed) {
        LEFT.lock().unwrap_or_else(|e| e.into_inner()).push((scope, note));
    }
}

/// The notes made in each live invocation (`leave`), so none is made twice in one.
static SEEN: std::sync::Mutex<Vec<(u64, LeftAlone)>> = std::sync::Mutex::new(Vec::new());

/// The first line of `left_alone_text`, by which a quiet run finds it in a command's text.
pub const LEFT_ALONE_HEADER: &str = "strobe left alone what it could not prove its own:";

/// Everything strobe left alone and has not reported, taken: for a stop that exits the process
/// (`isolate::end_on_stop`), which reports it all.
pub fn left_alone() -> Vec<LeftAlone> {
    std::mem::take(&mut *LEFT.lock().unwrap_or_else(|e| e.into_inner())).into_iter().map(|(_, l)| l).collect()
}

/// The text a run ends with when strobe left something alone: one line per process.
pub fn left_alone_text(left: &[LeftAlone]) -> String {
    let mut s = format!("{LEFT_ALONE_HEADER}\n");
    for l in left {
        s.push_str(&format!("  {l}\n"));
    }
    s
}

/// Ends every launch this strobe still owns (each one's tree, then a child's group; Windows ends the
/// jobs itself when strobe exits): for a stop of a command that then exits (`isolate::end_on_stop`).
/// An app launchd started for strobe is ended only as far as `kill_opened` proves it the launch's,
/// and never by a process group: strobe made none. The launches stay ended: their `end` only reaps them.
pub fn end_launches() {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    for l in live.drain(..) {
        if !l.child {
            if let Some(start) = l.start {
                super::kill_opened(l.pid, start);
            }
            continue;
        }
        if reused(l.pid, l.start) {
            continue;
        }
        super::kill_launch(l.pid, l.start);
        #[cfg(unix)]
        unsafe {
            libc::kill(-(l.pid as i32), libc::SIGKILL);
        }
    }
}

/// Whether a launch's pid names another process now: one started at another time than the launch
/// read. A child strobe has not reaped keeps its pid; an app Launch Services started (macOS) is reaped
/// by launchd as it exits, and its pid may then be reused.
fn reused(pid: u32, start: Option<u64>) -> bool {
    start.is_some() && super::start_time(pid).is_some_and(|s| Some(s) != start)
}

/// Jobs by their launch's root pid and the folder it was for, when it was for one (Windows): for
/// `group_pids` (by folder) and `jobs_ended` (by folder, or by root for a launch with none, a script
/// target's host).
#[cfg(windows)]
static JOBS: std::sync::Mutex<Vec<(u32, Option<String>, isize)>> = std::sync::Mutex::new(Vec::new());

/// A job for a launch (Windows): it ends what it holds when its last handle closes (strobe exiting
/// however it exits), and its processes are held from their start (`jobs::watch`). It lets nothing
/// break away: `assign` puts the launch in a job of its own inside it that does. None where Windows
/// gave none.
#[cfg(windows)]
pub fn new_job() -> Option<isize> {
    use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let j = super::jobs::job_with(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)?;
    // Unwatched, its readings are incomplete: what ended in it stays unknown, as before.
    super::jobs::watch(j);
    Some(j)
}

/// Puts a launch's root (`process`, created suspended) in its `new_job`, then in a job inside it that
/// lets a process break away (kept with the launch's job, `jobs::prepare`). A process that asks to
/// break away (`CREATE_BREAKAWAY_FROM_JOB`) is created, so no app fails for it, and leaves that inner
/// job alone: Windows moves it up to the first job that does not allow it, the launch's, so nothing
/// the launch starts runs outside it (on adais a child started that way was in no count of a job that
/// allowed it, and in the launch's job here); there it gets an inner job of its own (`jobs::take`),
/// so one it starts may break away too. Holds the root (`jobs::hold`). Ok(false) where the launch
/// gets no job (the inner one could not be made, or the launch's job refused the root: strobe's own
/// job refused a nested one); an error where the inner job refused the root once the launch's job took
/// it (the launch's job would refuse what breaks away, so its caller ends the root).
#[cfg(windows)]
pub fn assign(job: isize, process: isize, pid: u32) -> std::io::Result<bool> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, JOB_OBJECT_LIMIT_BREAKAWAY_OK};
    let Some(inner) = super::jobs::job_with(JOB_OBJECT_LIMIT_BREAKAWAY_OK) else { return Ok(false) };
    // Known before the root joins: the port's thread gives the root no inner job of its own.
    let kept = super::jobs::prepare(job, pid, inner);
    let drop_inner = || {
        if !kept {
            unsafe { CloseHandle(inner as _) };
        }
    };
    if unsafe { AssignProcessToJobObject(job as _, process as _) } == 0 {
        drop_inner();
        return Ok(false);
    }
    if unsafe { AssignProcessToJobObject(inner as _, process as _) } == 0 {
        let e = std::io::Error::last_os_error();
        drop_inner();
        return Err(std::io::Error::other(format!("the launch's job could not take the job inside it that lets a process break away ({e})")));
    }
    drop_inner();
    super::jobs::hold(job, process, pid);
    Ok(true)
}

/// Closes a `new_job` (ending what it holds), and the handles held of its processes.
#[cfg(windows)]
pub fn close_job(job: isize) {
    super::jobs::forget(job);
    unsafe { windows_sys::Win32::Foundation::CloseHandle(job as _) };
}

/// A launch made outside `spawn_owned` (Windows: a pseudo console's program, started with
/// `CreateProcessW`): listed so a stop's `end_launches` ends it too, and its job, when it has one,
/// found by `group_pids` for `home`. Taken back by `unlist_launch` before it is ended.
#[cfg(windows)]
pub fn list_launch(pid: u32, job: Option<isize>, home: Option<&Path>) {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).push(Live { pid, start: None, child: true });
    if let Some(j) = job {
        JOBS.lock().unwrap_or_else(|e| e.into_inner()).push((pid, home.map(home_key), j));
    }
}

/// Takes a `list_launch` back off the lists (before its owner ends it and closes its job).
#[cfg(windows)]
pub fn unlist_launch(pid: u32, job: Option<isize>) {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|l| l.pid != pid);
    if let Some(j) = job {
        JOBS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, _, x)| *x != j);
    }
}

impl Owned {
    pub fn pid(&self) -> u32 {
        match &self.process {
            Proc::Child(c) => c.id(),
            #[cfg(target_os = "macos")]
            Proc::Opened { pid, .. } => *pid,
        }
    }

    /// The child, when the launch is strobe's own child.
    fn child(&mut self) -> Option<&mut Child> {
        match &mut self.process {
            Proc::Child(c) => Some(c),
            #[cfg(target_os = "macos")]
            Proc::Opened { .. } => None,
        }
    }

    /// Whether an opened app still runs: its pid with the start time read at its launch, never its pid
    /// alone (launchd reaps it as it exits, and the pid may then be another process's). One whose
    /// start time strobe never read (it exited first) is gone.
    #[cfg(target_os = "macos")]
    fn opened_alive(&self) -> bool {
        self.start.is_some_and(|s| super::start_time(self.pid()) == Some(s))
    }

    /// The pid a launch's process set is walked from, proven now (`host::kill`'s standard; every window
    /// call, sample, input and signal starts from it): strobe's own child, whose pid cannot name another
    /// process while strobe holds it unreaped (Unix) or by its handle (Windows); an opened app only while
    /// it has the start time proven at the launch; else 0, which roots nothing (the set is then only
    /// what carries the launch's stamp).
    pub fn root(&self) -> u32 {
        match &self.process {
            Proc::Child(c) => c.id(),
            #[cfg(target_os = "macos")]
            Proc::Opened { .. } => {
                if self.opened_alive() {
                    self.pid()
                } else {
                    0
                }
            }
        }
    }

    /// The root's start time, read at the launch (`start_time`'s units): with its pid, what names it.
    /// None where it was not read (Windows), or an opened app that had exited first.
    pub fn start(&self) -> Option<u64> {
        self.start
    }

    /// When the child was released to run its program, on `host::now_ns`'s clock: what a launch's
    /// time counts from (a command's `run.duration`, an app's and a terminal program's `launch.*`), so
    /// strobe's set-up of the launch is before it. Windows: right before the suspended child resumes,
    /// already in its job and on its CPUs (a child neither needs starts at once: right after its
    /// creation). Unix: right before the spawn. macOS, an app Launch Services started: right before the
    /// request (`macos::Opened::at_ns`).
    pub fn released_ns(&self) -> u64 {
        self.released_ns
    }

    /// Windows: when the child first goes input idle (`win::input_idle`): the moment the shell's "app
    /// starting" pointer would clear for a launch of it. Call it right after the launch: its watch
    /// starts then, on a thread of its own. `NotOnThisOs` elsewhere.
    pub fn input_idle(&self, deadline: std::time::Instant) -> super::Feedback {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let Proc::Child(c) = &self.process;
            super::win::input_idle(c.as_raw_handle() as isize, deadline)
        }
        #[cfg(not(windows))]
        {
            let _ = deadline;
            super::Feedback::none(super::NoFeedback::NotOnThisOs)
        }
    }

    /// Windows: the command's spawn as a person's launch of it would take it, without strobe's job,
    /// completion port and CPU mask set while it was suspended: when its run began (the moment it
    /// first ran, less the OS's creation of it, `Spawned`), the calling thread's cycles and the cycle
    /// clock before and after that creation, and its wall time (ms). None elsewhere, where the spawn
    /// call is the spawn.
    #[allow(clippy::type_complexity)]
    pub fn spawn_span(&self) -> Option<(std::time::Instant, (u64, Option<u64>), (u64, Option<u64>), f64)> {
        #[cfg(windows)]
        {
            let s = self.spawned;
            Some((self.started.1.checked_sub(s.wall).unwrap_or(self.started.1), s.before, s.after, s.wall.as_secs_f64() * 1e3))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// The child's main thread (`super::MainThread`), bound when it was launched.
    pub fn main_thread(&self) -> Result<super::MainThread, super::MainLost> {
        self.main.clone()
    }

    /// The child's output pipes, when it was started with them (an opened app has none).
    pub fn output(&mut self) -> (Option<std::process::ChildStdout>, Option<std::process::ChildStderr>) {
        match self.child() {
            Some(c) => (c.stdout.take(), c.stderr.take()),
            None => (None, None),
        }
    }

    /// The child's exit code once it has exited (-1: ended by a signal, or by no status strobe could
    /// read, `Ended::Unreported`: a caller that says how it ended uses `ended` and `Ended::code`, which
    /// never makes up a code). On Unix the child stays a zombie (`waitid` with `WNOWAIT`): its pid stays
    /// its own until `end` reaps it.
    pub fn exited(&mut self) -> Option<i32> {
        self.ended().map(|e| match e {
            Ended::Code(c) => c,
            #[cfg(unix)]
            Ended::Signal(_) => -1,
            Ended::Unreported => -1,
        })
    }

    /// How the child ended, once it has (`exited`, with the signal that ended it on Unix).
    pub fn ended(&mut self) -> Option<Ended> {
        #[cfg(target_os = "macos")]
        if matches!(self.process, Proc::Opened { .. }) {
            return (!self.opened_alive()).then_some(Ended::Unreported);
        }
        #[cfg(windows)]
        {
            self.child().and_then(|c| c.try_wait().ok().flatten()).map(|s| Ended::Code(s.code().unwrap_or(-1)))
        }
        #[cfg(unix)]
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let r = libc::waitid(libc::P_PID, self.pid() as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT);
            if r != 0 || info.si_pid() == 0 {
                return None;
            }
            // Killed or dumped: `si_status` is the signal.
            Some(if info.si_code == libc::CLD_EXITED { Ended::Code(info.si_status()) } else { Ended::Signal(info.si_status()) })
        }
    }

    /// The child's own CPU so far in `super::process_cycles` units, read through what strobe still holds
    /// of it after it exited: Windows its process handle (`QueryProcessCycleTime` answers for an exited
    /// process), Unix its unreaped zombie (`exited` keeps it). An opened app (macOS) is read only while
    /// it runs: launchd reaps it. None when it cannot be read.
    pub fn own_cycles(&self) -> Option<u64> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let Proc::Child(c) = &self.process;
            let mut cycles = 0u64;
            let ok = unsafe { windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime(c.as_raw_handle() as _, &mut cycles) } != 0;
            ok.then_some(cycles)
        }
        #[cfg(unix)]
        {
            #[cfg(target_os = "macos")]
            if matches!(self.process, Proc::Opened { .. }) && !self.opened_alive() {
                return None;
            }
            super::process_cycles(self.pid())
        }
    }

    /// Takes the launch off the list a stop ends (`end_launches`), so neither a stop nor `end` ends
    /// its tree: for a child that exited and whose leftovers were already read and ended.
    fn unlist(&self) {
        let pid = self.pid();
        LIVE.lock().unwrap_or_else(|e| e.into_inner()).retain(|l| l.pid != pid);
    }

    /// Ends the child's tree (read first: ending the group orphans it), its group, then the child,
    /// and reaps it; on Linux then whatever is left in its cgroup (what left the group, a `setsid`
    /// daemon), and the cgroup. Idempotent.
    pub fn end(&mut self) {
        self.end_group();
        #[cfg(target_os = "linux")]
        drop(self.cgroup.take());
    }

    fn end_group(&mut self) {
        let Some(group) = self.group.take() else { return };
        // Held while ending: a stop's `end_launches` either ended this launch already or waits.
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        let pid = self.pid();
        let child = self.child().is_some();
        if let Some(i) = live.iter().position(|l| l.pid == pid) {
            live.remove(i);
            if child && !reused(pid, self.start) {
                super::kill_launch(pid, self.start);
            } else if let (false, Some(start)) = (child, self.start) {
                // An opened app: what `kill_opened` proves its; a pid that names another process now is
                // left alone, and said.
                if reused(pid, self.start) {
                    leave(Some(pid), "a process given the id of an app strobe launched through Launch Services", "that app had exited, and this process started at another time: it is not strobe's");
                }
                super::kill_opened(pid, start);
            }
        }
        #[cfg(windows)]
        if let Some(job) = group.job {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            let mut jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
            jobs.retain(|(_, _, j)| *j != job);
            unsafe { TerminateJobObject(job as _, 1) };
            close_job(job);
        }
        // An opened app's group is not strobe's: never signalled.
        #[cfg(unix)]
        if self.child().is_some() {
            unsafe { libc::kill(-(group.pgid as i32), libc::SIGKILL) };
        }
        drop(live);
        // An opened app is no child to reap: launchd reaps it as it exits.
        if let Some(c) = self.child() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.end();
    }
}

/// Calls `f` with each line of a child's output, decoded lossily, until the pipe closes. A line that is
/// not UTF-8 (a legacy code page's é, binary output) is a line with replacement characters, never the
/// end of the reading: `BufRead::lines` stops at it, so the lines after it were lost (a cli command's
/// later `strobe:metric` lines) and the pipe stopped draining (a chatty app then blocks on it).
/// A line longer than `LINE_MAX` (output with no newline, a binary stream) is passed on in pieces of
/// that size, so the reader's memory stays bounded.
pub fn each_line(r: impl std::io::Read, mut f: impl FnMut(String)) {
    use std::io::{BufRead, Read};
    const LINE_MAX: u64 = 1 << 20;
    let mut r = std::io::BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.by_ref().take(LINE_MAX).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                }
                f(String::from_utf8_lossy(&buf).into_owned());
            }
        }
    }
}

/// What the OS counted for a command that ran to its exit, with what it started (SPEC §9.6 `cli()`).
/// Each field is None where this OS does not count it for processes that have ended.
#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub code: i32,
    /// When strobe saw it exit, before anything was read.
    pub exited: Option<std::time::Instant>,
    /// It ran past its deadline: strobe ended it (and everything it started), and nothing else is read.
    pub timed_out: bool,
    /// CPU time (user and kernel) of the command and what it started (ns): Unix the kernel's per-task
    /// runtime, Windows each of its job's processes from its cycle counter (`jobs`), exact, or where a
    /// process of the job was not held the command's own plus `ticks_ns`.
    pub cpu_ns: Option<u64>,
    /// Why there is no CPU time (Windows: cycles that are not time, or no job to count children in).
    pub cpu_refused: Option<String>,
    /// Windows, where a process of the job was not held: the part of `cpu_ns` the processes the
    /// command started took, from the job's clock-tick times (sampled at the timer interrupt, 15.6 ms a
    /// tick); 0 when it started none or their cycles were read.
    pub ticks_ns: u64,
    /// Windows: how many processes the command started whose CPU is `ticks_ns` (its job's count, less
    /// the command's own); 0 when their cycles were read.
    pub started: u32,
    /// Context switches (voluntary and involuntary) of its threads.
    pub switches: Option<u64>,
    /// The most memory one of its processes held at once (bytes): Unix its peak resident set
    /// (`ru_maxrss`), Windows its peak commit (`PeakProcessMemoryUsed`).
    pub peak_bytes: Option<u64>,
    /// Linux: `peak_bytes` is no larger than what the child carried from strobe before its exec
    /// (`peak_floor`): the command's own peak is at most it, not known.
    pub peak_masked: bool,
    /// Linux: that floor (bytes).
    pub peak_floor: Option<u64>,
    pub read_bytes: Option<u64>,
    pub write_bytes: Option<u64>,
    /// Why there are no IO bytes, when there are none.
    pub io_refused: Option<String>,
    /// Processes still running when the command exited (pid, name), after a short grace: strobe ended
    /// them, and what they did is not the command's whole work.
    pub left: Vec<(u32, String)>,
}

/// How long processes of the command may take to end after it exited before they count as left.
const GRACE: std::time::Duration = std::time::Duration::from_millis(100);

/// `left` with a grace: processes that end within it (a child finishing its exit) do not count. Each
/// with its start time (pid, start, name), by which it is ended (`host::kill_opened`).
fn still_left(read: impl Fn() -> Vec<(u32, u64, String)>) -> Vec<(u32, u64, String)> {
    let until = std::time::Instant::now() + GRACE;
    loop {
        let mut left = read();
        left.sort();
        left.dedup_by_key(|p| p.0);
        if left.is_empty() || std::time::Instant::now() >= until {
            return left;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Ends what a command left (`still_left`), each by its pid and the start time read with what proved it
/// the command's (its group membership or its stamp, in the same read), never by a group of its own
/// (`host::kill_opened`), and lists it in `Usage::left`. A start time of 0 marks a process of the
/// launch's Windows job, which the job's end ends (`TerminateJobObject`), never by its pid.
fn end_left(left: Vec<(u32, u64, String)>) -> Vec<(u32, String)> {
    for &(p, s, _) in left.iter().filter(|l| l.1 != 0) {
        super::kill_opened(p, s);
    }
    left.into_iter().map(|(p, _, n)| (p, n)).collect()
}

impl Owned {
    /// Waits for the child to exit (at most `deadline`: then it and everything it started end, and
    /// `timed_out`), reads what the OS counted for it and everything it started, ends what is left of
    /// its group and of the processes stamped with `stamp` (`HOME_VAR`: what left the group, a
    /// `setsid` daemon or a job breakaway), and reaps it. On Unix the child stays a zombie while it is
    /// read (its pid, the group's id, is not reused), and only processes its tree waited for count: one
    /// it detached that ended before it is in no count. `on_exit` runs as soon as the child has exited
    /// (or its deadline ended it), before any of that is read. A launch with no home (a `cli()`
    /// command's) is not its own responsible process on macOS, so an XPC service launchd starts for it
    /// is neither in its group nor stamped: in no count, and not ended.
    pub fn run_to_exit(mut self, deadline: std::time::Duration, stamp: Option<&Path>, on_exit: impl FnOnce()) -> std::io::Result<Usage> {
        let stamped = |me: u32| -> Vec<(u32, u64, String)> { stamp.map(|h| stamped_with(h, me)).unwrap_or_default() };
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::{
                JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAndIoAccountingInformation, JobObjectExtendedLimitInformation,
                QueryInformationJobObject,
            };
            use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, WaitForSingleObject};
            use windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime;
            let (t0, w0, _) = self.started;
            let Proc::Child(child) = &self.process;
            let h = child.as_raw_handle();
            let ms = deadline.as_millis().min(u32::MAX as u128 - 1) as u32;
            let waited = unsafe { WaitForSingleObject(h as _, ms) };
            // Both clocks together, before anything else runs: the rate below divides one by the other.
            let (exited, t1) = (std::time::Instant::now(), super::tsc());
            on_exit();
            if waited != 0 {
                self.end();
                return Ok(Usage { timed_out: true, ..Usage::default() });
            }
            let wall_ns = exited.duration_since(w0).as_nanos() as u64;
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(h as _, &mut code) };
            let job = self.group.as_ref().and_then(|g| g.job);
            let mut usage = Usage { code: code as i32, exited: Some(exited), ..Usage::default() };
            // The child's own CPU from its cycle counter (exact); what it started from the cycles of
            // the job's processes strobe held from their start (`jobs`), or where one was not held from
            // the job's clock-tick accounting.
            let (mut cycles, mut child_100ns) = (0u64, None);
            unsafe {
                QueryProcessCycleTime(h as _, &mut cycles);
                let (mut c, mut e, mut k, mut u) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
                if GetProcessTimes(h as _, &mut c, &mut e, &mut k, &mut u) != 0 {
                    let t = |f: windows_sys::Win32::Foundation::FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
                    child_100ns = Some(t(k) + t(u));
                }
            }
            // The cycle clock's rate over the child's life (cycles are its units; None where they are
            // not time), both ends read with the wall clock.
            let hz = t0.zip(t1).filter(|(a, b)| b > a && wall_ns > 0).map(|(a, b)| (b - a) as f64 / (wall_ns as f64 / 1e9));
            let child_ns = hz.filter(|_| cycles > 0).map(|hz| (cycles as f64 / hz * 1e9) as u64);
            if child_ns.is_none() {
                usage.cpu_refused = Some(super::cycles_refused().unwrap_or_else(|| "the command's process gave no cycles over the cycle clock".into()));
            }
            let left = match job {
                Some(j) => {
                    let mut acct: JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
                    let ok = unsafe {
                        QueryInformationJobObject(j as _, JobObjectBasicAndIoAccountingInformation, &mut acct as *mut _ as *mut _, std::mem::size_of_val(&acct) as u32, std::ptr::null_mut())
                    } != 0;
                    // Every process of the job held: their cycles (the command's own among them, and one
                    // still running, which is ended below, as far as it got) over the same rate.
                    let held = super::jobs::read(&[j]);
                    let all: u64 = held.ended.values().chain(held.running.values()).sum();
                    if ok && held.complete && child_ns.is_some() {
                        usage.cpu_ns = hz.map(|hz| (all as f64 / hz * 1e9) as u64);
                    } else if ok {
                        let job_100ns = (acct.BasicInfo.TotalUserTime + acct.BasicInfo.TotalKernelTime).max(0) as u64;
                        usage.started = acct.BasicInfo.TotalProcesses.saturating_sub(1);
                        usage.ticks_ns = if acct.BasicInfo.TotalProcesses > 1 { job_100ns.saturating_sub(child_100ns.unwrap_or(0)) * 100 } else { 0 };
                        usage.cpu_ns = child_ns.map(|c| c + usage.ticks_ns);
                    }
                    if ok {
                        usage.read_bytes = Some(acct.IoInfo.ReadTransferCount);
                        usage.write_bytes = Some(acct.IoInfo.WriteTransferCount);
                    } else {
                        usage.cpu_refused = Some("Windows: the job holding the command and what it started could not be read".into());
                        usage.io_refused = Some("Windows: the job holding the command and what it started could not be read".into());
                    }
                    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
                    if unsafe { QueryInformationJobObject(j as _, JobObjectExtendedLimitInformation, &mut limits as *mut _ as *mut _, std::mem::size_of_val(&limits) as u32, std::ptr::null_mut()) }
                        != 0
                    {
                        usage.peak_bytes = Some(limits.PeakProcessMemoryUsed as u64);
                    }
                    // Ended with the job (self.end), not by pid: a start time of 0 (nd_left).
                    let pids = || job_pids(j).into_iter().map(|p| (p, 0, name_of(p))).collect::<Vec<_>>();
                    still_left(|| pids().into_iter().chain(stamped(self.pid())).collect())
                }
                None => {
                    // No job (strobe's own job refused a nested one): nothing counts what the command started.
                    usage.cpu_refused = Some("Windows: the job strobe runs in refused a job for the command, so what it started is in no count".into());
                    usage.io_refused = usage.cpu_refused.clone();
                    still_left(|| stamped(self.pid()))
                }
            };
            usage.left = end_left(left);
            // What it left was read and ended above, and an exited process is in no snapshot, so
            // ending its tree again (a snapshot of every process, 5 ms on adais) finds nothing.
            self.unlist();
            self.end();
            Ok(usage)
        }
        #[cfg(unix)]
        unsafe {
            if self.child().is_none() {
                return Err(std::io::Error::other("an app Launch Services started is not strobe's child: strobe cannot wait for its exit"));
            }
            let pid = self.pid();
            // The deadline: a watchdog ends the group (the child is not reaped, so the id is still its).
            let (stop, stopped) = std::sync::mpsc::channel::<()>();
            let timed_out = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = timed_out.clone();
            let watchdog = std::thread::spawn(move || {
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(deadline) {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            });
            let mut info: libc::siginfo_t = std::mem::zeroed();
            // Exited, not reaped: a zombie keeps its pid and the group's id while it is read.
            while libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) != 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::EINTR) {
                    // Not waitable (ECHILD: reaped already): its pid and group id may name another
                    // process now, so the watchdog is joined before anything else and the group is
                    // forgotten, and nothing signals it after this.
                    let _ = stop.send(());
                    let _ = watchdog.join();
                    self.unlist();
                    self.group = None;
                    return Err(e);
                }
            }
            let exited = std::time::Instant::now();
            on_exit();
            let _ = stop.send(());
            let _ = watchdog.join();
            if timed_out.load(std::sync::atomic::Ordering::SeqCst) {
                end_left(stamped(pid));
                self.end();
                return Ok(Usage { timed_out: true, ..Usage::default() });
            }
            let mut usage = Usage { code: if info.si_code == libc::CLD_EXITED { info.si_status() } else { -1 }, exited: Some(exited), ..Usage::default() };
            #[cfg(target_os = "linux")]
            {
                // The zombie's IO: its own and what its children (reaped by it) did.
                match std::fs::read_to_string(format!("/proc/{pid}/io")) {
                    Ok(io) => {
                        let field = |k: &str| io.lines().find_map(|l| l.strip_prefix(k)).and_then(|v| v.trim().parse::<u64>().ok());
                        usage.read_bytes = field("rchar:");
                        usage.write_bytes = field("wchar:");
                    }
                    Err(e) => usage.io_refused = Some(format!("Linux: /proc/{pid}/io of the ended command could not be read ({e})")),
                }
            }
            #[cfg(target_os = "macos")]
            {
                // Disk bytes of the process alone: its reaped children's are in no count, so with
                // children (their CPU time shows) IO is unknown.
                let mut ri: libc::rusage_info_v4 = std::mem::zeroed();
                if libc::proc_pid_rusage(pid as i32, libc::RUSAGE_INFO_V4, &mut ri as *mut _ as *mut libc::rusage_info_t) != 0 {
                    usage.io_refused = Some("macOS: the ended command's rusage could not be read".into());
                } else if ri.ri_child_user_time + ri.ri_child_system_time > 0 {
                    usage.io_refused = Some("macOS keeps no disk bytes of the processes a command started and reaped, and this one started some".into());
                } else {
                    usage.read_bytes = Some(ri.ri_diskio_bytesread);
                    usage.write_bytes = Some(ri.ri_diskio_byteswritten);
                }
            }
            let left = still_left(|| group_members(pid).into_iter().filter(|(p, _, _)| *p != pid).chain(stamped(pid)).collect());
            if !left.is_empty() {
                // The child's own group: it is not reaped, so the group's id is still its.
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            usage.left = end_left(left);
            // Off the list a stop ends before the reap: its pid may name another process after it.
            self.unlist();
            let mut status = 0;
            let mut ru: libc::rusage = std::mem::zeroed();
            while libc::wait4(pid as libc::pid_t, &mut status, 0, &mut ru) < 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::EINTR) {
                    // Not ours to reap (ECHILD: reaped already), so its pid and group id may name
                    // another process now: the drop must not end the group.
                    self.group = None;
                    return Err(e);
                }
            }
            // Reaped here: nothing is left to end, and the group's id may name another process now.
            self.group = None;
            let tv = |t: libc::timeval| t.tv_sec as u64 * 1_000_000_000 + t.tv_usec as u64 * 1_000;
            usage.cpu_ns = Some(tv(ru.ru_utime) + tv(ru.ru_stime));
            usage.switches = Some(ru.ru_nvcsw as u64 + ru.ru_nivcsw as u64);
            // Linux counts kilobytes, macOS bytes.
            usage.peak_bytes = Some(if cfg!(target_os = "macos") { ru.ru_maxrss as u64 } else { ru.ru_maxrss as u64 * 1024 });
            // Linux carries the resident set the child had before its exec (`peak_floor`, read before
            // the spawn) into its `ru_maxrss`: a peak no larger than it is at most the command's.
            #[cfg(target_os = "linux")]
            {
                usage.peak_floor = self.floor;
                usage.peak_masked = usage.peak_bytes.zip(self.floor).is_none_or(|(p, f)| p <= f);
            }
            Ok(usage)
        }
    }
}

/// The live processes in process group `pgid` now (pid, name): zombies and dead tasks, which have
/// ended and only wait to be reaped, are not.
#[cfg(unix)]
fn group_members(pgid: u32) -> Vec<(u32, u64, String)> {
    #[cfg(target_os = "linux")]
    {
        let mut out = Vec::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
            // `pid (comm) state ppid pgrp … starttime`: the name may hold spaces and parentheses. The
            // group and the start time come from this one read: of one process.
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
            let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
            let state = rest.first().copied().unwrap_or("");
            let pgrp = rest.get(2).and_then(|s| s.parse::<u32>().ok());
            let start = rest.get(19).and_then(|s| s.parse::<u64>().ok());
            if let (Some(start), true) = (start, pgrp == Some(pgid) && !matches!(state, "Z" | "X" | "x")) {
                out.push((pid, start, stat[open + 1..close].to_string()));
            }
        }
        out
    }
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0i32; 4096];
        // PROC_PGRP_ONLY (2), which the libc crate does not name.
        let n = unsafe { libc::proc_listpids(2, pgid, buf.as_mut_ptr() as *mut libc::c_void, (buf.len() * 4) as i32) };
        buf.truncate((n.max(0) as usize / 4).min(buf.len()));
        // Its group, state and start time from one read (a pid listed and reused since is in another
        // group, or started later): SZOMB (5) ended, not reaped.
        let member = |pid: i32| -> Option<u64> {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
            let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
            (got == size && info.pbi_pgid == pgid && info.pbi_status != 5).then_some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
        };
        buf.into_iter()
            .filter(|&p| p > 0)
            .filter_map(|p| Some((p as u32, member(p)?, super::exe_of(p as u32).and_then(|e| e.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default())))
            .collect()
    }
}
/// The processes of the groups launched for `dir` and still owned (Windows' jobs; none on Unix: the
/// tree and the stamp find them there).
pub fn group_pids(dir: &Path) -> Vec<u32> {
    #[cfg(windows)]
    {
        let k = home_key(dir);
        // Read under the lock: `Owned::end` closes a job only after taking it out of the list.
        let jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter().filter(|(_, h, _)| h.as_ref() == Some(&k)).flat_map(|(_, _, j)| job_pids(*j)).collect()
    }
    #[cfg(unix)]
    {
        let _ = dir;
        Vec::new()
    }
}

/// How many processes have ended in the jobs launched for `dir`, or (a launch for no folder, a script
/// target's host) rooted at one of `roots` (Windows' job accounting counts every process ever assigned,
/// `TotalProcesses`, beside those still in it); None without such a job.
#[cfg(windows)]
pub fn jobs_ended(roots: &[u32], dir: Option<&Path>) -> Option<u64> {
    use windows_sys::Win32::System::JobObjects::{JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation, QueryInformationJobObject};
    let k = dir.map(home_key);
    // Read under the lock: `Owned::end` closes a job only after taking it out of the list.
    let jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Option<u64> = None;
    for (_, _, j) in jobs.iter().filter(|(root, h, _)| of_set(roots, &k, *root, h)) {
        let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { QueryInformationJobObject(*j as _, JobObjectBasicAccountingInformation, &mut a as *mut _ as *mut _, std::mem::size_of_val(&a) as u32, std::ptr::null_mut()) } != 0;
        if ok {
            *out.get_or_insert(0) += a.TotalProcesses.saturating_sub(a.ActiveProcesses) as u64;
        }
    }
    out
}

/// A job listed by its launch's root and folder (`JOBS`) is one of the set's: launched for `dir`, or
/// for no folder and rooted at one of `roots`.
#[cfg(windows)]
fn of_set(roots: &[u32], dir: &Option<String>, root: u32, home: &Option<String>) -> bool {
    (dir.is_some() && home == dir) || (home.is_none() && roots.contains(&root))
}

/// The processes of the jobs `jobs_ended` counts, held from their start (`jobs::read`), with what
/// the ended ones used; None without such a job.
#[cfg(windows)]
pub fn jobs_cpu(roots: &[u32], dir: Option<&Path>) -> Option<super::jobs::Reading> {
    let k = dir.map(home_key);
    // Read under the lock: `Owned::end` forgets and closes a job only after taking it out of the list.
    let jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let set: Vec<isize> = jobs.iter().filter(|(root, h, _)| of_set(roots, &k, *root, h)).map(|(_, _, j)| *j).collect();
    (!set.is_empty()).then(|| super::jobs::read(&set))
}

/// Spawns `cmd` restricted to the CPUs in `cpus` (None: it inherits strobe's mask, as any child does).
/// strobe's own affinity never changes. macOS has no CPU affinity: there the child runs anywhere.
/// `flags` are Windows process creation flags (`CREATE_NO_WINDOW`, …), ignored elsewhere: a `Command`
/// keeps only the last flags set on it, so they go through here rather than on `cmd`.
pub fn spawn_pinned(cmd: &mut Command, cpus: Option<u64>, flags: u32) -> std::io::Result<Child> {
    #[cfg(windows)]
    {
        windows(cmd, cpus, flags, None).map(|(child, ..)| child)
    }
    #[cfg(not(windows))]
    {
        let _ = flags;
        let _ = pin(cmd, cpus);
        cmd.spawn()
    }
}

/// `spawn_pinned` as an `Owned` launch, which strobe ends whole when it is done with it; `home` is the
/// folder the launch is for (its stamp, set by the caller).
pub fn spawn_owned(cmd: &mut Command, cpus: Option<u64>, flags: u32, home: Option<&Path>) -> std::io::Result<Owned> {
    // Held from before the spawn until the launch is listed: a stop in between ends it too.
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let owned = spawn_group(cmd, cpus, flags, home)?;
    live.push(Live { pid: owned.pid(), start: owned.start, child: true });
    Ok(owned)
}

fn spawn_group(cmd: &mut Command, cpus: Option<u64>, flags: u32, home: Option<&Path>) -> std::io::Result<Owned> {
    #[cfg(windows)]
    {
        let job = new_job();
        let (child, assigned, started, spawned, first) = match windows(cmd, cpus, flags, job) {
            Ok(x) => x,
            Err(e) => {
                if let Some(j) = job {
                    close_job(j);
                }
                return Err(e);
            }
        };
        // strobe's own job may refuse a nested one: the stamp and the tree still find everything.
        let job = match (job, assigned) {
            (Some(j), false) => {
                close_job(j);
                None
            }
            (j, _) => j,
        };
        if let Some(j) = job {
            JOBS.lock().unwrap_or_else(|e| e.into_inner()).push((child.id(), home.map(home_key), j));
        }
        let main = match first {
            Some(t) => Ok(super::MainThread::of_handle(child.id(), t)),
            None => super::MainThread::attach(child.id()),
        };
        Ok(Owned { process: Proc::Child(child), group: Some(Group { job }), unattributed: None, start: None, main, released_ns: started.2, started, spawned })
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        let _ = flags;
        cmd.process_group(0);
        let _forks = pin(cmd, cpus);
        // A target (a launch for a home) whose program is an app bundle's starts as its own
        // responsible process, as a person starts an app, so its XPC services join its process set.
        #[cfg(target_os = "macos")]
        let bundle = home.is_some() && super::macos::own_responsibility(cmd);
        #[cfg(target_os = "linux")]
        let cgroup = into_cgroup(cmd);
        // Read just before the spawn: strobe's reader threads' buffers grow after it. The cgroup's move
        // is a `pre_exec` closure too: Rust forks then.
        #[cfg(target_os = "linux")]
        let floor = peak_floor(_forks || cgroup.is_some());
        let released_ns = super::now_ns();
        let child = cmd.spawn()?;
        let pgid = child.id();
        // `spawn` returns once the exec is done: whether the disclaim took is known now.
        #[cfg(target_os = "macos")]
        let unattributed = if bundle { super::macos::unattributed(pgid) } else { None };
        #[cfg(not(target_os = "macos"))]
        let unattributed = None;
        Ok(Owned {
            process: Proc::Child(child),
            group: Some(Group { pgid }),
            unattributed,
            start: super::start_time(pgid),
            main: super::MainThread::attach(pgid),
            released_ns,
            #[cfg(target_os = "linux")]
            floor,
            #[cfg(target_os = "linux")]
            cgroup: cgroup.map(|(c, _)| c.adopt(pgid, home)),
        })
    }
}

/// Linux: a launch cgroup the child moves itself into between fork and exec (`cgroup::create`), with
/// what it moves through, held open until the spawn returns.
#[cfg(target_os = "linux")]
fn into_cgroup(cmd: &mut Command) -> Option<(super::cgroup::Launch, std::fs::File)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let (launch, procs) = super::cgroup::create()?;
    let fd = procs.as_raw_fd();
    // Between fork and exec only async-signal-safe calls. A move that fails fails the spawn: a child
    // outside its launch's cgroup would be counted wrong.
    unsafe {
        cmd.pre_exec(move || {
            if libc::write(fd, b"0".as_ptr() as *const libc::c_void, 1) != 1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Some((launch, procs))
}

/// `spawn_owned` for a program on a pseudo-terminal (Unix): the child starts a session of its own
/// (`setsid`, which also makes it its process group, the group `Owned` ends) whose controlling
/// terminal is its stdin, as a terminal emulator starts a shell. `cmd`'s stdin must be the terminal.
/// `home` is the folder the launch is for, as `spawn_owned`'s.
#[cfg(unix)]
pub fn spawn_owned_terminal(cmd: &mut Command, cpus: Option<u64>, home: Option<&Path>) -> std::io::Result<Owned> {
    use std::os::unix::process::CommandExt;
    // Held from before the spawn until the launch is listed: a stop in between ends it too.
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let _ = pin(cmd, cpus);
    // Between fork and exec only async-signal-safe calls.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    #[cfg(target_os = "linux")]
    let cgroup = into_cgroup(cmd);
    #[cfg(not(target_os = "linux"))]
    let _ = home;
    #[cfg(target_os = "linux")]
    let floor = peak_floor(true);
    let released_ns = super::now_ns();
    let child = cmd.spawn()?;
    let pgid = child.id();
    let start = super::start_time(pgid);
    live.push(Live { pid: pgid, start, child: true });
    Ok(Owned {
        process: Proc::Child(child),
        group: Some(Group { pgid }),
        unattributed: None,
        start,
        main: super::MainThread::attach(pgid),
        released_ns,
        #[cfg(target_os = "linux")]
        floor,
        #[cfg(target_os = "linux")]
        cgroup: cgroup.map(|(c, _)| c.adopt(pgid, home)),
    })
}

/// Starts the app `bundle` through Launch Services (`macos::open_app`), as a person's launch from the
/// Dock or Finder starts it, and owns it as an `Owned` launch: by its pid and the start time read once
/// the pid was known (`Opened::start`), with its tree, the processes responsible to it and everything
/// stamped with `home` (which `env` must stamp: `isolate::launch_env`); never by a process group, which
/// strobe did not make. The wait for the launch holds no lock (a stop ends it at once, `Stopped`, and
/// `isolate::end_on_stop`'s sweep of `home` ends what started late); the launch is listed for a stop's
/// `end_launches` once its pid is known. A process Launch Services names that strobe cannot prove its
/// launch's (`macos::launched`) fails the launch (`OpenFailed::Foreign`) and is never touched; one that
/// had exited before it was proven is not adopted: the launch's pid is 0, so its process set is only
/// what carries `home`'s stamp, and nothing is ever done by the pid Launch Services gave. A stop during
/// the wait ends the launch once it surfaces (`open_app`, 2 s at most). It has no output pipes, no exit
/// status (`Ended::Unreported`) and, once gone, no CPU of its own to read. Returns the launch clock read
/// just before the request (`Opened::at_ns`, `epoch_ms`).
#[cfg(target_os = "macos")]
pub fn open_owned(bundle: &super::macos::Bundle, args: &[String], env: &[(String, String)], home: &Path, activate: bool) -> Result<(Owned, super::macos::Opened), super::macos::OpenFailed> {
    let opened = super::macos::open_app(bundle, args, env, activate, Some(home), std::time::Duration::from_secs(30))?;
    let start = opened.start;
    let pid = if start.is_some() { opened.pid } else { 0 };
    match start {
        Some(start) => LIVE.lock().unwrap_or_else(|e| e.into_inner()).push(Live { pid, start: Some(start), child: false }),
        None => leave(
            Some(opened.pid),
            "the process Launch Services answered strobe's launch with",
            format!("not adopted ({}): strobe tracks only what carries the launch's stamp", opened.unproven.unwrap_or("no start time")),
        ),
    }
    let owned = Owned {
        process: Proc::Opened { pid },
        // A group of no process of strobe's: never signalled (`end_group`); with no start time, nothing
        // of it is ended at all.
        group: start.map(|_| Group { pgid: pid }),
        unattributed: start.and_then(|_| super::macos::unattributed(pid)),
        start,
        main: match start {
            Some(_) => super::MainThread::attach(pid),
            None => Err(super::MainLost::Gone("the app exited at once".into())),
        },
        released_ns: opened.at_ns,
    };
    Ok((owned, opened))
}
/// Linux: the CPU mask set between fork and exec. macOS has no CPU affinity. Whether it added a
/// `pre_exec` closure (Rust then forks rather than `posix_spawn`s).
#[cfg(not(windows))]
fn pin(cmd: &mut Command, cpus: Option<u64>) -> bool {
    #[cfg(target_os = "linux")]
    if let Some(mask) = cpus {
        use std::os::unix::process::CommandExt;
        // Every CPU the machine has, not strobe's own set: strobe may sit on the other CPUs (a
        // bench keeps it on the runner's), and a count from available_parallelism names the
        // lowest CPUs whatever strobe's set is.
        let configured = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) }.clamp(1, 64) as u32;
        let all = if configured >= 64 { u64::MAX } else { (1u64 << configured) - 1 };
        let target = if mask == 0 { all } else { mask & all };
        if target != 0 {
            let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
            for c in (0..64).filter(|c| target >> c & 1 == 1) {
                unsafe { libc::CPU_SET(c, &mut set) };
            }
            // Between fork and exec only async-signal-safe calls: the set is built before.
            unsafe {
                cmd.pre_exec(move || {
                    libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
                    Ok(())
                });
            }
            return true;
        }
    }
    #[cfg(target_os = "macos")]
    let _ = (cmd, cpus);
    false
}

/// The processes in a job now (`JobObjectBasicProcessIdList`).
#[cfg(windows)]
fn job_pids(job: isize) -> Vec<u32> {
    use windows_sys::Win32::System::JobObjects::{JobObjectBasicProcessIdList, QueryInformationJobObject};
    // JOBOBJECT_BASIC_PROCESS_ID_LIST: two u32 counts (assigned, listed), then the ids, pointer-sized.
    let head = 8 / size_of::<usize>();
    let mut n = 64usize;
    loop {
        let mut buf = vec![0usize; head + n];
        let ok = unsafe { QueryInformationJobObject(job as _, JobObjectBasicProcessIdList, buf.as_mut_ptr() as *mut _, (buf.len() * size_of::<usize>()) as u32, std::ptr::null_mut()) } != 0;
        let counts = buf.as_ptr() as *const u32;
        let (assigned, listed) = unsafe { (*counts as usize, *counts.add(1) as usize) };
        if ok {
            return buf[head..head + listed.min(n)].iter().map(|p| *p as u32).collect();
        }
        // Too small: the call still says how many there are.
        if assigned <= n || n >= 65536 {
            return Vec::new();
        }
        n = assigned + 16;
    }
}

/// ntdll's `NtResumeProcess`: resumes every thread of a process.
#[cfg(windows)]
fn nt_resume() -> Option<unsafe extern "system" fn(*mut std::ffi::c_void) -> i32> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Resume = unsafe extern "system" fn(*mut std::ffi::c_void) -> i32;
    static F: std::sync::OnceLock<Option<Resume>> = std::sync::OnceLock::new();
    *F.get_or_init(|| {
        let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        let f = unsafe { GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), c"NtResumeProcess".as_ptr() as *const u8) }?;
        Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, Resume>(f) })
    })
}

/// Created suspended, put in `job` and given the mask, then resumed: its first instruction already
/// runs there. Returns whether the job took it, the clocks just before it ran, the OS's creation
/// of it (`Spawned`), and its first thread, held open before it ran (its main thread). A child with
/// neither runs from its creation: its clocks are read right after it, and it has no thread held.
#[cfg(windows)]
fn windows(cmd: &mut Command, cpus: Option<u64>, flags: u32, job: Option<isize>) -> std::io::Result<(Child, bool, Started, Spawned, Option<super::win::ThreadHandle>)> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows_sys::Win32::System::Threading::{CREATE_SUSPENDED, GetProcessAffinityMask, OpenThread, ResumeThread, SetProcessAffinityMask, THREAD_SUSPEND_RESUME};
    let clocks = || (super::tsc(), std::time::Instant::now(), super::now_ns());
    if cpus.is_none() && job.is_none() {
        let (child, spawned) = Spawned::around(|| cmd.creation_flags(flags).spawn());
        return Ok((child?, false, clocks(), spawned, None));
    }
    let (child, spawned) = Spawned::around(|| cmd.creation_flags(flags | CREATE_SUSPENDED).spawn());
    let mut child = child?;
    let h = child.as_raw_handle();
    unsafe {
        let assigned = match job.map(|j| assign(j, h as isize, child.id())) {
            Some(Err(e)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            a => a.is_some_and(|a| a.unwrap_or(false)),
        };
        if let Some(mask) = cpus {
            let (mut proc_mask, mut sys_mask) = (0usize, 0usize);
            if GetProcessAffinityMask(h, &mut proc_mask, &mut sys_mask) != 0 {
                let target = if mask == 0 { sys_mask } else { mask as usize & sys_mask };
                if target != 0 {
                    SetProcessAffinityMask(h, target);
                }
            }
        }
        // Its one thread, the main thread, held before it runs: no thread of its own can be mistaken
        // for it.
        let first = super::win::ThreadHandle::first_of(h as isize);
        // A process created suspended has its one thread: resume it. `NtResumeProcess` takes
        // microseconds, where a system-wide thread snapshot to find that thread took milliseconds,
        // which a launch's time would count.
        let started = clocks();
        let mut resumed = nt_resume().is_some_and(|resume| resume(h) >= 0);
        let snap = if resumed { INVALID_HANDLE_VALUE } else { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snap != INVALID_HANDLE_VALUE {
            let mut e: THREADENTRY32 = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut ok = Thread32First(snap, &mut e) != 0;
            while ok {
                if e.th32OwnerProcessID == child.id() {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, e.th32ThreadID);
                    if !t.is_null() {
                        resumed |= ResumeThread(t) != u32::MAX;
                        CloseHandle(t);
                    }
                }
                ok = Thread32Next(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        if !resumed {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("could not resume the child"));
        }
        Ok((child, assigned, started, spawned, first))
    }
}
