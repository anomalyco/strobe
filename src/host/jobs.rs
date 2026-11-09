//! Windows: every process that runs in a job strobe made, held from its start, so what an ended one
//! used is known. Windows keeps a process's cycle count (`QueryProcessCycleTime`) for as long as a
//! handle to it is open, after it exited too; a job's own times move in 15.6 ms clock ticks, and
//! nothing else sums what its ended processes used. Each job posts every process that joins it (its
//! jobs nested in it too: a runtime's own job for its children) to one completion port
//! (`JOB_OBJECT_MSG_NEW_PROCESS`), and one thread of strobe's, at high priority on strobe's CPUs,
//! opens each one as it is posted, while its creation is still in progress (on adais the open came
//! 26–34 µs after the process's creation time; the shortest process, `cmd /c exit`, lived 8 ms). A
//! process opened is checked to be in the job (`IsProcessInJob`) and keyed by its pid and creation
//! time (`host::start_time`'s), so a pid another process took is never read for it. An ended one's
//! cycles are read once (at its exit's post, or the next reading) and its handle closed then.
//!
//! A reading is complete only when the job holds no process strobe does not (`TotalProcesses`, every
//! process ever in it, equals those held, and `TotalProcesses − ActiveProcesses` the ended ones held):
//! one that ended and lost its last handle before strobe opened it is in no count. Nothing breaks away
//! from a launch's job (`spawn::assign`); a process started outside it (by a service, by a process
//! strobe did not launch) is in neither the job's counts nor strobe's, and one of the set that ended
//! refuses (`EndedMarks::dead_since`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, FILETIME, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus};
use windows_sys::Win32::System::JobObjects::{
    IsProcessInJob, JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectAssociateCompletionPortInformation, JobObjectBasicAccountingInformation,
    QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetProcessTimes, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime;

/// `JOB_OBJECT_MSG_NEW_PROCESS` (winnt.h): a process joined the job; the message's overlapped
/// pointer is its pid. `EXIT_PROCESS` and `ABNORMAL_EXIT_PROCESS`: one exited (none is posted for
/// one a job's termination ended).
const NEW_PROCESS: u32 = 6;
const EXIT_PROCESS: u32 = 7;
const ABNORMAL_EXIT_PROCESS: u32 = 8;

/// A process of a job: by pid and creation time, its handle while it runs, its cycles and executable's
/// file name once it ended.
struct Member {
    key: (u32, u64),
    handle: Option<isize>,
    end: Option<u64>,
    name: Option<String>,
}

/// A job strobe watches: its port key, its processes, how many joins the port posted, and whether a
/// reading already waited its whole time for it (Windows does not promise every post): then readings
/// of it are incomplete at once, until the posts catch up. With the jobs inside it that let a process
/// break away (`spawn::assign`'s, and one per process that broke away to it: `take`), and its
/// launch's root, which `spawn::assign` puts in its own.
struct Watched {
    id: usize,
    members: Vec<Member>,
    posted: u32,
    stale: bool,
    inner: Vec<isize>,
    root: Option<u32>,
}

/// The watched jobs by their handle, taken out (`forget`) before the handle is closed.
static WATCHED: Mutex<Option<HashMap<isize, Watched>>> = Mutex::new(None);

/// Who hears of each process as it joins a watched job (`subscribe`): told by pid and creation time
/// once the port's thread holds it, without waiting (a send to a channel never blocks), so the joining
/// process never waits on strobe. A subscriber that went is dropped at the next join.
static SUBSCRIBERS: Mutex<Vec<std::sync::mpsc::Sender<(u32, u64)>>> = Mutex::new(Vec::new());

/// Every process that joins a watched job from now on, by pid and creation time, as the port's thread
/// opens it (while its creation is still in progress: its first thread may not exist yet). Dropping
/// the receiver ends it.
pub fn subscribe() -> std::sync::mpsc::Receiver<(u32, u64)> {
    let (tx, rx) = std::sync::mpsc::channel();
    SUBSCRIBERS.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    rx
}

fn tell(key: (u32, u64)) {
    SUBSCRIBERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| s.send(key).is_ok());
}

/// The completion port and its thread, made at the first job; None where it could not be.
fn port() -> Option<isize> {
    static PORT: std::sync::OnceLock<Option<isize>> = std::sync::OnceLock::new();
    *PORT.get_or_init(|| {
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1) };
        if port.is_null() {
            return None;
        }
        let p = port as isize;
        std::thread::Builder::new().name("strobe-jobs".into()).spawn(move || serve(p)).ok()?;
        Some(p)
    })
}

/// The port's thread's handle (once it runs), and the CPUs it is kept on (`pin`; None: the process's).
static THREAD: Mutex<(Option<isize>, Option<u64>)> = Mutex::new((None, None));

/// Keeps the port's thread on `mask` (strobe's CPUs while a bench in the daemon keeps its own thread
/// there: a new thread takes the process's CPUs, and this one wakes at high priority), or back on the
/// process's CPUs (None). Kept for a thread that starts later.
pub fn pin(mask: Option<u64>) {
    let mut t = THREAD.lock().unwrap_or_else(|e| e.into_inner());
    t.1 = mask;
    if let Some(h) = t.0 {
        apply(h, mask);
    }
}

fn apply(thread: isize, mask: Option<u64>) {
    use windows_sys::Win32::System::Threading::{GetProcessAffinityMask, SetThreadAffinityMask};
    let (mut process, mut system) = (0usize, 0usize);
    let mask = mask.map(|m| m as usize).or_else(|| (unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) } != 0).then_some(process));
    if let Some(m) = mask.filter(|m| *m != 0) {
        unsafe { SetThreadAffinityMask(thread as _, m) };
    }
}

/// The port's thread: takes each posted join as it comes.
fn serve(port: isize) {
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };
    let mut me = std::ptr::null_mut();
    let p = unsafe { GetCurrentProcess() };
    if unsafe { DuplicateHandle(p, GetCurrentThread(), p, &mut me, 0, 0, DUPLICATE_SAME_ACCESS) } != 0 {
        let mut t = THREAD.lock().unwrap_or_else(|e| e.into_inner());
        t.0 = Some(me as isize);
        if t.1.is_some() {
            apply(me as isize, t.1);
        }
    }
    loop {
        let (mut msg, mut key, mut pid) = (0u32, 0usize, std::ptr::null_mut());
        let ok = unsafe { GetQueuedCompletionStatus(port as _, &mut msg, &mut key, &mut pid, INFINITE) } != 0;
        if !ok && pid.is_null() {
            return;
        }
        let mut w = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
        take(w.get_or_insert_with(HashMap::new), msg, key, pid as usize as u32);
    }
}

/// Every join the port holds now, taken here (a reading's, before it counts what is held).
fn drain(port: isize, jobs: &mut HashMap<isize, Watched>) {
    loop {
        let (mut msg, mut key, mut pid) = (0u32, 0usize, std::ptr::null_mut());
        let ok = unsafe { GetQueuedCompletionStatus(port as _, &mut msg, &mut key, &mut pid, 0) } != 0;
        if !ok && pid.is_null() {
            return;
        }
        take(jobs, msg, key, pid as usize as u32);
    }
}

/// One posted message: a process that joined a watched job is opened and held. One already held
/// (the launch's root, `hold`) is not held twice; one that could not be opened as a process of the job
/// (it ended and lost its last handle first, its pid now another's) is not held, and the job's count
/// then exceeds what is held.
/// A held process that exited is read then, and its handle closed: what strobe holds stays the
/// processes running (an ended one's pid is its own while it is held).
fn take(jobs: &mut HashMap<isize, Watched>, msg: u32, key: usize, pid: u32) {
    let Some((&job, w)) = jobs.iter_mut().find(|(_, w)| w.id == key) else { return };
    match msg {
        NEW_PROCESS => {
            w.posted += 1;
            // Without SYNCHRONIZE (a process that denies it) its exit is read from its times.
            let mut h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
            if h.is_null() {
                h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            }
            if !h.is_null()
                && let Some(k) = add(w, job, pid, h as isize)
            {
                tell(k);
                wrap(w, k, h as isize);
            }
        }
        EXIT_PROCESS | ABNORMAL_EXIT_PROCESS => {
            for m in w.members.iter_mut().filter(|m| m.key.0 == pid) {
                settle(m);
            }
        }
        _ => {}
    }
}

/// A process that broke away to the launch's job itself (in none of the jobs inside it that allow
/// that) goes into a job of its own inside it that does, as `spawn::assign` puts the launch's root:
/// else one more breakaway, from it, would fail (the launch's job allows none). Opened while its
/// creation is still in progress, before it can start one. Left as it is where it cannot be (a
/// process that denies the access).
fn wrap(w: &mut Watched, key: (u32, u64), h: isize) {
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, JOB_OBJECT_LIMIT_BREAKAWAY_OK};
    use windows_sys::Win32::System::Threading::{PROCESS_SET_QUOTA, PROCESS_TERMINATE};
    if w.root == Some(key.0) || w.inner.iter().any(|&j| in_job(h, j)) {
        return;
    }
    let Some(inner) = job_with(JOB_OBJECT_LIMIT_BREAKAWAY_OK) else { return };
    let p = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, key.0) };
    let ok = !p.is_null() && created(p as isize) == Some(key.1) && unsafe { AssignProcessToJobObject(inner as _, p) } != 0;
    if ok {
        w.inner.push(inner);
    } else {
        unsafe { CloseHandle(inner as _) };
    }
    if !p.is_null() {
        unsafe { CloseHandle(p) };
    }
}

/// A job with these limits, or None.
pub(super) fn job_with(flags: u32) -> Option<isize> {
    use windows_sys::Win32::System::JobObjects::{CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation};
    unsafe {
        let j = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if j.is_null() {
            return None;
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = flags;
        if SetInformationJobObject(j, JobObjectExtendedLimitInformation, &limits as *const _ as *const _, std::mem::size_of_val(&limits) as u32) == 0 {
            CloseHandle(j);
            return None;
        }
        Some(j as isize)
    }
}

fn in_job(h: isize, job: isize) -> bool {
    let mut inside = 0;
    unsafe { IsProcessInJob(h as _, job as _, &mut inside) != 0 && inside != 0 }
}

/// Reads a held process's cycles once it exited, and closes its handle.
fn settle(m: &mut Member) {
    let Some(h) = m.handle.filter(|_| m.end.is_none()) else { return };
    let mut c = 0u64;
    if exited(h) && unsafe { QueryProcessCycleTime(h as _, &mut c) } != 0 {
        m.end = Some(c);
        m.handle = None;
        unsafe { CloseHandle(h as _) };
    }
}

/// The file name of a process's executable, from a handle to it while it runs: what `EndedCpu` names
/// an ended process by.
fn image_name(h: isize) -> Option<String> {
    use windows_sys::Win32::System::Threading::{PROCESS_NAME_WIN32, QueryFullProcessImageNameW};
    let mut buf = [0u16; 1024];
    let mut n = buf.len() as u32;
    if unsafe { QueryFullProcessImageNameW(h as _, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) } == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..n as usize]);
    path.rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).map(str::to_string)
}

/// The process has exited: signaled, or (a handle without SYNCHRONIZE) an exit time set.
fn exited(h: isize) -> bool {
    match unsafe { WaitForSingleObject(h as _, 0) } {
        WAIT_OBJECT_0 => true,
        WAIT_FAILED => times(h).is_some_and(|(_, exit)| exit != 0),
        _ => false,
    }
}

/// Holds `h` (a process handle strobe owns) as a process of `job` when it is one and not held yet
/// (its key then); else closes it.
fn add(w: &mut Watched, job: isize, pid: u32, h: isize) -> Option<(u32, u64)> {
    let key = created(h).map(|c| (pid, c));
    match key {
        Some(k) if in_job(h, job) && !w.members.iter().any(|m| m.key == k) => {
            // Read now: Windows gives no image name of a process that has exited.
            w.members.push(Member { key: k, handle: Some(h), end: None, name: image_name(h) });
            Some(k)
        }
        _ => {
            unsafe { CloseHandle(h as _) };
            None
        }
    }
}

/// A process's creation and exit times (100 ns since 1601; exit 0 while it runs).
fn times(h: isize) -> Option<(u64, u64)> {
    let t = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = unsafe { std::mem::zeroed() };
    (unsafe { GetProcessTimes(h as _, &mut c, &mut e, &mut k, &mut u) } != 0).then(|| (t(c), t(e)))
}

/// A process's creation time, as `host::start_time` reads it.
fn created(h: isize) -> Option<u64> {
    times(h).map(|t| t.0)
}

/// Watches a new job before any process joins it: its joins post to strobe's port. False where they
/// cannot (then no reading of it is complete).
pub fn watch(job: isize) -> bool {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let Some(port) = port() else { return false };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    WATCHED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(job, Watched { id, members: Vec::new(), posted: 0, stale: false, inner: Vec::new(), root: None });
    let a = JOBOBJECT_ASSOCIATE_COMPLETION_PORT { CompletionKey: id as *mut std::ffi::c_void, CompletionPort: port as _ };
    let ok = unsafe { SetInformationJobObject(job as _, JobObjectAssociateCompletionPortInformation, &a as *const _ as *const _, std::mem::size_of_val(&a) as u32) } != 0;
    if !ok {
        forget(job);
    }
    ok
}

/// Before a launch's root joins `job`: `inner` is the job inside it that the root goes into
/// (`spawn::assign`), which `job` keeps from then (closed by `forget`). False where `job` is not
/// watched: then its caller keeps `inner`.
pub fn prepare(job: isize, pid: u32, inner: isize) -> bool {
    let mut w = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
    match w.as_mut().and_then(|w| w.get_mut(&job)) {
        Some(x) => {
            x.inner.push(inner);
            x.root = Some(pid);
            true
        }
        None => false,
    }
}

/// Holds a launch's root (`process`: strobe's handle, before the process first runs), whatever the
/// port's thread does meanwhile: a launcher that hands over and exits at once is held too.
pub fn hold(job: isize, process: isize, pid: u32) {
    let mut dup = std::ptr::null_mut();
    let me = unsafe { GetCurrentProcess() };
    if unsafe { DuplicateHandle(me, process as _, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) } == 0 {
        return;
    }
    let mut w = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
    match w.as_mut().and_then(|w| w.get_mut(&job)) {
        Some(x) => {
            add(x, job, pid, dup as isize);
        }
        None => unsafe {
            CloseHandle(dup);
        },
    }
}

/// Stops watching a job and closes every handle held of its processes and of the jobs inside it
/// (before the job's handle is closed: its handle value may name another job after). The inner jobs
/// end nothing when closed: the launch's job does.
pub fn forget(job: isize) {
    let Some(gone) = WATCHED.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|w| w.remove(&job)) else { return };
    for h in gone.members.into_iter().filter_map(|m| m.handle).chain(gone.inner) {
        unsafe { CloseHandle(h as _) };
    }
}

/// The processes of some watched jobs at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// Every process that was ever in the jobs is held: the jobs' counts and strobe's agree.
    pub complete: bool,
    /// The jobs read (their port keys).
    pub jobs: BTreeSet<usize>,
    /// The processes that ended, by pid and creation time, with their CPU (cycles, `process_cycles`'
    /// units).
    pub ended: BTreeMap<(u32, u64), u64>,
    /// The processes running in them, by pid and creation time, with their CPU so far.
    pub running: BTreeMap<(u32, u64), u64>,
    /// The executables' file names of the ended ones, where they could be read.
    pub names: BTreeMap<(u32, u64), String>,
}

/// A job's process counts: ever in it, and running now.
fn counts(job: isize) -> Option<(u32, u32)> {
    let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { QueryInformationJobObject(job as _, JobObjectBasicAccountingInformation, &mut a as *mut _ as *mut _, std::mem::size_of_val(&a) as u32, std::ptr::null_mut()) } != 0;
    ok.then_some((a.TotalProcesses, a.ActiveProcesses))
}

/// The jobs' processes now. The port's posts are taken first; a reading waits (100 ms at most) while a
/// join is not posted or taken yet (the thread took it and waits for the lock), and is read again when
/// a process joined or ended while it was read. Incomplete where a job is not watched or not read, or
/// holds a process strobe does not.
pub fn read(jobs: &[isize]) -> Reading {
    let until = Instant::now() + Duration::from_millis(100);
    loop {
        {
            let mut w = WATCHED.lock().unwrap_or_else(|e| e.into_inner());
            let w = w.get_or_insert_with(HashMap::new);
            if let Some(p) = port() {
                drain(p, w);
            }
            match snapshot(w, jobs) {
                Ok(r) => return r,
                // Only the job that kept the reading waiting reads incomplete at once from now on.
                Err(job) if Instant::now() > until => {
                    if let Some(x) = w.get_mut(&job) {
                        x.stale = true;
                    }
                    return Reading::default();
                }
                Err(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// `read` once: the job whose joins are not all taken, or whose counts moved while it was read, as
/// the error.
fn snapshot(w: &mut HashMap<isize, Watched>, jobs: &[isize]) -> Result<Reading, isize> {
    let mut out = Reading { complete: true, ..Reading::default() };
    for &job in jobs {
        let (Some(x), Some((total, active))) = (w.get_mut(&job), counts(job)) else {
            out.complete = false;
            continue;
        };
        if x.posted < total {
            if x.stale {
                out.complete = false;
                continue;
            }
            return Err(job);
        }
        out.jobs.insert(x.id);
        let mut ended = 0u32;
        for m in &mut x.members {
            settle(m);
            match (m.end, m.handle) {
                (Some(c), _) => {
                    ended += 1;
                    out.ended.insert(m.key, c);
                    if let Some(n) = &m.name {
                        out.names.insert(m.key, n.clone());
                    }
                }
                (None, Some(h)) => {
                    let mut c = 0u64;
                    if unsafe { QueryProcessCycleTime(h as _, &mut c) } == 0 {
                        out.complete = false;
                    }
                    out.running.insert(m.key, c);
                }
                (None, None) => out.complete = false,
            }
        }
        // A process missed (fewer held than the job counts) stays missed: no reading again.
        if x.members.len() as u32 != total {
            out.complete = false;
            continue;
        }
        if counts(job) != Some((total, active)) || ended != total - active {
            if x.stale {
                out.complete = false;
                continue;
            }
            return Err(job);
        }
        x.stale = false;
    }
    Ok(out)
}
