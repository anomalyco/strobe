//! This machine: paths, the monotonic clock, processes, CPU topology, power and displays.

pub(crate) mod adb;
pub mod build;
pub mod cache;
pub mod capture;
#[cfg(target_os = "linux")]
pub mod cgroup;
#[cfg(windows)]
pub mod cycle_clock;
#[cfg(target_os = "linux")]
pub mod dbus;
pub mod disk;
pub mod git;
pub mod http;
pub mod ios;
#[cfg(windows)]
pub mod jobs;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod motion;
pub mod paths;
pub mod power;
pub mod reserve;
pub mod spawn;
pub mod tcp;
#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(windows)]
pub mod win;
#[cfg(target_os = "linux")]
pub mod x11;

pub use spawn::{spawn_owned, spawn_pinned};

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
use macos as os;
#[cfg(windows)]
use win as os;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Monotonic nanoseconds (QPC on Windows).
pub fn now_ns() -> u64 {
    #[cfg(windows)]
    {
        win::qpc_ns()
    }
    #[cfg(not(windows))]
    {
        os::now_ns()
    }
}

/// Whether the user's session shows the lock screen: it covers every window, so nothing the target
/// draws reaches the display and Chromium treats its windows as hidden.
pub fn session_locked() -> bool {
    #[cfg(windows)]
    {
        win::session_locked()
    }
    #[cfg(not(windows))]
    {
        os::session_locked()
    }
}

/// Seconds since the last keyboard or mouse input in the session, where strobe uses it (macOS: it
/// takes a person's focus only after a minute without input).
pub fn person_idle_s() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        Some(os::user_idle_s())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Turns the display back on if it went dark on idle (a zero-pixel mouse move on Windows).
pub fn wake_display() {
    #[cfg(windows)]
    win::wake_display();
}

/// The processes that composite the screen, by executable name: Windows' desktop window manager,
/// macOS's window server, X servers and the common Wayland and X11 compositors.
pub const COMPOSITORS: &[&str] = &["dwm.exe", "WindowServer", "Xorg", "Xwayland", "gnome-shell", "kwin_wayland", "kwin_x11", "weston", "sway", "picom", "mutter"];

/// The console display's state (0 off, 1 on, 2 dimmed) and how many times it changed so far; None
/// where it is not known.
pub fn display_state() -> Option<(u8, u64)> {
    #[cfg(windows)]
    {
        win::display_state()
    }
    #[cfg(not(windows))]
    {
        unix_display_state()
    }
}

/// While alive, keeps the machine awake and, while the session is unlocked, the display on (an
/// unattended bench must not let the display sleep under it), and counts how often it saw the session
/// locked, every 500 ms. A locked session's display may go dark as Windows decides: that is how a
/// locked machine runs for its users, and keeping a display lit for days of locked benches would be
/// strobe changing the machine. Benches track the display state per block instead (`display_state`)
/// and redo a pair whose blocks saw different states.
pub struct Awake {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    held: std::sync::Arc<std::sync::atomic::AtomicBool>,
    locked: std::sync::Arc<std::sync::atomic::AtomicU64>,
    unlocked: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Awake {
    /// Tracks the lock state from now on; keeps the machine awake only while `hold(true)`.
    pub fn start() -> Awake {
        use std::sync::atomic::Ordering;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let held = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let locked = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let unlocked = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (s, h, l, u) = (stop.clone(), held.clone(), locked.clone(), unlocked.clone());
        std::thread::spawn(move || {
            // What this `Awake` asks for now: (machine awake, display on).
            let mut asked = (false, false);
            while !s.load(Ordering::Relaxed) {
                let locked = session_locked();
                if locked {
                    l.fetch_add(1, Ordering::Relaxed);
                } else {
                    u.fetch_add(1, Ordering::Relaxed);
                }
                let hold = h.load(Ordering::Relaxed);
                let now = (hold, hold && !locked);
                if now != asked {
                    Awake::ask(asked, now);
                    asked = now;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Awake::ask(asked, (false, false));
        });
        Awake { stop, held, locked, unlocked }
    }

    /// One `Awake` changes what it asks for. Windows keeps an execution state per thread, so each
    /// `Awake` thread holds its own. macOS's power assertions and Linux's inhibitors are one per
    /// process: they follow every live `Awake`'s asks together, so a short command that ends in the
    /// daemon never lets the machine sleep under a bench that still holds it.
    fn ask(old: (bool, bool), new: (bool, bool)) {
        #[cfg(windows)]
        {
            let _ = old;
            os::keep_awake(new.0, new.1);
        }
        #[cfg(not(windows))]
        {
            static ASKING: std::sync::Mutex<(u32, u32)> = std::sync::Mutex::new((0, 0));
            let mut n = ASKING.lock().unwrap_or_else(|e| e.into_inner());
            n.0 = n.0 + new.0 as u32 - old.0 as u32;
            n.1 = n.1 + new.1 as u32 - old.1 as u32;
            os::keep_awake(n.0 > 0, n.1 > 0);
        }
    }

    /// Keeps the machine awake (and the display on while unlocked) or lets it sleep as the user's
    /// settings say: a bench holds it only while its blocks run, never while it waits (within 500 ms).
    pub fn hold(&self, on: bool) {
        self.held.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// Lock observations so far: a block during which this changed saw the lock screen.
    pub fn locks(&self) -> u64 {
        self.locked.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Unlocked observations so far: a block run on a locked screen during which this changed saw it unlocked.
    pub fn unlocks(&self) -> u64 {
        self.unlocked.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// CPU time used so far by one process, in nanoseconds.
#[derive(Clone, Debug, Default)]
pub struct ProcTimes {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    pub cpu_ns: u64,
    /// CPU cycles charged so far (`tsc` units): exact, where `cpu_ns` moves in clock ticks.
    pub cycles: u64,
    pub private_bytes: u64,
    pub working_set: u64,
}

/// Every process in the tree rooted at `root`, with CPU times.
/// One process's CPU so far in `ProcTimes::cycles` units (`tsc` units: cycles on Windows, CPU ns
/// elsewhere), read without listing the system's processes: cheap enough at a run's two ends.
pub fn process_cycles(pid: u32) -> Option<u64> {
    #[cfg(windows)]
    {
        win::process_cycles(pid, None)
    }
    #[cfg(not(windows))]
    {
        os::process_cycles(pid)
    }
}

/// `process_cycles` of the process that started at `start` (`start_time`'s units): on Windows read on
/// the handle that checked it, so a pid another process took since is not read for it (strobe holds
/// its jobs' processes, so their pids stay openable after they exit); elsewhere `process_cycles`.
pub fn process_cycles_at(pid: u32, start: u64) -> Option<u64> {
    #[cfg(windows)]
    {
        win::process_cycles(pid, Some(start))
    }
    #[cfg(not(windows))]
    {
        let _ = start;
        os::process_cycles(pid)
    }
}

pub fn process_tree(root: u32) -> Vec<ProcTimes> {
    process_trees(&[root])
}

/// Every process in the trees rooted at `roots` (each once), with CPU times, from one system
/// snapshot. A child is followed only when it started no earlier than its parent: a parent pid that
/// was reused names an older, unrelated process. Pid 0 roots nothing (the kernel's own).
pub fn process_trees(roots: &[u32]) -> Vec<ProcTimes> {
    trees(roots, &|_| Vec::new())
}

/// Picks root pids from a snapshot's process list (pid, parent, start time).
pub(crate) type PickRoots<'a> = dyn Fn(&[(u32, u32, u64)]) -> Vec<u32> + 'a;

/// `process_trees` of `roots` and of the roots `more` picks from the same snapshot.
fn trees(roots: &[u32], more: &PickRoots) -> Vec<ProcTimes> {
    let more = |list: &[(u32, u32, u64)]| -> Vec<u32> { roots.iter().copied().chain(more(list)).filter(|&r| r != 0).collect() };
    #[cfg(windows)]
    {
        win::process_trees(&more)
    }
    #[cfg(not(windows))]
    {
        os::process_trees(&more)
    }
}

/// Every process now: (pid, parent, start time in `start_time`'s units), the cheapest snapshot.
pub fn process_list() -> Vec<(u32, u32, u64)> {
    #[cfg(windows)]
    {
        win::process_list()
    }
    #[cfg(not(windows))]
    {
        os::process_list()
    }
}

/// Ends a running process and every process of its tree now: Windows terminates each (when its
/// creation time is still the snapshot's), Unix sends SIGKILL to the process group it leads (when it
/// runs and leads one) and to each process of the tree, children first.
pub fn kill_tree(pid: u32) {
    kill_launch(pid, None);
}

/// `kill_tree` of a launch strobe owns and has not reaped; `start` is its start time, read at the
/// launch: on macOS a launch that exited still roots what it left running (`macos::kill_set`).
pub(crate) fn kill_launch(pid: u32, start: Option<u64>) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        let _ = start;
        win::kill_tree(pid);
    }
    #[cfg(unix)]
    {
        #[cfg(target_os = "macos")]
        let tree = macos::kill_set(pid, start);
        #[cfg(not(target_os = "macos"))]
        let tree = {
            let _ = start;
            tree_pids(pid)
        };
        if tree.is_empty() {
            return;
        }
        if unsafe { libc::getpgid(pid as i32) } == pid as i32 {
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        for p in tree.iter().rev() {
            unsafe { libc::kill(*p as i32, libc::SIGKILL) };
        }
    }
}

/// One process's memory by what holds it (SPEC §8.6), in bytes. Windows walks its address space
/// (committed regions); Linux reads `smaps` (resident and swapped pages).
#[derive(Clone, Debug, Default, Serialize)]
pub struct MemoryMap {
    pub pid: u32,
    pub name: String,
    /// What `memory.app.private` reads: the commit charge (Windows), which includes commit held for
    /// the process's GPU allocations; anonymous resident and swapped pages (Linux).
    pub private: u64,
    /// `memory.app.host`: memory the process allocated in its own address space (heap, stacks,
    /// anonymous mappings), without GPU memory mapped into it.
    pub host: u64,
    /// GPU memory mapped for the CPU: write-combined or uncached regions (Windows: a GPU driver maps
    /// the VRAM it allocates for the process this way, and Windows charges it as commit), GPU device
    /// mappings (Linux). `gpu.vram` and `gpu.shared` count the allocations themselves.
    pub gpu_mapped: u64,
    /// Shared memory sections (pagefile-backed on Windows, shmem and memfd on Linux).
    pub shared: u64,
    /// Mapped data files.
    pub files: u64,
    /// Executables and libraries: code and their data.
    pub image: u64,
    /// Windows: commit charged to the process with no private mapping in its address space (page
    /// tables, commit a driver holds for it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmapped: Option<u64>,
    pub working_set: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_working_set: Option<u64>,
}

/// `pid`'s memory by what holds it. None where it cannot be read: a process strobe cannot open, and
/// on macOS any other process (its regions need its task port, which only a debugger gets).
pub fn memory_map(pid: u32) -> Option<MemoryMap> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        os::memory_map(pid)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// One process from a system snapshot.
#[derive(Clone, Debug, Default)]
pub struct ProcInfo {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    /// Kernel + user time, in ns; the OS updates it per clock tick (15.6 ms on Windows).
    pub cpu_ns: u64,
    /// CPU cycles charged at context switches (`tsc` units), exact where `cpu_ns` is coarse.
    pub cycles: u64,
    pub private_bytes: u64,
    pub threads: u32,
}

/// One thread from a system snapshot.
#[derive(Clone, Debug, Default)]
pub struct ThreadInfo {
    pub pid: u32,
    pub tid: u32,
    /// When the thread (macOS: the process, for its row) started, in the OS's own unit: with `pid`
    /// and `tid`, which thread this is, as the OS hands a reused id to a new thread.
    pub born: u64,
    /// Context switches so far: each time the thread was switched in to run (`switches_since`). None
    /// where the OS stopped counting (macOS holds a process's count at 2^31 - 1).
    pub switches: Option<u64>,
    /// Kernel + user time, in ns, in clock ticks.
    pub cpu_ns: u64,
}

/// The context switches between two counts of one thread (the same `born`). Windows counts them in
/// 32 bits, which wrap (a thread cannot switch 2^32 times between two snapshots); elsewhere the
/// count is 64-bit.
pub fn switches_since(earlier: u64, now: u64) -> u64 {
    if cfg!(windows) { u64::from((now as u32).wrapping_sub(earlier as u32)) } else { now.saturating_sub(earlier) }
}

/// The threads of `pids` (sorted) and each process's IO bytes so far (pid, read, written), from one
/// system snapshot.
pub fn threads(pids: &[u32]) -> (Vec<ThreadInfo>, Vec<(u32, u64, u64)>) {
    #[cfg(windows)]
    {
        win::threads(pids)
    }
    #[cfg(not(windows))]
    {
        os::threads(pids)
    }
}

/// CPU cycles charged to a thread so far (`tsc` units), or its name, when it can be opened.
pub fn thread_cycles_of(tid: u32) -> Option<u64> {
    #[cfg(windows)]
    {
        win::thread_cycles_of(tid)
    }
    #[cfg(not(windows))]
    {
        os::thread_cycles_of(tid)
    }
}

pub fn thread_name(tid: u32) -> Option<String> {
    #[cfg(windows)]
    {
        win::thread_name(tid)
    }
    #[cfg(not(windows))]
    {
        os::thread_name(tid)
    }
}

/// Why a process's main thread has no CPU reading (`MainThread`), typed so its refusal names the
/// cause.
#[derive(Clone, Debug, PartialEq)]
pub enum MainLost {
    /// This OS, kernel or user gives no exact CPU count of another process's thread: refused
    /// (`NotOnThisOs`), never read from clock ticks.
    NotOnThisOs(String),
    /// The thread or its process is gone, or none could be bound (`RunIncomplete`).
    Gone(String),
}

impl MainLost {
    /// The refusal of `metric` for it.
    pub fn refusal(&self, metric: &str) -> crate::model::Refusal {
        use crate::model::{Refusal, ValidatorId};
        match self {
            MainLost::NotOnThisOs(why) => Refusal::new(ValidatorId::NotOnThisOs, format!("{metric}: {why}")),
            MainLost::Gone(why) => Refusal::new(ValidatorId::RunIncomplete, format!("{metric}: {why}")).fix("keep the process and its main thread running through the runs"),
        }
    }
}

/// One process's main thread (the thread it started with), bound to that thread for good: a reading
/// is never of another thread that took its id. Reading it costs one system call (about 1 µs), so it
/// is read at the edges of runs in clean timing blocks.
///
/// - Windows: a handle held open (a launch's suspended first thread, `NtGetNextThread`; a running
///   process's oldest thread when it was created with the process, within 5 ms); its cycle counter
///   (`QueryThreadCycleTime`), exact, refused where cycles are not time (`cycles_refused`).
/// - Linux: the thread whose id is the pid, its `/proc/<pid>/task/<pid>/schedstat` and `stat` held
///   open (the kernel answers an open file only for its own task); schedstat's first field, CPU ns,
///   which the kernel adds at each switch and scheduler tick, so a thread on a CPU reads short by up to
///   one tick (4 ms at 250 Hz: a busy share of 101% on a 2-vCPU VM). A reading of a running thread
///   waits for its count to move (the next tick), exact then, so a reading is exact at its `at_ns`.
///   A kernel without schedstat, or with tickless CPUs (`nohz_full`, where a busy thread's count
///   moves once a second), is refused, never read from 10 ms clock ticks.
/// - macOS: the lowest 64-bit thread id when it is bound (ids only grow), read by that id
///   (`PROC_PIDTHREADID64INFO`), which no other thread ever gets.
#[derive(Clone)]
pub struct MainThread {
    pub pid: u32,
    /// Its id as strobe's views key threads (`ThreadInfo::tid`).
    pub tid: u32,
    #[cfg(windows)]
    thread: std::sync::Arc<win::ThreadHandle>,
    #[cfg(target_os = "linux")]
    schedstat: std::sync::Arc<std::fs::File>,
    #[cfg(target_os = "linux")]
    stat: std::sync::Arc<std::fs::File>,
    #[cfg(target_os = "macos")]
    id: u64,
}

/// A main thread's CPU so far (`tsc` units: cycles on Windows, ns elsewhere), with the cycle clock
/// and the host clock read right after it.
#[derive(Clone, Copy, Debug)]
pub struct MainCpu {
    pub cpu: u64,
    pub tsc: Option<u64>,
    pub at_ns: u64,
}

impl MainThread {
    /// The main thread of a running process strobe did not start suspended.
    pub fn attach(pid: u32) -> Result<MainThread, MainLost> {
        let gone = |why: String| MainLost::Gone(format!("process {pid}: {why}"));
        #[cfg(windows)]
        {
            let create = start_time(pid).ok_or_else(|| gone("not running".into()))?;
            let (threads, _) = win::threads(&[pid]);
            let first = threads.iter().min_by_key(|t| t.born).ok_or_else(|| gone("has no threads".into()))?;
            // Created with the process (as `probe::suspend` binds one): else its first thread exited and
            // the oldest left is a worker, never the main thread.
            if first.born.abs_diff(create) > 50_000 {
                return Err(gone("the thread it started with has exited".into()));
            }
            let thread = win::ThreadHandle::open(pid, first.tid, first.born).ok_or_else(|| gone("its main thread could not be opened".into()))?;
            Ok(MainThread::of_handle(pid, thread))
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::FileExt;
            let start = start_time(pid).ok_or_else(|| gone("not running".into()))?;
            let file = match std::fs::File::open(format!("/proc/{pid}/task/{pid}/schedstat")) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && std::path::Path::new(&format!("/proc/{pid}/stat")).exists() => {
                    return Err(MainLost::NotOnThisOs(
                        "this kernel keeps no schedstat (CONFIG_SCHED_INFO): a thread's exact CPU time is not readable, and strobe never reads 10 ms clock ticks".into(),
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(MainLost::NotOnThisOs(format!("process {pid}'s schedstat may not be read ({e}): /proc hides other users' processes")));
                }
                Err(e) => return Err(gone(format!("its main thread could not be read ({e})"))),
            };
            let stat = std::fs::File::open(format!("/proc/{pid}/task/{pid}/stat")).map_err(|e| gone(format!("its main thread could not be read ({e})")))?;
            // Opened for the process that started then: a pid taken again in between is another one.
            if start_time(pid) != Some(start) {
                return Err(gone("exited".into()));
            }
            let tickless = std::fs::read_to_string("/sys/devices/system/cpu/nohz_full").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "(null)");
            if let Some(cpus) = tickless {
                return Err(MainLost::NotOnThisOs(format!("CPUs {cpus} are tickless (nohz_full): a busy thread's CPU count moves there once a second, so no reading of it is exact at a run's edge")));
            }
            if let Some(why) = thread_cpu_refused() {
                return Err(MainLost::NotOnThisOs(why));
            }
            let mut buf = [0u8; 96];
            file.read_at(&mut buf, 0).map_err(|e| gone(format!("its main thread could not be read ({e})")))?;
            Ok(MainThread { pid, tid: pid, schedstat: std::sync::Arc::new(file), stat: std::sync::Arc::new(stat) })
        }
        #[cfg(target_os = "macos")]
        {
            let _ = gone;
            let id = macos::first_thread_id(pid).map_err(|e| macos_lost(pid, e))?;
            Ok(MainThread { pid, tid: macos::short_tid(id), id })
        }
    }

    /// The first thread of a process strobe created suspended (Windows).
    #[cfg(windows)]
    pub fn of_handle(pid: u32, thread: win::ThreadHandle) -> MainThread {
        MainThread { pid, tid: thread.tid, thread: std::sync::Arc::new(thread) }
    }

    /// Its CPU so far, with the clocks. An exited thread is gone.
    pub fn read(&self) -> Result<MainCpu, MainLost> {
        let gone = || MainLost::Gone(format!("process {}'s main thread ended", self.pid));
        #[cfg(windows)]
        let cpu = {
            if let Some(why) = cycles_refused() {
                return Err(MainLost::NotOnThisOs(format!("{why} (thread times move in 15.6 ms ticks, too coarse to use)")));
            }
            if self.thread.exited() {
                return Err(gone());
            }
            self.thread.cycles().ok_or_else(gone)?
        };
        #[cfg(target_os = "linux")]
        {
            let (cpu, at_ns) = self.exact().ok_or_else(gone)?;
            Ok(MainCpu { cpu, tsc: Some(at_ns), at_ns })
        }
        #[cfg(not(target_os = "linux"))]
        {
            #[cfg(target_os = "macos")]
            let cpu = {
                let _ = gone;
                macos::thread_cpu(self.pid, self.id).map_err(|e| macos_lost(self.pid, e))?
            };
            Ok(MainCpu { cpu, tsc: tsc(), at_ns: now_ns() })
        }
    }

    /// Linux: its CPU ns and the moment that count is exact at. A thread not on a CPU was counted
    /// when it left one: exact now (read once more, in case it left a CPU between the two reads). One
    /// that is running (state R) is read again every 50 µs until its count moves, at its CPU's next
    /// tick or when it leaves the CPU, and is exact then (within the 50 µs); a count that does not move
    /// in 25 ms (two ticks at 100 Hz) is of a thread waiting for a CPU, exact all along. A main thread
    /// that ended while others run (a zombie leader) is gone.
    #[cfg(target_os = "linux")]
    fn exact(&self) -> Option<(u64, u64)> {
        use std::os::unix::fs::FileExt;
        let read = |f: &std::fs::File, buf: &mut [u8]| -> Option<usize> { f.read_at(buf, 0).ok().filter(|n| *n > 0) };
        let mut buf = [0u8; 512];
        let cpu = |buf: &mut [u8]| -> Option<u64> {
            let n = read(&self.schedstat, buf)?;
            std::str::from_utf8(&buf[..n]).ok()?.split_whitespace().next()?.parse::<u64>().ok()
        };
        // Its state (`stat` field 3); None once it is a zombie or dead.
        let state = |buf: &mut [u8]| -> Option<u8> {
            let n = read(&self.stat, buf)?;
            let s = *std::str::from_utf8(&buf[..n]).ok()?.rsplit_once(')')?.1.trim_start().as_bytes().first()?;
            (!matches!(s, b'Z' | b'X' | b'x')).then_some(s)
        };
        let first = cpu(&mut buf)?;
        let at = now_ns();
        // Off a CPU: its count was added when it left. Read again: it may have left between the reads.
        let settled = |buf: &mut [u8]| -> Option<(u64, u64)> {
            let c = cpu(buf)?;
            Some(if c != first { (c, now_ns()) } else { (first, at) })
        };
        if state(&mut buf)? != b'R' {
            return settled(&mut buf);
        }
        let until = at + 25_000_000;
        loop {
            std::thread::sleep(std::time::Duration::from_micros(50));
            let c = cpu(&mut buf)?;
            let now = now_ns();
            if c != first {
                return Some((c, now));
            }
            if state(&mut buf)? != b'R' {
                return settled(&mut buf);
            }
            if now > until {
                return Some((first, at));
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_lost(pid: u32, errno: i32) -> MainLost {
    match errno {
        libc::EPERM | libc::EINVAL => MainLost::NotOnThisOs(format!("macOS lets strobe read no thread of process {pid} ({})", std::io::Error::from_raw_os_error(errno))),
        _ => MainLost::Gone(format!("process {pid}'s main thread ended")),
    }
}

/// The share of its time a main thread was on a CPU (0 to 1) over a run's timed parts: `at` its
/// readings at the run's start, at each untimed stretch's start and end, and at its end, so each pair
/// is one timed part, over the time between the pair's own readings (each exact at its `at_ns`; on
/// Linux up to a tick past the run's edge, `MainThread::read`); cycles to ns at the cycle clock's
/// rate from the first reading to the last. `cpu.main` is it times the run's timed time. None
/// without a rate or a pair. At most 1: one thread runs on one CPU at a time, so more is the cycle
/// clock's rate measured over a span a few µs off the counter's (rounding, never a reading).
pub fn main_share(at: &[MainCpu]) -> Option<f64> {
    let (first, last) = (at.first()?, at.last()?);
    let (t0, t1) = (first.tsc?, last.tsc?);
    if at.len() % 2 == 1 || t1 <= t0 || last.at_ns <= first.at_ns {
        return None;
    }
    let per_ns = (t1 - t0) as f64 / (last.at_ns - first.at_ns) as f64;
    let cpu: u64 = at.chunks(2).map(|p| p[1].cpu.saturating_sub(p[0].cpu)).sum();
    let span: u64 = at.chunks(2).map(|p| p[1].at_ns.saturating_sub(p[0].at_ns)).sum();
    (span > 0).then(|| (cpu as f64 / per_ns / span as f64).min(1.0))
}

/// A top-level window: its screen rect (x, y, width, height) and client size in physical pixels, its
/// DPI, and whether a person could see it (visible, not minimized, not cloaked by the compositor).
#[derive(Clone, Debug, Default, Serialize)]
pub struct TopWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub rect: (i32, i32, i32, i32),
    pub client: (i32, i32),
    pub visible: bool,
    pub iconic: bool,
    pub cloaked: bool,
    pub dpi: u32,
}

impl TopWindow {
    pub fn shown(&self) -> bool {
        self.visible && !self.iconic && !self.cloaked
    }

    pub fn scale(&self) -> f64 {
        self.dpi as f64 / 96.0
    }
}

/// Linux: the proportional set size of these processes summed (`/proc/<pid>/smaps_rollup` `Pss`: a
/// page shared by n processes counts 1/n), so memory an app's processes share counts once
/// (`memory.app.pss`). None elsewhere, or when a process's could not be read (no partial sums).
pub fn pss_bytes(pids: &[u32]) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let mut total = 0u64;
        for pid in pids {
            let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
            let kb: u64 = text.lines().find_map(|l| l.strip_prefix("Pss:")).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())?;
            total += kb * 1024;
        }
        (!pids.is_empty()).then_some(total)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pids;
        None
    }
}

/// The largest visible, unminimized top-level window of `pids` (Windows only).
pub fn main_window(pids: &[u32]) -> Option<TopWindow> {
    #[cfg(windows)]
    {
        win::main_window(pids)
    }
    #[cfg(not(windows))]
    {
        os::main_window(pids)
    }
}

/// The visible top-level windows of `pids`, front to back: handle, title, click-through.
pub fn top_windows(pids: &[u32]) -> Vec<(isize, String, bool)> {
    #[cfg(windows)]
    {
        win::top_windows(pids)
    }
    #[cfg(target_os = "linux")]
    {
        linux::top_windows(pids)
    }
    #[cfg(target_os = "macos")]
    {
        macos::top_windows(pids)
    }
}

/// The thread that owns a window (its UI thread), while the window exists. On Linux and macOS no
/// thread owns a window: the UI thread is the process's main thread, where AppKit must run and
/// X11 toolkits do.
pub fn window_thread(hwnd: isize) -> Option<u32> {
    #[cfg(windows)]
    {
        let tid = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd as _, std::ptr::null_mut()) };
        (tid != 0).then_some(tid)
    }
    #[cfg(target_os = "linux")]
    {
        window_state(hwnd).map(|w| w.pid).filter(|&p| p != 0)
    }
    #[cfg(target_os = "macos")]
    {
        macos::main_thread(window_state(hwnd)?.pid)
    }
}

/// A window's client area as it draws it now. Windows: the window's own rendering (`PrintWindow`), else
/// (it failed, or gave black: a Direct3D 12 or Vulkan flip-model window) the compositor's copy of it
/// (`capture::Grabber`).
pub fn window_image(hwnd: isize) -> Option<crate::png::Image> {
    #[cfg(windows)]
    {
        match win::window_image(hwnd) {
            Some(img) if img.rgba.as_chunks::<4>().0.iter().any(|p| p[..3].iter().any(|c| *c > 1)) => Some(img),
            _ => {
                let mut g = capture::Grabber::open(hwnd).ok()?;
                let (w, h) = g.size().ok()?;
                g.grab([0.0, 0.0, w, h]).ok().map(|s| s.img)
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        linux::window_image(hwnd)
    }
    #[cfg(target_os = "macos")]
    {
        macos::window_image(hwnd)
    }
}

/// Whether a window is one of a headless compositor strobe runs (`wayland`): it shows on no display.
pub fn headless_window(hwnd: isize) -> bool {
    #[cfg(target_os = "linux")]
    {
        wayland::owns(hwnd)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = hwnd;
        false
    }
}

/// A debug C runtime the executable imports, when it imports one (Windows executables).
pub fn debug_runtime(exe: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        win::debug_runtime(exe)
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        None
    }
}

/// A window's state now; None once it is gone.
pub fn window_state(hwnd: isize) -> Option<TopWindow> {
    #[cfg(windows)]
    {
        win::window_state(hwnd)
    }
    #[cfg(not(windows))]
    {
        os::window_state(hwnd)
    }
}

/// Moves a window (and with `client`, sizes its client area in device-independent pixels), topmost
/// without focus.
pub fn place_window(w: &TopWindow, x: i32, y: i32, client: Option<(u32, u32)>) {
    #[cfg(windows)]
    win::place_window(w, x, y, client);
    #[cfg(not(windows))]
    os::place_window(w, x, y, client);
}

/// Where a top-level window is, for `page.window` sizing (SPEC §9.6), in the OS's own screen units
/// (physical pixels on Windows and X11, points on macOS): its frame, its client area's size (macOS: the
/// frame's, as CoreGraphics gives no other), the work area of the display it is on, screen units per
/// device-independent pixel, and whether it is maximized: the OS's state where it keeps one and strobe
/// asks for it (X11's window manager), else a frame that fills the work area (Windows, where strobe
/// never sends the activating maximize command; macOS; X11 without a manager that keeps the state).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub frame: [f64; 4],
    pub client: (f64, f64),
    pub work: [f64; 4],
    pub per_dip: f64,
    pub maximized: bool,
    /// The frame's part nobody sees, which may lie past the work area (left, top, right, bottom; screen
    /// units): Windows' invisible resize borders (none on a frameless popup), zero elsewhere.
    pub unseen: [f64; 4],
}

/// A window's `Placement` now; None once it is gone.
pub fn window_placement(hwnd: isize) -> Option<Placement> {
    #[cfg(windows)]
    {
        win::window_placement(hwnd)
    }
    #[cfg(target_os = "macos")]
    {
        macos::window_placement(hwnd)
    }
    #[cfg(target_os = "linux")]
    {
        linux::window_placement(hwnd)
    }
}

/// Puts a window's frame at `frame` (`Placement` units), out of the maximized state first, without
/// activating it. Asked of the OS; the window takes it on its own time.
pub fn set_window_frame(hwnd: isize, frame: [f64; 4]) -> crate::model::Result<()> {
    #[cfg(windows)]
    {
        win::set_window_frame(hwnd, frame)
    }
    #[cfg(target_os = "macos")]
    {
        macos::set_window_frame(hwnd, frame)
    }
    #[cfg(target_os = "linux")]
    {
        linux::set_window_frame(hwnd, frame)
    }
}

/// Maximizes a window without activating it: a frame of the display's work area `work` (Windows: with
/// the frame's invisible resize borders outside it; macOS: never the zoom button, which may take the
/// window to a full-screen Space), or X11's maximized state where the window manager keeps one.
pub fn maximize_window(hwnd: isize, work: [f64; 4]) -> crate::model::Result<()> {
    #[cfg(windows)]
    {
        win::maximize_window(hwnd, work)
    }
    #[cfg(target_os = "macos")]
    {
        macos::set_window_frame(hwnd, work)
    }
    #[cfg(target_os = "linux")]
    {
        linux::maximize_window(hwnd, work)
    }
}

/// Takes a window out of the maximized state and back to `frame` (`Placement` units).
pub fn restore_window(hwnd: isize, frame: [f64; 4]) -> crate::model::Result<()> {
    set_window_frame(hwnd, frame)
}

/// Every process on the machine, from one system snapshot (no process is opened, so protected
/// processes are included).
pub fn all_processes() -> Vec<ProcInfo> {
    #[cfg(windows)]
    {
        win::all_processes()
    }
    #[cfg(not(windows))]
    {
        os::all_processes()
    }
}

/// A process's CPU so far by its pid and start time (`start_time`'s): `app::record::Cycles`.
pub type CpuByProcess = std::collections::BTreeMap<(u32, u64), u64>;

/// What tells that a process of a set ended between two readings of it, even one neither reading saw
/// (`ended_since`), and on Linux and Windows what the CPU of the ended ones came to (`dead_since`). A
/// process that ended took the CPU it used since the first reading with it, so a sum of the set's
/// processes' CPU over the two is short.
#[derive(Clone, Debug, Default)]
pub struct EndedMarks {
    /// Per process of the set, by pid and start time: what its ended children left it, which grows at
    /// each one it waits for (Linux: their page faults and CPU ticks, `cminflt`, `cmajflt`, `cutime`
    /// and `cstime` of `/proc/<pid>/stat`, which every exec moves; macOS: their CPU ns). One the first
    /// reading did not see started in between, with none: it counts from 0.
    pub reaped: CpuByProcess,
    /// Children of the set's processes that ended and are not reaped yet (zombies: in no process
    /// tree, their CPU in no parent's count), by pid and start time, with their parent's pid (Unix).
    pub zombies: std::collections::BTreeMap<(u32, u64), u32>,
    /// Windows: the processes that ended in the jobs strobe launched for the set's home, or rooted at
    /// one of its processes for a launch for no folder (a script's host).
    pub jobs: Option<u64>,
    /// Windows: the processes of those jobs, each held from its start (`jobs`), with what the ended
    /// ones used.
    #[cfg(windows)]
    pub held: Option<jobs::Reading>,
    /// Linux: the CPU of the set's launch cgroups (`cgroup`), where its launches have them.
    #[cfg(target_os = "linux")]
    pub cgroup: Option<cgroup::Reading>,
    /// Linux: a process of the set (as listed with its cycles, just before) had ended when these were
    /// read: its CPU would be both in that listing and in the cgroups' count of what ended.
    pub skewed: bool,
}

impl EndedMarks {
    /// The processes the marks saw running in the set's launches, by pid and start time: Windows'
    /// jobs' (`held`); none elsewhere. A set listed before them lacks one born in between, which
    /// `dead_since` refuses: its reader adds these to the set it reads next.
    pub fn running(&self) -> Vec<(u32, u64)> {
        #[cfg(windows)]
        {
            self.held.as_ref().map(|h| h.running.keys().copied().collect()).unwrap_or_default()
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }

    /// A process of the set ended since `earlier`, as far as the marks tell: a count of a process grew
    /// (from 0 for one `earlier` did not see), a child that ended waits to be reaped that did not
    /// then, or a job counted another (or held one more that ended).
    pub fn ended_since(&self, earlier: &EndedMarks) -> bool {
        #[cfg(windows)]
        if let Some(b) = &self.held
            && b.ended.keys().any(|k| earlier.held.as_ref().is_none_or(|a| !a.ended.contains_key(k)))
        {
            return true;
        }
        self.reaped.iter().any(|(k, v)| *v > earlier.reaped.get(k).copied().unwrap_or(0))
            || self.zombies.keys().any(|k| !earlier.zombies.contains_key(k))
            || self.jobs.unwrap_or(0) > earlier.jobs.unwrap_or(0)
    }

    /// The CPU (the set's cycle units: ns on Linux) the set's processes that ended since `earlier`
    /// used since then, with the processes they started that ended too. `set_a` and `set_b` are the
    /// set's processes' CPU at the two readings. Linux: the launch cgroups' count over the two
    /// readings, less what the processes still running in them used. Windows: each ended process's
    /// cycles held from its start (`jobs`), less its own at `set_a` (0 for one born since). None where
    /// that is not known: no launch cgroup or job at either reading (macOS, a target strobe did not
    /// launch), one removed in between, a job's process strobe did not hold (it ended before strobe
    /// opened it), or something ended outside them (a process of the set that ended or reaped one
    /// outside the cgroups or jobs, or one that left them running).
    /// None too where the cgroups or jobs and the set's listing disagree: a process in them that is
    /// none of the set's at either reading (a grandchild reparented out of the tree the set follows,
    /// one that dropped the launch's stamp: what it used would be taken out of what ended as running
    /// while the set counts none of it, or on Windows counted from 0 where it ran before), or one that
    /// ended between the listing and the cgroups' reading (`skewed`: counted in both).
    pub fn dead_since(&self, earlier: &EndedMarks, set_a: &CpuByProcess, set_b: &CpuByProcess) -> Option<u64> {
        #[cfg(windows)]
        {
            let (a, b) = (earlier.held.as_ref()?, self.held.as_ref()?);
            if !a.complete || !b.complete || !a.jobs.is_subset(&b.jobs) {
                return None;
            }
            if a.running.keys().any(|k| !set_a.contains_key(k)) || b.running.keys().any(|k| !set_b.contains_key(k)) {
                return None;
            }
            // In the set at the first reading and gone at the last: ended in a job, from its reading on
            // (else it left the set running, or ran outside the jobs). One the last reading still
            // read (its handle held) is in `cycles_used` already.
            let mut dead = 0u64;
            for (k, c) in set_a.iter().filter(|(k, _)| !set_b.contains_key(*k)) {
                dead += b.ended.get(k)?.saturating_sub(*c);
            }
            // Born after the first reading (it ran in no reading of it) and ended before the last.
            dead += b.ended.iter().filter(|(k, _)| !a.ended.contains_key(*k) && !set_a.contains_key(*k) && !set_b.contains_key(*k)).map(|(_, c)| c).sum::<u64>();
            Some(dead)
        }
        #[cfg(target_os = "linux")]
        {
            let (a, b) = (earlier.cgroup.as_ref()?, self.cgroup.as_ref()?);
            if !a.launches.is_subset(&b.launches) || earlier.skewed || self.skewed {
                return None;
            }
            if a.members.keys().any(|k| !set_a.contains_key(k)) || b.members.keys().any(|k| !set_b.contains_key(k)) {
                return None;
            }
            // Left the cgroups still running (still in the set), or ended outside them.
            if a.members.keys().any(|k| !b.members.contains_key(k) && set_b.contains_key(k)) || set_a.keys().any(|k| !set_b.contains_key(k) && !a.members.contains_key(k)) {
                return None;
            }
            let inside = |pid: u32| a.members.keys().chain(b.members.keys()).any(|k| k.0 == pid);
            if self.reaped.iter().any(|(k, v)| *v > earlier.reaped.get(k).copied().unwrap_or(0) && !inside(k.0)) {
                return None;
            }
            if self.zombies.iter().any(|(k, parent)| !earlier.zombies.contains_key(k) && !inside(*parent)) {
                return None;
            }
            let running: u64 = b.members.iter().map(|(k, c)| c.saturating_sub(a.members.get(k).copied().unwrap_or(0))).sum();
            Some(b.usage_ns.saturating_sub(a.usage_ns).saturating_sub(running))
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = (earlier, set_a, set_b);
            None
        }
    }

    /// What `dead_since` adds up, by the ended processes' executable: their cycles and how many there
    /// were. Windows, where each was held from its start; empty elsewhere (a launch cgroup counts what
    /// ended as one sum).
    pub fn dead_by_name(&self, earlier: &EndedMarks, set_a: &CpuByProcess, set_b: &CpuByProcess) -> std::collections::BTreeMap<String, (u64, u32)> {
        #[cfg(windows)]
        {
            let mut out: std::collections::BTreeMap<String, (u64, u32)> = std::collections::BTreeMap::new();
            if let (Some(a), Some(b)) = (earlier.held.as_ref(), self.held.as_ref()) {
                let mut add = |k: &(u32, u64), c: u64| {
                    let e = out.entry(b.names.get(k).cloned().unwrap_or_else(|| "(no name)".into())).or_insert((0, 0));
                    e.0 += c;
                    e.1 += 1;
                };
                for (k, c) in set_a.iter().filter(|(k, _)| !set_b.contains_key(*k)) {
                    if let Some(e) = b.ended.get(k) {
                        add(k, e.saturating_sub(*c));
                    }
                }
                for (k, c) in b.ended.iter().filter(|(k, _)| !a.ended.contains_key(*k) && !set_a.contains_key(*k) && !set_b.contains_key(*k)) {
                    add(k, *c);
                }
            }
            out
        }
        #[cfg(not(windows))]
        {
            let _ = (earlier, set_a, set_b);
            std::collections::BTreeMap::new()
        }
    }
}

/// A set's `EndedMarks` now: `pids` are its processes, `home` the folder its launches were for.
pub fn ended_marks(pids: &[u32], home: Option<&Path>) -> EndedMarks {
    let mut out = EndedMarks::default();
    #[cfg(windows)]
    {
        // The count after the processes: one that ends between the two moves the count, and the held
        // reading then lacks it, which `dead_since` refuses rather than miss it.
        out.held = spawn::jobs_cpu(pids, home);
        out.jobs = spawn::jobs_ended(pids, home);
    }
    #[cfg(target_os = "linux")]
    {
        out.cgroup = cgroup::read(pids, home);
        for &pid in pids {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                out.skewed = true;
                continue;
            };
            // After `(comm)`: state is field 0, ppid 1, …, minflt 7, cminflt 8, majflt 9, cmajflt 10,
            // utime 11, stime 12, cutime 13, cstime 14, …, starttime 19 (proc(5)); only the children's.
            let Some(close) = stat.rfind(')') else { continue };
            let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
            out.skewed |= matches!(rest.first(), Some(&"Z" | &"X" | &"x"));
            let Some(start) = rest.get(19).and_then(|s| s.parse::<u64>().ok()) else { continue };
            let sum = [8, 10, 13, 14].iter().filter_map(|&i| rest.get(i)?.parse::<u64>().ok()).fold(0u64, u64::wrapping_add);
            out.reaped.insert((pid, start), sum);
        }
        out.zombies = linux::zombies_of(pids);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = home;
        for &pid in pids {
            if let Ok(k) = macos::counts(pid) {
                out.reaped.insert((pid, k.start_us), k.child_time);
            }
        }
        out.zombies = macos::zombies_of(pids);
    }
    out
}

/// The CPU (the set's cycle units) that processes of a set's launches used and took with them when
/// they ended, since the launches started: what a launch's CPU from its spawn adds to its running
/// processes' (read just before). Linux from its launch cgroups (`cgroup::Reading::ended`), Windows
/// from its jobs' processes held from their start (`jobs`). None without them, where a process of
/// the set runs outside them (or a job's process was not held), and on macOS.
pub fn launch_ended_cpu(pids: &[u32], home: Option<&Path>) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let r = cgroup::read(pids, home)?;
        let same = pids.iter().all(|p| r.members.keys().any(|k| k.0 == *p)) && r.members.keys().all(|k| pids.contains(&k.0));
        same.then(|| r.ended())
    }
    #[cfg(windows)]
    {
        let r = spawn::jobs_cpu(pids, home)?;
        let same = r.complete && pids.iter().all(|p| r.running.keys().any(|k| k.0 == *p)) && r.running.keys().all(|k| pids.contains(&k.0));
        same.then(|| r.ended.values().sum())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (pids, home);
        None
    }
}

/// PIDs of the tree rooted at `root` (root first), from one system snapshot. Children that started
/// before their parent (a reused parent PID) are not followed.
pub fn tree_pids(root: u32) -> Vec<u32> {
    #[cfg(windows)]
    {
        win::tree_pids(root)
    }
    #[cfg(not(windows))]
    {
        os::tree_pids(root)
    }
}

/// Idle cycles (`tsc` units) so far per logical CPU; a CPU's busy share over an interval is
/// `1 - Δidle / Δtsc`. False when unavailable.
pub fn idle_cycles(out: &mut Vec<u64>) -> bool {
    #[cfg(windows)]
    {
        win::idle_cycles(out)
    }
    #[cfg(not(windows))]
    {
        os::idle_cycles(out)
    }
}

/// How long a CPU quota has stopped strobe's cgroup and the cgroups above it so far, in ns (Linux's
/// CPU controller: `cpu.max`, a container's `--cpus`, systemd's `CPUQuota`). What strobe launches
/// inherits its cgroup, so a rise over a run means the target ran below normal speed (`Throttled`).
/// 0 with no quota (no file is read then); None on other OSes and when a quota's count is unreadable.
pub fn cgroup_throttled_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux::cgroup_throttled_ns()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The resolution of Linux's idle counters (`/proc/stat` counts in `USER_HZ` ticks, 100 a second):
/// a CPU's idle time over a short span is off by up to one.
pub const IDLE_TICK_NS: u64 = 10_000_000;

/// Hypervisor steal so far per logical CPU, in ns: a Linux guest reads it (`/proc/stat`); false on
/// bare metal and on other OSes, where a hypervisor's time away shows as nothing a guest can read.
pub fn steal_ns(out: &mut Vec<u64>) -> bool {
    #[cfg(target_os = "linux")]
    {
        static GUEST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*GUEST.get_or_init(|| linux::virtual_machine().is_some()) {
            out.clear();
            return false;
        }
        linux::steal_ns(out)
    }
    #[cfg(not(target_os = "linux"))]
    {
        out.clear();
        false
    }
}

/// A clock in the unit of `ProcInfo::cycles`. Windows: the performance counter times the kernel's
/// cycle factor, or the time stamp counter where the kernel charges raw TSC cycles (x64); None where
/// its cycles are not time (`cycles_refused`, `cycle_clock`). ns elsewhere.
pub fn tsc() -> Option<u64> {
    #[cfg(windows)]
    {
        cycle_clock::read()
    }
    #[cfg(not(windows))]
    {
        Some(now_ns())
    }
}

/// Settles the cycle clock (`cycle_clock`) now: before strobe moves its own CPU affinity and before
/// any measured window opens, since its check pins a thread to every CPU (once per boot, then cached).
pub fn warm_cycle_clock() {
    #[cfg(windows)]
    cycle_clock::clock();
}

/// Why this machine's process and thread cycles cannot be read as time (`tsc` is None), so every
/// metric made of them is refused rather than read from 15.6 ms clock ticks. None where they can.
pub fn cycles_refused() -> Option<String> {
    #[cfg(windows)]
    {
        let os = if cfg!(target_arch = "aarch64") { "Windows on Arm" } else { "Windows" };
        cycle_clock::clock().unit.as_ref().err().map(|why| format!("{os}: {why}"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Why threads' CPU cannot be read here as exact time, so a per-thread view of it is refused rather
/// than read from clock ticks: Windows where cycles are not time (`cycles_refused`), a Linux kernel
/// without schedstat (strobe's own thread reads 0 there). None where it can.
pub fn thread_cpu_refused() -> Option<String> {
    #[cfg(windows)]
    {
        cycles_refused()
    }
    #[cfg(target_os = "linux")]
    {
        // Decided once, from strobe's own main thread, which has run and switched for a while: "0 0 0"
        // there is a kernel that keeps none. A fresh thread or child legitimately reads "0 0 0" before
        // its first switch (with sched_schedstats off), so it is never the one asked.
        static KEPT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let kept = *KEPT.get_or_init(|| {
            let me = std::process::id();
            std::fs::read_to_string(format!("/proc/{me}/task/{me}/schedstat")).ok().is_some_and(|s| s.split_whitespace().any(|f| f != "0"))
        });
        (!kept).then(|| "this kernel keeps no schedstat (CONFIG_SCHED_INFO): a thread's exact CPU time is not readable, and strobe never reads 10 ms clock ticks".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        None
    }
}

/// The cycle clock in words, for `status`.
pub fn cycle_clock_text() -> String {
    #[cfg(windows)]
    {
        cycle_clock::describe()
    }
    #[cfg(target_os = "linux")]
    {
        "ns: the processes' and threads' CPU clocks".into()
    }
    #[cfg(target_os = "macos")]
    {
        "ns: Mach time charged at every context switch".into()
    }
}

/// CPU cycles charged so far to the calling thread (`tsc` units). Exact, where
/// thread times move in 15.6 ms clock ticks.
pub fn thread_cycles() -> u64 {
    #[cfg(windows)]
    {
        win::thread_cycles()
    }
    #[cfg(not(windows))]
    {
        os::thread_cycles()
    }
}

/// Turns on ANSI escape handling for stdout; false when stdout is not a console that has it.
pub fn enable_ansi() -> bool {
    #[cfg(windows)]
    {
        win::enable_ansi()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Installs a Ctrl+C handler (once) and returns the flag it sets instead of killing the process.
pub fn ctrl_c() -> &'static std::sync::atomic::AtomicBool {
    static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    #[cfg(windows)]
    win::on_ctrl_c(&FLAG);
    #[cfg(not(windows))]
    unix_ctrl_c(&FLAG);
    &FLAG
}

/// The tree rooted at `root` plus everything launched for `home` and their trees: every process
/// stamped with it (an app's detached background service, a double-forked daemon: outside the tree,
/// but the target's) and, on Windows, every process in its launch's job. Sorted by pid. strobe's own
/// process is in no target's set (it launched the target, and samples itself apart), unless it is
/// the root.
pub fn process_set(root: u32, home: Option<&Path>) -> Vec<ProcTimes> {
    let mut set = trees(&[root], &|list| match home {
        Some(h) => spawn::pids_in(list, h).into_iter().chain(spawn::group_pids(h)).collect(),
        None => Vec::new(),
    });
    let me = std::process::id();
    set.retain(|p| p.pid != me || root == me);
    set.sort_by_key(|p| p.pid);
    set
}

/// The pids of `process_set`, sorted, for a reader that needs no more (the OS sampler's re-read of a
/// target's set, a page's edges). On Linux it reads only the stat lines and on Windows the snapshot:
/// `process_set` also reads each process's memory (Linux's `smaps_rollup`, which for a browser's
/// dozen processes cost the sampler about 20 ms a re-read on vps).
pub fn process_set_pids(root: u32, home: Option<&Path>) -> Vec<u32> {
    #[cfg(any(target_os = "linux", windows))]
    {
        let roots = |list: &[(u32, u32, u64)]| -> Vec<u32> {
            let more = match home {
                Some(h) => spawn::pids_in(list, h).into_iter().chain(spawn::group_pids(h)).collect(),
                None => Vec::new(),
            };
            std::iter::once(root).chain(more).filter(|&r| r != 0).collect()
        };
        let me = std::process::id();
        let mut set: Vec<u32> = os::tree_pids_of(&roots).into_iter().filter(|&p| p != me || root == me).collect();
        set.sort_unstable();
        set
    }
    #[cfg(target_os = "macos")]
    {
        process_set(root, home).into_iter().map(|p| p.pid).collect()
    }
}

/// `process_set_pids` (each with its creation time) and every thread of them, from one system
/// snapshot (Windows): strobe's own stack sampler re-reads both every refresh.
#[cfg(windows)]
pub fn process_set_threads(root: u32, home: Option<&Path>) -> (Vec<(u32, u64)>, Vec<ThreadInfo>) {
    let roots = |list: &[(u32, u32, u64)]| -> Vec<u32> {
        let more = match home {
            Some(h) => spawn::pids_in(list, h).into_iter().chain(spawn::group_pids(h)).collect(),
            None => Vec::new(),
        };
        std::iter::once(root).chain(more).filter(|&r| r != 0).collect()
    };
    let me = std::process::id();
    let (mut set, mut threads) = win::tree_threads(&roots);
    set.retain(|p| p.0 != me || root == me);
    threads.retain(|t| t.pid != me || root == me);
    set.sort_unstable();
    (set, threads)
}

/// One group of logical CPUs that shares an L3 cache (a CCD on Ryzen).
#[derive(Clone, Debug, Serialize)]
pub struct CacheGroup {
    pub mask: u64,
    pub cpus: u32,
    pub l3_mb: u32,
}

/// One physical core: its logical CPUs (SMT siblings) and how fast its kind is among this CPU's.
#[derive(Clone, Debug, Serialize)]
pub struct Core {
    pub mask: u64,
    /// 0 for the slowest kind of core; higher is faster. Equal on every core of a CPU with one kind,
    /// lower on the efficiency cores of a hybrid one.
    pub class: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct Topology {
    /// How many logical CPUs strobe may use (`allowed`).
    pub logical: u32,
    /// The CPUs strobe may use: its affinity when it started, before it narrowed its own
    /// (`OwnAffinity`), so cpusets, `taskset` and offline or non-contiguous CPUs count.
    pub allowed: u64,
    /// Why strobe's 64-bit CPU masks cannot name every CPU it may use (CPUs numbered 64 or more,
    /// more than one Windows processor group): `--pin auto` leaves the target unpinned then.
    pub limit: Option<String>,
    /// The L3 groups, within `allowed`.
    pub groups: Vec<CacheGroup>,
    /// The physical cores within `allowed`, by first CPU; none on macOS (no CPU affinity). Where
    /// the OS says nothing of SMT (most virtual machines' vCPUs) each logical CPU is its own core.
    pub cores: Vec<Core>,
}

/// What `--pin auto` picks on this machine (`Topology::auto_pin`).
#[derive(Clone, Debug, Serialize)]
pub struct AutoPin {
    /// The target's CPUs; None leaves it unpinned.
    pub target: Option<u64>,
    /// Every other allowed CPU: strobe's and the runner's (and where `--reserve-cpus` moves this
    /// user's other processes). Never empty when `target` is set.
    pub reserved: Option<u64>,
    /// What was picked and why, in one line.
    pub reason: String,
}

impl Topology {
    /// The group with the largest L3 (3D V-Cache), when groups differ.
    pub fn vcache(&self) -> Option<&CacheGroup> {
        let max = self.groups.iter().map(|g| g.l3_mb).max()?;
        let min = self.groups.iter().map(|g| g.l3_mb).min()?;
        if max == min {
            return None;
        }
        self.groups.iter().find(|g| g.l3_mb == max)
    }

    /// The CPU's shape as `status` and `up` print it: one L3 group is no CCD (an Intel CPU, a single-CCD
    /// Ryzen), several are named with their CPUs and V-Cache; a hybrid CPU adds its kinds of core.
    pub fn cpu_shape(&self) -> Vec<String> {
        let vcache = self.vcache().map(|v| v.mask);
        let mut shape: Vec<String> = match self.groups.as_slice() {
            [g] => vec![format!("{} MB L3", g.l3_mb)],
            groups => groups.iter().enumerate().map(|(i, g)| format!("CCD{i} {} MB L3 ({} CPUs{})", g.l3_mb, g.cpus, if vcache == Some(g.mask) { ", V-Cache" } else { "" })).collect(),
        };
        let fastest = self.cores.iter().map(|c| c.class).max().unwrap_or(0);
        let fast = self.cores.iter().filter(|c| c.class == fastest).count();
        let slower = self.cores.len() - fast;
        if slower > 0 {
            shape.push(format!("{fast} fast and {slower} slower core{}", if slower == 1 { "" } else { "s" }));
        }
        shape
    }

    /// Every CPU strobe may use (`allowed`).
    pub fn all(&self) -> u64 {
        self.allowed
    }

    /// Where `--pin auto` puts the target, with strobe and the runner on every other allowed CPU
    /// (SPEC §9.4). The target never shares a physical core with them and never mixes core kinds:
    /// - L3 groups that differ: the target on the largest (3D V-Cache).
    /// - Equal L3 groups: the target on the first, when it holds 4 or more cores of one kind.
    /// - Hybrid CPU: the target on the fastest kind of core when there are 4 or more of them,
    ///   strobe and the runner on the slower ones.
    /// - One kind, 4 or more cores: the last cores/4 (1 or 2) whole cores kept apart, the target on
    ///   the rest.
    /// - Fewer cores, CPUs the masks cannot name, or macOS (no CPU affinity): unpinned, naming the
    ///   `--pin` list that would pin anyway where there is one.
    pub fn auto_pin(&self) -> AutoPin {
        let unpinned = |why: String| AutoPin { target: None, reserved: None, reason: format!("unpinned ({why})") };
        if cfg!(target_os = "macos") {
            return unpinned("macOS has no CPU affinity: the OS places every thread".into());
        }
        if let Some(why) = &self.limit {
            return unpinned(why.clone());
        }
        let all = self.all();
        let cores: Vec<Core> = if self.cores.is_empty() { bits(all).map(|c| Core { mask: 1 << c, class: 0 }).collect() } else { self.cores.clone() };
        let split = |target: u64, what: String| -> Option<AutoPin> {
            let reserved = all & !target;
            (target & all != 0 && reserved != 0).then(|| AutoPin {
                target: Some(target & all),
                reserved: Some(reserved),
                reason: format!("{what} · strobe and the runner on CPUs {}", cpu_text(reserved)),
            })
        };
        if self.groups.len() > 1 {
            let pick = match self.vcache() {
                Some(v) => Some((v, "V-Cache".to_string())),
                None => {
                    let g = &self.groups[0];
                    let inside: Vec<&Core> = cores.iter().filter(|c| c.mask & !g.mask == 0).collect();
                    (inside.len() >= 4 && inside.iter().all(|c| c.class == inside[0].class)).then(|| (g, format!("first of {} equal L3 groups", self.groups.len())))
                }
            };
            if let Some(p) = pick.and_then(|(g, why)| {
                let i = self.groups.iter().position(|x| x.mask == g.mask).unwrap_or(0);
                split(g.mask, format!("CCD{i} ({} MB L3, {} CPUs) · {why}", g.l3_mb, g.cpus))
            }) {
                return p;
            }
        }
        let fastest = cores.iter().map(|c| c.class).max().unwrap_or(0);
        let fast: Vec<&Core> = cores.iter().filter(|c| c.class == fastest).collect();
        if fast.len() < cores.len() {
            let mask = fast.iter().fold(0u64, |m, c| m | c.mask);
            let n = fast.len();
            if n >= 4
                && let Some(p) = split(mask, format!("CPUs {} · the {n} fastest cores of {}", cpu_text(mask), cores.len()))
            {
                return p;
            }
            return unpinned(format!("{n} fastest cores, too few to keep the slower ones apart by default: --pin {} pins the target to them", cpu_text(mask)));
        }
        if cores.len() < 4 {
            let Some(last) = cores.iter().max_by_key(|c| c.mask.trailing_zeros()) else { return unpinned("no CPU known".into()) };
            let rest = all & !last.mask;
            if rest == 0 {
                return unpinned("1 core: nothing to keep apart".into());
            }
            return unpinned(format!("{} cores, too few to keep one apart by default: --pin {} pins the target there and leaves CPUs {} to strobe", cores.len(), cpu_text(rest), cpu_text(last.mask)));
        }
        let keep = (cores.len() / 4).clamp(1, 2);
        let mut order: Vec<&Core> = cores.iter().collect();
        order.sort_by_key(|c| std::cmp::Reverse(c.mask.trailing_zeros()));
        let kept = order.iter().take(keep).fold(0u64, |m, c| m | c.mask);
        let target = all & !kept;
        split(target, format!("CPUs {} · {keep} of {} cores kept apart", cpu_text(target), cores.len())).unwrap_or_else(|| unpinned("no core left for the target".into()))
    }
}

/// The CPUs strobe may use and what its masks cannot name, read once: the first call comes before
/// strobe narrows its own affinity (`set_affinity`, `set_thread_affinity` and `topology` read it).
pub(crate) struct Allowed {
    pub mask: u64,
    pub count: u32,
    pub limit: Option<String>,
}

pub(crate) fn allowed() -> &'static Allowed {
    static ALLOWED: std::sync::OnceLock<Allowed> = std::sync::OnceLock::new();
    ALLOWED.get_or_init(os::allowed)
}

/// The CPU numbers in a mask.
pub(crate) fn bits(mask: u64) -> impl Iterator<Item = u32> {
    (0..64).filter(move |c| (mask >> c) & 1 == 1)
}

/// A CPU mask as a list: `0-5,8,10-11`.
pub fn cpu_text(mask: u64) -> String {
    let mut parts = Vec::new();
    let mut c = 0u32;
    while c < 64 {
        if (mask >> c) & 1 == 0 {
            c += 1;
            continue;
        }
        let start = c;
        while c < 64 && (mask >> c) & 1 == 1 {
            c += 1;
        }
        parts.push(if c - 1 == start { start.to_string() } else { format!("{start}-{}", c - 1) });
    }
    parts.join(",")
}

pub fn topology() -> Topology {
    allowed();
    #[cfg(windows)]
    {
        win::topology()
    }
    #[cfg(not(windows))]
    {
        os::topology()
    }
}

/// Restricts this process (and the children it starts afterwards) to `mask` (0: every allowed CPU);
/// returns the old mask.
pub fn set_affinity(mask: u64) -> Option<u64> {
    allowed();
    #[cfg(windows)]
    {
        win::set_affinity(mask)
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "linux")]
        return os::set_affinity(mask);
        #[cfg(not(target_os = "linux"))]
        {
            let _ = mask;
            None
        }
    }
}

/// Restricts the calling thread to `mask`.
pub fn set_thread_affinity(mask: u64) {
    allowed();
    #[cfg(windows)]
    win::set_thread_affinity(mask);
    #[cfg(target_os = "linux")]
    os::set_thread_affinity(mask);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = mask;
}

/// Keeps the thread that holds the processes of strobe's jobs on `mask`, or back on the process's
/// CPUs (None): Windows (`jobs::pin`); its own thread elsewhere has none.
pub fn pin_job_thread(mask: Option<u64>) {
    #[cfg(windows)]
    jobs::pin(mask);
    #[cfg(not(windows))]
    let _ = mask;
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Env {
    pub os: String,
    pub cpu: String,
    pub logical_cpus: u32,
    pub l3_groups: Vec<CacheGroup>,
    pub power_plan: String,
    pub on_battery: bool,
    /// The hypervisor, when this is a virtual machine.
    pub virtual_machine: Option<String>,
    /// The OS's own "reduce motion" setting (`motion::reduced_motion`), read only; None when unknown.
    pub reduced_motion: Option<bool>,
    /// Why this is a CI runner other jobs share (`shared_runner`): no timing holds here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_runner: Option<String>,
    pub strobe: String,
}

/// Why this machine is a CI runner other jobs share, whose timings move with its neighbours' load
/// (SPEC §10.1 `SharedRunner`): a GitHub-hosted runner (`RUNNER_ENVIRONMENT=github-hosted`), a job
/// that says so (`CI_SHARED_ENVIRONMENT=true`), neither of which anything overrides; or a CI job in a
/// virtual machine, unless it says it is dedicated (`RUNNER_ENVIRONMENT=self-hosted`, or
/// `CI_SHARED_ENVIRONMENT` false, 0, no or off). A person's shell, and a CI job on bare metal, is none.
pub fn shared_runner() -> Option<String> {
    let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let runner = var("RUNNER_ENVIRONMENT");
    if runner.as_deref() == Some("github-hosted") {
        return Some("a GitHub-hosted runner (RUNNER_ENVIRONMENT=github-hosted)".into());
    }
    let falsey = |v: &str| matches!(v.to_ascii_lowercase().as_str(), "false" | "0" | "no" | "off");
    let declared = var("CI_SHARED_ENVIRONMENT");
    if declared.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return Some("a runner its job says is shared (CI_SHARED_ENVIRONMENT=true)".into());
    }
    // The guess below (a CI job in a virtual machine) is all a job can say otherwise of: a dedicated
    // runner names itself self-hosted, or its job sets CI_SHARED_ENVIRONMENT=false.
    if runner.as_deref() == Some("self-hosted") || declared.as_deref().is_some_and(falsey) {
        return None;
    }
    let ci = ["CI", "GITHUB_ACTIONS", "GITLAB_CI", "CIRCLECI", "TF_BUILD", "BUILDKITE"].into_iter().find(|k| var(k).is_some_and(|v| !falsey(&v)))?;
    #[cfg(windows)]
    let vm = win::virtual_machine();
    #[cfg(not(windows))]
    let vm = os::virtual_machine();
    vm.map(|vm| format!("a CI job ({ci} is set) in a virtual machine ({vm}), taken for a shared runner (a dedicated one: RUNNER_ENVIRONMENT=self-hosted or CI_SHARED_ENVIRONMENT=false)"))
}

/// The active power plan's name, with Windows 11's power mode when one is set on it
/// ("Balanced · Best performance").
pub fn env_power_plan() -> String {
    #[cfg(windows)]
    {
        let plan = win::power_plan();
        match power::mode() {
            Some(m) if plan == "Balanced" => format!("{plan} · {m}"),
            _ => plan,
        }
    }
    #[cfg(not(windows))]
    {
        os::power_plan()
    }
}

/// A laptop running on its battery.
pub fn on_battery() -> bool {
    #[cfg(windows)]
    {
        win::on_battery()
    }
    #[cfg(not(windows))]
    {
        os::on_battery()
    }
}

pub fn env() -> Env {
    let topo = topology();
    #[cfg(windows)]
    let (os, cpu, power_plan, on_battery) = (win::os_name(), win::cpu_name(), env_power_plan(), win::on_battery());
    #[cfg(not(windows))]
    let (os, cpu, power_plan, on_battery) = (os::os_name(), os::cpu_name(), env_power_plan(), os::on_battery());
    #[cfg(windows)]
    let virtual_machine = win::virtual_machine();
    #[cfg(not(windows))]
    let virtual_machine = os::virtual_machine();
    let reduced_motion = motion::reduced_motion();
    Env {
        os,
        cpu,
        logical_cpus: topo.logical,
        l3_groups: topo.groups,
        power_plan,
        on_battery,
        virtual_machine,
        reduced_motion,
        shared_runner: shared_runner(),
        strobe: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Share of all CPUs busy over `ms`, 0..1.
pub fn system_busy(ms: u64) -> f64 {
    #[cfg(windows)]
    {
        win::system_busy(ms)
    }
    #[cfg(not(windows))]
    {
        os::system_busy(ms)
    }
}

/// The largest visible window of these processes, shrunk to a 16×16 grid of averaged BGRA pixels read
/// from the screen: what a person sees of it, for "painted" and "settled" (Windows only).
pub fn window_pixels(pids: &[u32]) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        win::window_pixels(pids)
    }
    #[cfg(not(windows))]
    {
        os::window_pixels(pids)
    }
}

/// Raises the visible windows of these processes (a target's process set) to topmost without
/// activating them; returns how many.
pub fn raise_windows(pids: &[u32]) -> usize {
    #[cfg(windows)]
    {
        win::raise_windows(pids)
    }
    #[cfg(not(windows))]
    {
        os::raise_windows(pids)
    }
}

/// Refresh rate (Hz) of the display at a screen point.
pub fn refresh_at(x: i32, y: i32) -> Option<u32> {
    #[cfg(windows)]
    {
        win::refresh_at(x, y)
    }
    #[cfg(not(windows))]
    {
        os::refresh_at(x, y)
    }
}

/// `refresh_at` for calls inside a run (an input's pacing): Windows reads each display's mode once per
/// display until its mode changes (`win::refresh_at_kept`), elsewhere it is read each time.
pub fn refresh_at_kept(x: i32, y: i32) -> Option<u32> {
    #[cfg(windows)]
    {
        win::refresh_at_kept(x, y)
    }
    #[cfg(not(windows))]
    {
        os::refresh_at(x, y)
    }
}

/// A display, in device-independent pixels on the desktop (from the main display's top left): its
/// bounds, and its work area, what a window may cover (less the menu bar and the Dock on macOS, the
/// taskbar on Windows, panels the window manager keeps on X11).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct WorkArea {
    pub screen: [f64; 4],
    pub work: [f64; 4],
}

/// The `WorkArea` of the display holding the point `at` (a window's centre) now (the Dock or taskbar
/// may move), or None without a display.
pub fn work_area(at: (f64, f64)) -> Option<WorkArea> {
    #[cfg(windows)]
    {
        win::work_area(at)
    }
    #[cfg(target_os = "macos")]
    {
        macos::work_area(at)
    }
    #[cfg(target_os = "linux")]
    {
        x11::work_area(at)
    }
}

/// This machine's name, lower case and without a domain: `COMPUTERNAME` on Windows, `gethostname`
/// elsewhere (shells keep `HOSTNAME` unexported, and macOS adds `.local`).
pub fn device_name() -> String {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    #[cfg(not(windows))]
    let name = {
        let mut buf = [0u8; 256];
        let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if ok { String::from_utf8_lossy(&buf[..end]).to_string() } else { std::env::var("HOSTNAME").unwrap_or_default() }
    };
    name.split('.').next().unwrap_or("").to_lowercase()
}

/// The module each thread started in (its path), per tid, for `(pid, tid)` pairs (Windows: its start
/// address; Linux: read from the top of its stack; macOS keeps no per-thread start a user can read).
pub fn thread_modules(threads: &[(u32, u32)]) -> std::collections::HashMap<u32, String> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        os::thread_modules(threads)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = threads;
        Default::default()
    }
}

/// The display path a window shows on: its display target (the `VidPnTargetId` vertical blanks
/// carry), the adapter that scans it out (LUID), and its nominal refresh.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DisplayPath {
    pub device: String,
    pub target: u32,
    pub adapter_luid: u64,
    pub hz: f64,
}

/// The display path of the monitor a window is on (the nearest one).
pub fn display_of(hwnd: isize) -> Option<DisplayPath> {
    #[cfg(windows)]
    {
        win::display_of(hwnd)
    }
    #[cfg(not(windows))]
    {
        os::display_of(hwnd)
    }
}

/// Finds an executable: on PATH, then in the given candidate paths.
pub fn find_exe(name: &str, candidates: &[PathBuf]) -> Option<PathBuf> {
    if let Some(found) = candidates.iter().find(|p| p.is_file()) {
        return Some(found.clone());
    }
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(&exe)).find(|p| p.is_file()))
}

/// Runs a command and returns stdout when it succeeds.
pub fn output(cmd: &mut Command) -> Option<String> {
    cmd.output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// The user's own Documents and Desktop folders, where an app saves without being asked to (an
/// onboarding project): Windows' known folders where the shell keeps them (OneDrive's folder backup
/// moves them out of the profile), elsewhere in the home.
pub fn user_folders() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        win::user_folders()
    }
    #[cfg(not(windows))]
    {
        vec![home().join("Documents"), home().join("Desktop")]
    }
}

pub fn exists(path: &Path) -> bool {
    path.exists()
}

/// Why a process (None: strobe itself, whose priority what it launches inherits) runs at background
/// priority: macOS `taskpolicy -b`, Windows' idle priority class, efficiency mode or background mode,
/// Linux `SCHED_IDLE` or nice 19. There the OS keeps it on slow cores or behind everything else, so
/// its frames and input latencies describe no person's machine (SPEC §10.1 `BackgroundPriority`).
pub fn background_qos(pid: Option<u32>) -> Option<String> {
    os::background_qos(pid.unwrap_or(0))
}

/// macOS: why strobe's timers (and those of what it launches) fire late, when they do: a launchd job
/// whose ProcessType lets macOS coalesce them (SPEC §10.1 `TimerCoalescing`).
pub fn timer_coalescing() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        os::timer_coalescing().map(|(ms, job)| {
            format!(
                "timers that wake about {} late: it runs in a launchd job (its oldest ancestor: {job}) whose ProcessType lets macOS coalesce the timers of everything it starts",
                crate::fmt::time(ms)
            )
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The executable path of a running process.
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    os::exe_of(pid)
}

/// PIDs of running processes whose executable is exactly `exe`.
pub fn running(exe: &Path) -> Vec<u32> {
    os::running(exe)
}

/// A process's environment (`NAME=value`), never its arguments, where this user may read it.
pub fn env_of(pid: u32) -> Option<Vec<String>> {
    os::env_of(pid)
}

/// When a process started, in units of its OS (only compared for equality): with its pid it names
/// one process, since a reused pid starts later.
pub fn start_time(pid: u32) -> Option<u64> {
    os::start_time(pid)
}

/// SIGINT and SIGTERM set `flag` instead of ending strobe; a second one while the flag is still set
/// ends it as the signal would.
#[cfg(unix)]
pub fn unix_ctrl_c(flag: &'static std::sync::atomic::AtomicBool) {
    use std::sync::atomic::{AtomicPtr, Ordering};
    static TARGET: AtomicPtr<std::sync::atomic::AtomicBool> = AtomicPtr::new(std::ptr::null_mut());
    extern "C" fn handler(sig: libc::c_int) {
        let flag = TARGET.load(Ordering::SeqCst);
        if !flag.is_null() && !unsafe { (*flag).swap(true, Ordering::SeqCst) } {
            return;
        }
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
    if TARGET.swap(flag as *const _ as *mut _, Ordering::SeqCst).is_null() {
        unsafe {
            libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, handler as *const () as libc::sighandler_t);
        }
    }
}

/// The main display on (1) or asleep (0), and how often that changed, from a poll every 500 ms that
/// starts on first use (macOS); None where the OS does not say (Linux).
#[cfg(unix)]
fn unix_display_state() -> Option<(u8, u64)> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
        static STATE: AtomicU8 = AtomicU8::new(u8::MAX);
        static CHANGES: AtomicU64 = AtomicU64::new(0);
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            STATE.store(macos::display_on() as u8, Ordering::Relaxed);
            std::thread::spawn(|| {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let v = macos::display_on() as u8;
                    if STATE.swap(v, Ordering::Relaxed) != v {
                        CHANGES.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        });
        Some((STATE.load(Ordering::Relaxed), CHANGES.load(Ordering::Relaxed)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// 32-bit pixels (`stride` bytes per row) averaged into a 16×16 grid of 4-byte cells, sampling every
/// second pixel of each cell.
#[cfg(unix)]
pub fn grid16(bytes: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; 16 * 16 * 4];
    for cy in 0..16 {
        for cx in 0..16 {
            let (x0, x1, y0, y1) = (cx * width / 16, (cx + 1) * width / 16, cy * height / 16, (cy + 1) * height / 16);
            let mut sum = [0u64; 4];
            let mut n = 0u64;
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    let at = y * stride + x * 4;
                    if let Some(px) = bytes.get(at..at + 4) {
                        for (s, p) in sum.iter_mut().zip(px) {
                            *s += *p as u64;
                        }
                        n += 1;
                    }
                }
            }
            for (k, s) in sum.iter().enumerate() {
                out[(cy * 16 + cx) * 4 + k] = (s / n.max(1)) as u8;
            }
        }
    }
    out
}
