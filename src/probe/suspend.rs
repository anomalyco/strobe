//! Native stacks without elevation (Windows x64, attribution only, SPEC §13.4): when the deep-mode
//! helper is not installed, strobe samples the target's own threads from its own process, as a
//! debugger would, which a same-user process may do. Every 2 ms it reads the cycle counter of each
//! thread that ran in the last 100 ms, and a thread that used another 2 ms of CPU since its last
//! sample is sampled: suspended, its registers read (`GetThreadContext`, which waits for the suspend
//! to land) and up to 64 KB of its stack copied in one `ReadProcessMemory`, then resumed. Unwinding
//! happens after the resume, on that copy, with the x64 unwind data (`.pdata` and `UNWIND_INFO`) of
//! the modules in the target's memory, as `RtlVirtualUnwind` does in a process for itself. So each
//! sample stands for 2 ms of that thread's CPU, as the kernel's sampled profile does for 200 µs.
//!
//! Caveats, said in the recording: no kernel frames (the user-mode part only); only threads that
//! exist at a refresh (every 100 ms, 20 ms while the set's processes change; the first opens every
//! thread) are sampled, each from its opening, except a child that joins a launch's job: told as it
//! is created (`jobs::subscribe`), it is opened at the next tick; a thread that ran and is waiting
//! when sampled shows where it waits; code without unwind data (a JIT's) ends the stack there; a
//! thread that ran more than 4 ms between two looks (a late tick) gives one sample for it, and the
//! rest is counted (`forgone`) for coverage; each sample stalls the
//! thread for the time between suspend and resume (measured per sample and reported), and while it is
//! stopped it may hold a lock (the loader's, a heap's) another of the app's threads then waits on.
//! strobe takes no lock of the target and calls nothing in it, so the target cannot stall strobe.
//! Samples are capped at one per thread per 2 ms and 4,000 a second in all.

use crate::host;
use crate::model::{Error, Result};
use crate::probe::stacks::{Deep, Module, SuspendStats, Targets};
use std::collections::HashMap;
use std::sync::mpsc;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_CONTROL_AMD64, CONTEXT_INTEGER_AMD64, GetThreadContext, ReadProcessMemory};
use windows_sys::Win32::System::Threading::{
    CreateWaitableTimerExW, OpenProcess, OpenThread, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, ResumeThread, SetWaitableTimer, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
    THREAD_SUSPEND_RESUME, WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;

/// The sampling interval: one sample per 2 ms of a thread's CPU.
const INTERVAL_US: u64 = 2000;
/// Samples a second at most, over all threads.
const MAX_RATE: u64 = 4000;
/// Stack bytes copied per sample.
const STACK_BYTES: usize = 64 << 10;

pub struct Capture {
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<Result<Deep>>>,
}

impl Capture {
    pub fn start(targets: Targets) -> Result<Capture> {
        if !cfg!(target_arch = "x86_64") {
            return Err(Error::msg("native stacks without the deep helper unwind x64 code only"));
        }
        if targets.pids().is_empty() {
            return Err(Error::msg(format!("process {} is not running", targets.root)));
        }
        // Each sample stands for 2 ms of its thread's cycles: without the cycle clock as time there is
        // no 2 ms to count (a rate of 0 sampled every thread that was listed as busy at every tick).
        if let Some(why) = host::cycles_refused() {
            return Err(Error::msg(format!("strobe's own stack sampler weighs each sample by its thread's CPU cycles, and {why}"))
                .hint("install the deep helper (strobe up), whose kernel sampler needs no cycle clock"));
        }
        let (stop, rx) = mpsc::channel();
        let thread = std::thread::Builder::new().name("strobe-stacks".into()).spawn(move || run(&targets, rx)).map_err(|e| Error::msg(format!("the stack sampler thread: {e}")))?;
        Ok(Capture { stop, thread: Some(thread) })
    }

    pub fn stop(mut self) -> Result<Deep> {
        let _ = self.stop.send(());
        self.thread.take().ok_or_else(|| Error::msg("no stack sampler thread"))?.join().map_err(|_| Error::msg("the stack sampler thread panicked"))?
    }
}

/// Dropped without `stop`: the sampler ends at its next 2 ms tick (never between a suspend and its
/// resume), closing every thread and process handle, joined within 2 s.
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            super::join_within(t, std::time::Duration::from_secs(2));
        }
    }
}

struct Thread {
    pid: u32,
    handle: HANDLE,
    cycles: u64,
    /// Its cycles when the capture began counting it: at its opening for a thread that existed
    /// before the capture, 0 for one created during it (all of its CPU is the capture's). Coverage
    /// only: sampling credit starts at the opening.
    base: u64,
    /// CPU since the last sample, in samples' worth.
    credit: f64,
    stack_base: Option<u64>,
    /// Refreshes left to read a name it has not set yet (a runtime names its main thread after the
    /// loader, and strobe may open it before).
    unnamed: u8,
    /// When strobe opened it: a look at the set taken before then may not list it.
    opened: std::time::Instant,
}

impl Drop for Thread {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

struct Process {
    handle: HANDLE,
    modules: Vec<Image>,
    /// When the module list was read (a new module is found on the next miss after 200 ms).
    read_at: std::time::Instant,
    /// Reads since its opening that found no module (`reread_due`).
    empty_reads: u8,
    /// Unwind data read so far, by address (None: unreadable).
    unwind: HashMap<u64, Option<Vec<u8>>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

/// Reads of an empty module list after the first, 10 ms apart, before the 200 ms wait applies.
const EMPTY_READS: u8 = 8;

impl Process {
    /// Whether a frame in no module may read the module list again. A list that came back empty (a
    /// process opened before its loader listed its modules, which Windows then refuses) is read again
    /// at the first miss, then every 10 ms up to `EMPTY_READS` times: a short child ends before
    /// 200 ms, and every sample of it stayed a bare address in no module. Otherwise every 200 ms, for
    /// a module loaded since.
    fn reread_due(&self) -> bool {
        let since = self.read_at.elapsed();
        match (self.modules.is_empty(), self.empty_reads) {
            (true, 0) => true,
            (true, n) if n <= EMPTY_READS => since >= std::time::Duration::from_millis(10),
            _ => since > std::time::Duration::from_millis(200),
        }
    }

    fn reread(&mut self) {
        self.modules = images(self.handle);
        self.read_at = std::time::Instant::now();
        if self.modules.is_empty() {
            self.empty_reads = self.empty_reads.saturating_add(1);
        }
    }
}

/// One module in the target's memory and its function table, read once.
struct Image {
    base: u64,
    size: u64,
    path: String,
    /// (begin RVA, end RVA, UNWIND_INFO RVA), sorted.
    functions: Option<Vec<(u32, u32, u32)>>,
}

#[repr(C, align(16))]
struct Aligned(CONTEXT);

fn run(targets: &Targets, rx: mpsc::Receiver<()>) -> Result<Deep> {
    let t0 = host::now_ns();
    // A sampler preempted between a suspend and its resume stalls the target's thread with it: run
    // above normal threads (a 75 ms stall once at normal priority).
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let hz = tsc_hz();
    if !(hz.is_finite() && hz > 0.0) {
        return Err(Error::msg("strobe's own stack sampler could not measure the cycle clock's rate, so it cannot tell 2 ms of a thread's CPU"));
    }
    let per_sample = hz * INTERVAL_US as f64 / 1e6;
    let timer = unsafe { CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), 0x2, 0x1F0003) };
    let mut processes: HashMap<u32, Process> = HashMap::new();
    let mut threads: HashMap<u32, Thread> = HashMap::new();
    // Names read while the threads and processes lived: a child that ran and exited during the
    // capture is gone by its end, and its samples kept no name.
    let mut names: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
    let mut exes: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
    let mut busy: Vec<u32> = Vec::new();
    let mut stalls: Vec<f64> = Vec::new();
    let (mut capped, mut unopened) = (0u64, 0usize);
    let mut stacks: Vec<Vec<u64>> = Vec::new();
    let mut index: HashMap<Vec<u64>, u32> = HashMap::new();
    let mut samples = Vec::new();
    let mut last_refresh: Option<(std::time::Instant, HashMap<u32, u64>)> = None;
    let mut second = (std::time::Instant::now(), 0u64);
    let mut stack = vec![0u8; STACK_BYTES];
    // The capture's start on the clock threads are born by (FILETIME units).
    let born_t0 = filetime_now();
    // Each process's main thread, bound once when strobe first sees the process: its first thread,
    // created with it (within 5 ms). None when that thread is gone by then; a worker is never one.
    let mut mains: HashMap<u32, Option<u32>> = HashMap::new();
    // A process that joins a launch's job is told as it is created (`jobs::subscribe`, from before the
    // first look: one created while it was taken is told too), and opened at the next tick with the
    // threads it has, instead of at the next refresh (up to 100 ms later: a child of 150 ms showed 100
    // of them). Its threads are looked for again each tick over its first 50 ms (its first thread may
    // not exist when it is told, the loader starts more). Credit starts at the opening, as for any
    // thread; its CPU before counts toward coverage.
    let news = crate::host::jobs::subscribe();
    let mut fresh: Vec<(u32, std::time::Instant)> = Vec::new();
    // The set as one look saw it (`looker`): the first here, the rest from the looker's thread.
    let mut next_look = Some(look(targets));
    let looks = looker(targets.clone(), next_look.as_ref().map(|l| l.1.iter().map(|p| p.0).collect()).unwrap_or_default())?;
    // Cycles of the threads strobe followed, over the capture (coverage), and the CPU the snapshots
    // gave threads it never opened.
    let (mut followed_cycles, mut unopened_ns) = (0u64, 0u64);
    let mut never_opened: HashMap<u32, (u64, u64)> = HashMap::new();
    // Ticks on a 2 ms grid, not 2 ms after each tick's work: a timer that fires late and the work
    // between waits made the period 2.6 ms on adais, and a busy thread then got one sample per 2.6 ms
    // of its CPU, each counted as 2 ms (a thread busy the whole capture read as 76% covered).
    let tick = std::time::Duration::from_micros(INTERVAL_US);
    let mut due = std::time::Instant::now() + tick;
    // Samples' worth of CPU the cap of two left out: a thread that ran on while the sampler was late.
    let mut forgone = 0.0f64;
    // Cycles the threads begun during the capture used before strobe opened them.
    let mut late_cycles = 0u64;
    loop {
        match rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        // The latest look at the process set's threads, and which of them ran since the last look.
        // The first look opens every thread, so the capture's first 100 ms are sampled too.
        if let Some((taken, set, now)) = next_look.take().or_else(|| looks.try_iter().last()) {
            for (pid, created) in &set {
                mains.entry(*pid).or_insert_with(|| now.iter().find(|t| t.pid == *pid).filter(|t| t.born.abs_diff(*created) <= 50_000).map(|t| t.tid));
            }
            let cpu: HashMap<u32, u64> = now.iter().map(|t| (t.tid, t.cpu_ns)).collect();
            busy = now.iter().filter(|t| last_refresh.as_ref().is_none_or(|(_, before)| before.get(&t.tid).is_none_or(|c| *c < t.cpu_ns))).map(|t| t.tid).collect();
            // A thread opened since the look was taken (a job's child, told at once) is busy and live
            // whatever the look says: it may not be in it.
            let since: Vec<u32> = threads.iter().filter(|(tid, t)| t.opened >= taken && !busy.contains(tid)).map(|(tid, _)| *tid).collect();
            busy.extend(since);
            for t in &now {
                if threads.contains_key(&t.tid) || !busy.contains(&t.tid) {
                    continue;
                }
                // A thread strobe cannot open: its CPU from the snapshots, for coverage.
                let unopenable = |never: &mut HashMap<u32, (u64, u64)>| {
                    let e = never.entry(t.tid).or_insert((if t.born < born_t0 { t.cpu_ns } else { 0 }, t.cpu_ns));
                    e.1 = t.cpu_ns;
                };
                if let std::collections::hash_map::Entry::Vacant(e) = processes.entry(t.pid) {
                    // The process the look found, by its creation time on the handle (`open_process`).
                    let h = set.iter().find(|(p, _)| *p == t.pid).and_then(|&(p, c)| open_process(p, c)).unwrap_or(std::ptr::null_mut());
                    if h.is_null() || wow64(h) {
                        if !h.is_null() {
                            unsafe { CloseHandle(h) };
                        }
                        unopened += 1;
                        unopenable(&mut never_opened);
                        continue;
                    }
                    if let Some(exe) = image_name(h) {
                        exes.insert(t.pid, exe);
                    }
                    e.insert(Process { handle: h, modules: images(h), read_at: std::time::Instant::now(), empty_reads: 0, unwind: HashMap::new() });
                }
                // The thread the look found (`open_thread`).
                let Some(h) = open_thread(t.pid, t.tid, t.born) else {
                    unopened += 1;
                    unopenable(&mut never_opened);
                    continue;
                };
                let main = mains.get(&t.pid).copied().flatten() == Some(t.tid);
                let thread = Thread::new(t.pid, t.tid, h, t.born < born_t0, main, &mut names);
                late_cycles += thread.cycles - thread.base;
                threads.insert(t.tid, thread);
            }
            // A name set since it was opened, looked for over its first second; a thread that ended
            // keeps the name it had, and its last cycles count toward coverage.
            threads.retain(|tid, t| {
                if t.unnamed > 0 {
                    t.unnamed -= 1;
                    if let Some(n) = t.own_name() {
                        names.insert(*tid, n);
                        t.unnamed = 0;
                    }
                }
                let live = cpu.contains_key(tid) || t.opened >= taken;
                if !live {
                    followed_cycles += t.used();
                }
                live
            });
            for (tid, e) in never_opened.iter_mut() {
                if let Some(c) = cpu.get(tid) {
                    e.1 = e.1.max(*c);
                }
            }
            last_refresh = Some((taken, cpu));
        }
        // Processes told since the last tick: a child of one strobe holds (so of the set: its pid is
        // that process's while strobe holds a handle to it), with the creation time it was told by.
        for (pid, created) in news.try_iter() {
            if processes.contains_key(&pid) {
                continue;
            }
            if let Some(p) = open_child(pid, created, &processes) {
                if let Some(exe) = image_name(p.handle) {
                    exes.insert(pid, exe);
                }
                processes.insert(pid, p);
                fresh.push((pid, std::time::Instant::now()));
            }
        }
        // Over its first 50 ms, while it runs (an exit time set: nothing more to look for).
        fresh.retain(|(pid, at)| at.elapsed() < std::time::Duration::from_millis(50) && processes.get(pid).and_then(|p| times(p.handle)).is_some_and(|t| t.1 == 0));
        for &(pid, _) in &fresh {
            let Some(p) = processes.get(&pid) else { continue };
            let created = times(p.handle).map_or(0, |t| t.0);
            for h in next_threads(p.handle) {
                let tid = unsafe { windows_sys::Win32::System::Threading::GetThreadId(h) };
                let born = thread_born(h);
                if tid == 0 || threads.contains_key(&tid) || born.is_none() {
                    unsafe { CloseHandle(h) };
                    continue;
                }
                let born = born.unwrap_or(0);
                // Its first thread, created with it (within 5 ms), is its main one; a later one never is.
                let main = *mains.entry(pid).or_insert_with(|| (born.abs_diff(created) <= 50_000).then_some(tid)) == Some(tid);
                let thread = Thread::new(pid, tid, h, born < born_t0, main, &mut names);
                late_cycles += thread.cycles - thread.base;
                threads.insert(tid, thread);
                if !busy.contains(&tid) {
                    busy.push(tid);
                }
            }
        }
        if second.0.elapsed() >= std::time::Duration::from_secs(1) {
            second = (std::time::Instant::now(), 0);
        }
        for tid in &busy {
            let Some(t) = threads.get_mut(tid) else { continue };
            let mut cycles = 0;
            if unsafe { QueryThreadCycleTime(t.handle, &mut cycles) } == 0 {
                continue;
            }
            let credit = t.credit + cycles.saturating_sub(t.cycles) as f64 / per_sample;
            forgone += (credit - 2.0).max(0.0);
            t.credit = credit.min(2.0);
            t.cycles = cycles;
            if t.credit < 1.0 {
                continue;
            }
            t.credit -= 1.0;
            if second.1 >= MAX_RATE {
                capped += 1;
                continue;
            }
            let Some(p) = processes.get_mut(&t.pid) else { continue };
            let Some((at, stall, regs, len)) = take(t, p.handle, &mut stack) else { continue };
            second.1 += 1;
            stalls.push(stall);
            let frames = unwind(p, regs, &stack[..len]);
            if frames.is_empty() {
                continue;
            }
            let next = stacks.len() as u32;
            let at_stack = *index.entry(frames.clone()).or_insert_with(|| {
                stacks.push(frames);
                next
            });
            samples.push((at, t.pid, *tid, 0u16, at_stack));
        }
        // Wait until the next tick on a high-resolution timer (no change to the system's timer
        // resolution), or sleep where there is none. A tick whose time already passed (a preemption,
        // opening many threads) is not made up, and the next one is a whole interval away: a thread is never
        // suspended again moments after its last sample; the credit, capped at two samples, carries it.
        let now = std::time::Instant::now();
        if now >= due {
            due = now + tick;
        }
        let wait = due - now;
        due += tick;
        if timer.is_null() {
            std::thread::sleep(wait);
        } else {
            let at: i64 = -((wait.as_nanos() / 100).max(1) as i64);
            unsafe {
                SetWaitableTimer(timer, &at, 0, None, std::ptr::null(), 0);
                WaitForSingleObject(timer, 100);
            }
        }
    }
    if !timer.is_null() {
        unsafe { CloseHandle(timer) };
    }
    stalls.sort_by(f64::total_cmp);
    followed_cycles += threads.values().map(Thread::used).sum::<u64>();
    unopened_ns += never_opened.values().map(|(from, to)| to.saturating_sub(*from)).sum::<u64>();
    // The CPU the set's threads used over the capture as strobe saw them: the followed threads'
    // cycles and the snapshots' times of those it could not open. A thread that began and ended
    // between two looks is in neither.
    let cpu_ms = followed_cycles as f64 / hz * 1e3 + unopened_ns as f64 / 1e6;
    let stats = SuspendStats {
        cpu_ms: Some(cpu_ms),
        samples: samples.len() as u64,
        threads: samples.iter().map(|s| s.2).collect::<std::collections::BTreeSet<u32>>().len(),
        stall_us_mean: if stalls.is_empty() { 0.0 } else { stalls.iter().sum::<f64>() / stalls.len() as f64 },
        stall_us_p99: stalls.get((stalls.len() as f64 * 0.99) as usize).or(stalls.last()).copied().unwrap_or(0.0),
        stall_us_max: stalls.last().copied().unwrap_or(0.0),
        capped,
        unopened,
        forgone_ms: forgone * INTERVAL_US as f64 / 1e3,
        late_ms: late_cycles as f64 / hz * 1e3,
    };
    let mut modules = Vec::new();
    for (pid, p) in &processes {
        modules.extend(p.modules.iter().map(|m| Module { pid: *pid, base: m.base, size: m.size, path: m.path.clone() }));
    }
    let mut pids: Vec<u32> = processes.keys().copied().collect();
    pids.sort_unstable();
    for (tid, t) in &threads {
        if let Some(n) = t.own_name() {
            names.insert(*tid, n);
        }
    }
    let sampled: std::collections::BTreeSet<u32> = samples.iter().map(|s| s.2).collect();
    names.retain(|tid, _| sampled.contains(tid));
    Ok(Deep { interval_us: INTERVAL_US as f64, t0_ns: t0, t1_ns: host::now_ns(), modules, stacks, samples, threads: names, pids, processes: exes, suspend: Some(stats), ..Deep::default() })
}

/// One look at the target's set: when it began, the processes (pid, creation time) and every thread
/// of them, from one system snapshot.
type Look = (std::time::Instant, Vec<(u32, u64)>, Vec<host::ThreadInfo>);

fn look(targets: &Targets) -> Look {
    let taken = std::time::Instant::now();
    let (set, threads) = host::process_set_threads(targets.root, targets.home.as_deref());
    (taken, set, threads)
}

/// Looks at the set on a thread of its own, every 100 ms, and every 20 ms for a second after its
/// processes changed (a tree starting and ending children), so a short child is found early; the
/// sampler takes the latest look at its next tick. A look is a system snapshot (7.4 ms on adais):
/// taken on the sampler's own thread it skipped three or four ticks each time, and every busy thread
/// lost those samples (22% of a spawning tree's CPU). Ends at the first look after the sampler went.
fn looker(targets: Targets, first: Vec<u32>) -> Result<mpsc::Receiver<Look>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("strobe-stacks-set".into())
        .spawn(move || {
            let (mut pids, mut churn_until, mut last) = (first, std::time::Instant::now(), std::time::Instant::now());
            loop {
                let every = std::time::Duration::from_millis(if last < churn_until { 20 } else { 100 });
                std::thread::sleep((last + every).saturating_duration_since(std::time::Instant::now()));
                let l = look(&targets);
                last = l.0;
                let now: Vec<u32> = l.1.iter().map(|p| p.0).collect();
                if now != pids {
                    churn_until = last + std::time::Duration::from_secs(1);
                    pids = now;
                }
                if tx.send(l).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| Error::msg(format!("the stack sampler's set thread: {e}")))?;
    Ok(rx)
}

/// The access strobe holds a thread with to sample it.
const THREAD_ACCESS: u32 = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;

/// A process told as it joined a launch's job (`jobs::subscribe`), opened to sample: the one created
/// at `created` (its pid not yet another's), whose parent strobe holds. None otherwise, or where it
/// cannot be opened (the next refresh then finds it as any process, and counts what it cannot open).
fn open_child(pid: u32, created: u64, processes: &HashMap<u32, Process>) -> Option<Process> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if h.is_null() {
        return None;
    }
    let ours = times(h).is_some_and(|t| t.0 == created) && !wow64(h) && parent(h).is_some_and(|p| processes.contains_key(&p));
    if !ours {
        unsafe { CloseHandle(h) };
        return None;
    }
    Some(Process { handle: h, modules: images(h), read_at: std::time::Instant::now(), empty_reads: 0, unwind: HashMap::new() })
}

/// A process's parent's pid (`InheritedFromUniqueProcessId`).
fn parent(process: HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Query = unsafe extern "system" fn(HANDLE, i32, *mut std::ffi::c_void, u32, *mut u32) -> i32;
    let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    let query: Query = unsafe { std::mem::transmute(GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), c"NtQueryInformationProcess".as_ptr() as *const u8)?) };
    // PROCESS_BASIC_INFORMATION: ExitStatus, PebBaseAddress, AffinityMask, BasePriority,
    // UniqueProcessId, then InheritedFromUniqueProcessId at 40.
    let mut info = [0u64; 6];
    (unsafe { query(process, 0, info.as_mut_ptr() as *mut _, 48, std::ptr::null_mut()) } >= 0).then_some(info[5] as u32)
}

/// Every thread of a process now, oldest first, each opened to sample (`NtGetNextThread`: no system
/// snapshot). The caller closes the handles it does not keep.
fn next_threads(process: HANDLE) -> Vec<HANDLE> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Next = unsafe extern "system" fn(HANDLE, HANDLE, u32, u32, u32, *mut HANDLE) -> i32;
    let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    let Some(f) = (unsafe { GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), c"NtGetNextThread".as_ptr() as *const u8) }) else { return Vec::new() };
    let next: Next = unsafe { std::mem::transmute(f) };
    let mut out: Vec<HANDLE> = Vec::new();
    loop {
        let mut h: HANDLE = std::ptr::null_mut();
        let at = out.last().copied().unwrap_or(std::ptr::null_mut());
        if unsafe { next(process, at, THREAD_ACCESS, 0, 0, &mut h) } < 0 || h.is_null() {
            break;
        }
        out.push(h);
    }
    out
}

/// Opens process `pid` to read its memory, only while it is the process created at `created` (FILETIME
/// units, as the look gave it), checked on the handle itself: a pid another process took since the
/// look is closed again, never read (`host::kill`, row 21).
fn open_process(pid: u32, created: u64) -> Option<HANDLE> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if h.is_null() {
        return None;
    }
    if times(h).is_some_and(|t| t.0 == created) {
        Some(h)
    } else {
        unsafe { CloseHandle(h) };
        None
    }
}

/// Opens thread `tid` to suspend, read and resume it, only while it is a thread of process `pid`
/// created at `born` (0: the look gave no time, not checked), both checked on the handle: a thread id
/// another app's thread took since the look is closed again and never suspended (`host::kill`, row 21).
fn open_thread(pid: u32, tid: u32, born: u64) -> Option<HANDLE> {
    let h = unsafe { OpenThread(THREAD_ACCESS, 0, tid) };
    if h.is_null() {
        return None;
    }
    if unsafe { windows_sys::Win32::System::Threading::GetProcessIdOfThread(h) } == pid && (born == 0 || thread_born(h) == Some(born)) {
        Some(h)
    } else {
        unsafe { CloseHandle(h) };
        None
    }
}

/// For the dev harness (`scripts/dev/bounce`): whether the sampler would open process `pid` created at
/// `created` and its thread `tid` created at `born` now (each handle closed again at once).
pub fn would_open(pid: u32, created: u64, tid: u32, born: u64) -> (bool, bool) {
    let close = |h: Option<HANDLE>| {
        h.inspect(|&h| {
            unsafe { CloseHandle(h) };
        })
        .is_some()
    };
    (close(open_process(pid, created)), close(open_thread(pid, tid, born)))
}

/// A process's creation and exit times (FILETIME units; exit 0 while it runs).
fn times(process: HANDLE) -> Option<(u64, u64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    let t = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = unsafe { std::mem::zeroed() };
    (unsafe { GetProcessTimes(process, &mut c, &mut e, &mut k, &mut u) } != 0).then(|| (t(c), t(e)))
}

/// When a thread was created (FILETIME units).
fn thread_born(thread: HANDLE) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetThreadTimes;
    let t = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = unsafe { std::mem::zeroed() };
    (unsafe { GetThreadTimes(thread, &mut c, &mut e, &mut k, &mut u) } != 0).then(|| t(c))
}

impl Thread {
    /// A thread just opened (`handle`, held from now): credit from now on, its cycles counted for
    /// coverage from now (`before`: it existed before the capture began) or from its start, its name
    /// read (`(main)` for its process's first thread, which names itself rarely).
    fn new(pid: u32, tid: u32, handle: HANDLE, before: bool, main: bool, names: &mut std::collections::BTreeMap<u32, String>) -> Thread {
        let mut cycles = 0;
        unsafe { QueryThreadCycleTime(handle, &mut cycles) };
        let mut thread = Thread { pid, handle, cycles, base: if before { cycles } else { 0 }, credit: 0.0, stack_base: None, unnamed: 10, opened: std::time::Instant::now() };
        match thread.own_name() {
            Some(n) => {
                names.insert(tid, n);
                thread.unnamed = 0;
            }
            None if main => {
                names.insert(tid, "(main)".into());
            }
            None => {}
        }
        thread
    }

    /// Its own name (`SetThreadDescription`), read on the handle strobe holds; None when it has none
    /// or it can no longer be read.
    fn own_name(&self) -> Option<String> {
        use windows_sys::Win32::System::Threading::GetThreadDescription;
        unsafe {
            let mut text: *mut u16 = std::ptr::null_mut();
            if GetThreadDescription(self.handle, &mut text) >= 0 && !text.is_null() {
                let mut n = 0;
                while *text.add(n) != 0 {
                    n += 1;
                }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
                windows_sys::Win32::Foundation::LocalFree(text as _);
                Some(s).filter(|s| !s.is_empty())
            } else {
                None
            }
        }
    }

    /// Its cycles over the capture: from `base` to now (a thread that ended keeps its count while
    /// the handle is open).
    fn used(&self) -> u64 {
        let mut cycles = 0;
        let now = if unsafe { QueryThreadCycleTime(self.handle, &mut cycles) } != 0 { cycles.max(self.cycles) } else { self.cycles };
        now.saturating_sub(self.base)
    }
}

/// Now on the clock threads and processes are born by (FILETIME units, 100 ns).
fn filetime_now() -> u64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    let mut f = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    unsafe { GetSystemTimePreciseAsFileTime(&mut f) };
    (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
}

/// A process's executable, its file name.
fn image_name(process: HANDLE) -> Option<String> {
    use windows_sys::Win32::System::Threading::{PROCESS_NAME_WIN32, QueryFullProcessImageNameW};
    let mut buf = vec![0u16; 1024];
    let mut n = buf.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) } == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..n as usize]);
    path.rsplit(['\\', '/']).next().map(str::to_string).filter(|s| !s.is_empty())
}

/// One sample: suspend, registers, the stack from RSP up, resume. (host ns, stall µs, registers,
/// bytes of stack copied.)
fn take(t: &mut Thread, process: HANDLE, stack: &mut [u8]) -> Option<(u64, f64, [u64; 17], usize)> {
    let base = match t.stack_base {
        Some(b) => b,
        None => {
            let b = stack_base(t.handle, process)?;
            t.stack_base = Some(b);
            b
        }
    };
    let mut ctx: Aligned = unsafe { std::mem::zeroed() };
    ctx.0.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
    let at = host::now_ns();
    if unsafe { SuspendThread(t.handle) } == u32::MAX {
        return None;
    }
    let ok = unsafe { GetThreadContext(t.handle, &mut ctx.0) } != 0;
    let c = &ctx.0;
    let len = if ok { (base.saturating_sub(c.Rsp) as usize).min(stack.len()) } else { 0 };
    let mut read = 0usize;
    if len > 0 {
        unsafe { ReadProcessMemory(process, c.Rsp as *const _, stack.as_mut_ptr() as *mut _, len, &mut read) };
    }
    unsafe { ResumeThread(t.handle) };
    let stall = (host::now_ns() - at) as f64 / 1e3;
    if !ok {
        return None;
    }
    let regs = [c.Rax, c.Rcx, c.Rdx, c.Rbx, c.Rsp, c.Rbp, c.Rsi, c.Rdi, c.R8, c.R9, c.R10, c.R11, c.R12, c.R13, c.R14, c.R15, c.Rip];
    Some((at, stall, regs, read))
}

/// The top of a thread's stack, from its TEB (`NT_TIB.StackBase`).
fn stack_base(thread: HANDLE, process: HANDLE) -> Option<u64> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Query = unsafe extern "system" fn(HANDLE, i32, *mut std::ffi::c_void, u32, *mut u32) -> i32;
    let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    let query: Query = unsafe { std::mem::transmute(GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), c"NtQueryInformationThread".as_ptr() as *const u8)?) };
    // THREAD_BASIC_INFORMATION: ExitStatus, then TebBaseAddress at 8.
    let mut info = [0u64; 6];
    if unsafe { query(thread, 0, info.as_mut_ptr() as *mut _, 48, std::ptr::null_mut()) } < 0 {
        return None;
    }
    read_u64(process, info[1] + 8)
}

fn read_u64(process: HANDLE, at: u64) -> Option<u64> {
    let mut v = 0u64;
    let mut n = 0usize;
    (unsafe { ReadProcessMemory(process, at as *const _, &mut v as *mut u64 as *mut _, 8, &mut n) } != 0 && n == 8).then_some(v)
}

fn read_bytes(process: HANDLE, at: u64, len: usize) -> Option<Vec<u8>> {
    let mut v = vec![0u8; len];
    let mut n = 0usize;
    (unsafe { ReadProcessMemory(process, at as *const _, v.as_mut_ptr() as *mut _, len, &mut n) } != 0 && n == len).then_some(v)
}

fn wow64(process: HANDLE) -> bool {
    use windows_sys::Win32::System::Threading::IsWow64Process;
    let mut w = 0;
    unsafe { IsWow64Process(process, &mut w) != 0 && w != 0 }
}

/// The process's modules (base, size, path); their function tables are read when first needed.
fn images(process: HANDLE) -> Vec<Image> {
    use windows_sys::Win32::System::ProcessStatus::{K32EnumProcessModulesEx, K32GetModuleFileNameExW, K32GetModuleInformation, LIST_MODULES_ALL, MODULEINFO};
    let mut out = Vec::new();
    let mut mods = vec![std::ptr::null_mut(); 2048];
    let mut need = 0u32;
    unsafe {
        if K32EnumProcessModulesEx(process, mods.as_mut_ptr(), (mods.len() * 8) as u32, &mut need, LIST_MODULES_ALL) == 0 {
            return out;
        }
        for &m in mods.iter().take((need as usize / 8).min(mods.len())) {
            let mut info: MODULEINFO = std::mem::zeroed();
            let mut name = vec![0u16; 1024];
            let n = K32GetModuleFileNameExW(process, m, name.as_mut_ptr(), name.len() as u32);
            if n > 0 && K32GetModuleInformation(process, m, &mut info, std::mem::size_of::<MODULEINFO>() as u32) != 0 {
                out.push(Image { base: info.lpBaseOfDll as u64, size: info.SizeOfImage as u64, path: String::from_utf16_lossy(&name[..n as usize]), functions: None });
            }
        }
    }
    out
}

/// A module's `RUNTIME_FUNCTION` table (the exception directory), from its headers in memory.
fn functions(process: HANDLE, base: u64) -> Vec<(u32, u32, u32)> {
    let read = || -> Option<Vec<(u32, u32, u32)>> {
        let pe = base + read_bytes(process, base + 0x3C, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))? as u64;
        // PE32+ optional header at pe + 24; its data directories at +112; the exception one is 3rd.
        let dir = read_bytes(process, pe + 24 + 112 + 3 * 8, 8)?;
        let (rva, size) = (u32::from_le_bytes([dir[0], dir[1], dir[2], dir[3]]), u32::from_le_bytes([dir[4], dir[5], dir[6], dir[7]]));
        if rva == 0 || size < 12 {
            return Some(Vec::new());
        }
        let raw = read_bytes(process, base + rva as u64, size as usize - size as usize % 12)?;
        let mut f: Vec<(u32, u32, u32)> = raw
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| (u32::from_le_bytes([c[0], c[1], c[2], c[3]]), u32::from_le_bytes([c[4], c[5], c[6], c[7]]), u32::from_le_bytes([c[8], c[9], c[10], c[11]])))
            .collect();
        f.sort_unstable();
        Some(f)
    };
    read().unwrap_or_default()
}

/// The stack as return addresses, innermost first: x64 unwinding over the copied stack (from
/// `regs[4]`, RSP) with each module's unwind data, until an address outside any module, outside the
/// copy, or 128 frames.
fn unwind(p: &mut Process, mut regs: [u64; 17], stack: &[u8]) -> Vec<u64> {
    let lo = regs[4];
    let mem = |at: u64| -> Option<u64> {
        let off = at.checked_sub(lo)? as usize;
        stack.get(off..off + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()))
    };
    let mut frames = Vec::new();
    for depth in 0..128 {
        let ip = regs[16];
        if ip == 0 {
            break;
        }
        frames.push(ip);
        // A caller's address is a return address: look up the call instruction before it.
        let at = if depth == 0 { ip } else { ip - 1 };
        let found = p.modules.iter().position(|m| at >= m.base && at < m.base + m.size);
        let i = match found {
            Some(i) => i,
            // A module loaded since the list was read, or a list that was empty: read it again.
            None if p.reread_due() => {
                p.reread();
                match p.modules.iter().position(|m| at >= m.base && at < m.base + m.size) {
                    Some(i) => i,
                    None => break,
                }
            }
            None => break,
        };
        let (handle, base) = (p.handle, p.modules[i].base);
        let table = p.modules[i].functions.get_or_insert_with(|| functions(handle, base));
        let rva = (at - base) as u32;
        let f = match table.binary_search_by(|f| {
            if rva < f.0 {
                std::cmp::Ordering::Greater
            } else if rva >= f.1 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(k) => Some(table[k]),
            Err(_) => None,
        };
        let ok = match f {
            // A leaf function (no unwind data): the return address is at RSP.
            None => pop_return(&mut regs, &mem),
            Some((begin, _, info)) => {
                (depth == 0 && epilog(handle, &mut regs, &mem)) || unwind_info(p, base, info, rva - begin, true, &mut regs, &mem).is_some_and(|machine| machine || pop_return(&mut regs, &mem))
            }
        };
        if !ok || regs[4] < lo {
            break;
        }
    }
    frames
}

fn pop_return(regs: &mut [u64; 17], mem: &dyn Fn(u64) -> Option<u64>) -> bool {
    let Some(ret) = mem(regs[4]) else { return false };
    regs[16] = ret;
    regs[4] += 8;
    true
}

/// Applies a function's unwind codes (those whose prolog instruction already ran, for the primary
/// entry), then a chained entry's. Some(true) when a machine frame set RIP and RSP already.
fn unwind_info(p: &mut Process, base: u64, rva: u32, offset: u32, primary: bool, regs: &mut [u64; 17], mem: &dyn Fn(u64) -> Option<u64>) -> Option<bool> {
    let at = base + rva as u64;
    let handle = p.handle;
    let info = p.unwind.entry(at).or_insert_with(|| {
        let head = read_bytes(handle, at, 4)?;
        let (flags, count) = (head[0] >> 3, head[2] as usize);
        let len = count * 2 + if flags & 4 != 0 { 12 + (count % 2) * 2 } else { 0 };
        let mut all = head;
        all.extend(if len > 0 { read_bytes(handle, at + 4, len)? } else { Vec::new() });
        Some(all)
    });
    let info = info.clone()?;
    let (version, flags, count, frame) = (info[0] & 7, info[0] >> 3, info[2] as usize, info[3]);
    let codes = &info[4..];
    let slot = |k: usize| u16::from_le_bytes([codes[2 * k], codes[2 * k + 1]]);
    let mut k = 0;
    while k < count {
        let (at, op, info) = (codes[2 * k] as u32, codes[2 * k + 1] & 0xF, (codes[2 * k + 1] >> 4) as usize);
        let slots = match op {
            0 | 2 | 3 | 10 => 1,
            1 => {
                if info == 0 {
                    2
                } else {
                    3
                }
            }
            4 | 8 => 2,
            5 | 9 => 3,
            6 => {
                if version >= 2 {
                    1
                } else {
                    2
                }
            }
            7 => {
                if version >= 2 {
                    2
                } else {
                    3
                }
            }
            _ => return None,
        };
        if k + slots > count {
            return None;
        }
        if !primary || at <= offset {
            match op {
                0 => {
                    regs[info] = mem(regs[4])?;
                    regs[4] += 8;
                }
                1 => regs[4] += if info == 0 { slot(k + 1) as u64 * 8 } else { slot(k + 1) as u64 | (slot(k + 2) as u64) << 16 },
                2 => regs[4] += info as u64 * 8 + 8,
                3 => regs[4] = regs[(frame & 0xF) as usize].wrapping_sub(16 * (frame >> 4) as u64),
                4 => regs[info] = mem(regs[4] + slot(k + 1) as u64 * 8)?,
                5 => regs[info] = mem(regs[4] + (slot(k + 1) as u64 | (slot(k + 2) as u64) << 16))?,
                10 => {
                    if info == 1 {
                        regs[4] += 8;
                    }
                    regs[16] = mem(regs[4])?;
                    regs[4] = mem(regs[4] + 24)?;
                    return Some(true);
                }
                _ => {}
            }
        }
        k += slots;
    }
    if flags & 4 != 0 {
        let at = (count + count % 2) * 2;
        let chained = u32::from_le_bytes([codes[at + 8], codes[at + 9], codes[at + 10], codes[at + 11]]);
        return unwind_info(p, base, chained, 0, false, regs, mem);
    }
    Some(false)
}

/// When RIP is in an epilog (`add rsp`/`lea rsp`, pops, then `ret` or a tail `jmp`), runs the rest
/// of it as RtlVirtualUnwind does; false when it is not in one.
fn epilog(process: HANDLE, regs: &mut [u64; 17], mem: &dyn Fn(u64) -> Option<u64>) -> bool {
    let Some(code) = read_bytes(process, regs[16], 32) else { return false };
    let mut r = *regs;
    let mut i = 0;
    let rex = |b: u8| b & 0xF8 == 0x48;
    // The first instruction may restore RSP.
    if code.len() >= 4 && rex(code[0]) && code[1] == 0x83 && code[2] == 0xC4 {
        r[4] = r[4].wrapping_add(code[3] as i8 as i64 as u64);
        i = 4;
    } else if code.len() >= 7 && rex(code[0]) && code[1] == 0x81 && code[2] == 0xC4 {
        r[4] = r[4].wrapping_add(i32::from_le_bytes([code[3], code[4], code[5], code[6]]) as i64 as u64);
        i = 7;
    } else if code.len() >= 4 && rex(code[0]) && code[1] == 0x8D && (code[2] >> 3) & 7 == 4 && code[2] >> 6 != 0 && code[2] >> 6 != 3 {
        // lea rsp, [reg + disp]
        let base = (code[2] & 7) as usize + if code[0] & 1 != 0 { 8 } else { 0 };
        let mut at = 3;
        if code[2] & 7 == 4 {
            at += 1; // SIB (rsp base)
        }
        let disp = if code[2] >> 6 == 1 { code[at] as i8 as i64 } else { i32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]]) as i64 };
        r[4] = r[base].wrapping_add(disp as u64);
        i = at + if code[2] >> 6 == 1 { 1 } else { 4 };
    }
    loop {
        match code.get(i..) {
            Some([0x58..=0x5F, ..]) => {
                let reg = (code[i] - 0x58) as usize;
                let Some(v) = mem(r[4]) else { return false };
                r[reg] = v;
                r[4] += 8;
                i += 1;
            }
            Some([0x41, 0x58..=0x5F, ..]) => {
                let reg = (code[i + 1] - 0x58) as usize + 8;
                let Some(v) = mem(r[4]) else { return false };
                r[reg] = v;
                r[4] += 8;
                i += 2;
            }
            // A tail call's jump ends an epilog only after something of one: alone it is a branch.
            Some([0xC3, ..]) | Some([0xC2, ..]) => return finish(regs, r, mem),
            Some([0xE9, ..]) | Some([0x48, 0xFF, 0x25, ..]) | Some([0xFF, 0x25, ..]) if i > 0 => return finish(regs, r, mem),
            _ => return false,
        }
    }
}

/// The epilog's return: RIP from the stack.
fn finish(regs: &mut [u64; 17], mut r: [u64; 17], mem: &dyn Fn(u64) -> Option<u64>) -> bool {
    let Some(ret) = mem(r[4]) else { return false };
    r[16] = ret;
    r[4] += 8;
    *regs = r;
    true
}

/// The time stamp counter's rate (cycles per second), measured over 20 ms; 0 without one.
fn tsc_hz() -> f64 {
    let (Some(c0), t0) = (host::tsc(), host::now_ns()) else { return 0.0 };
    std::thread::sleep(std::time::Duration::from_millis(20));
    let (Some(c1), t1) = (host::tsc(), host::now_ns()) else { return 0.0 };
    (c1.saturating_sub(c0)) as f64 / ((t1 - t0) as f64 / 1e9)
}
