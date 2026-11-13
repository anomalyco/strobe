//! Ending processes. Every signal strobe sends to a process it did not just spawn goes through here.
//!
//! THE STANDARD. Read it before you add or change anything that ends a process, on any OS:
//!
//! strobe sends a signal to a process only when, at the moment of signalling, it holds proof that the
//! process is one it launched:
//! - the process's id and the start time strobe captured when it first identified it, read again
//!   right before the signal (a pid names one process only together with its start time);
//! - where the process's environment is readable, strobe's own stamp in it (`spawn::HOME_VAR` and
//!   `spawn::OWNER_VAR`, `spawn::stamp_of`). "The environment is readable and carries no stamp" is proof
//!   that the process is NOT strobe's, never a reason to go on;
//! - a process group is signalled only when strobe created it: the group of strobe's own child that
//!   strobe has not reaped (Unix: until the reap, neither its pid nor its group id can name another
//!   process). Never the group of a process strobe did not parent, even one it proved its own;
//! - what cannot be proven is left alone and reported (`spawn::left_alone`).
//!
//! What a launch started is the launch's when it is proven from the launch: a process in its tree
//! (from the same snapshot: its parent proven, and it started no earlier than that parent) or, on
//! macOS, one responsible to the launch (the responsible process named by its pid AND start time,
//! `macos::kill_set`). Each is checked again by its own start time right before its signal.
//!
//! A start time read at clean-up time proves nothing by itself: only the one captured when strobe
//! identified the process (at the launch), or one read in the same read as the proof itself (a stamp,
//! a group or job membership, a place in a tree walked from a root proven by its launch-time start, in
//! the snapshot that orders parent before child), names the process strobe proved. The table says
//! where each start time comes from.
//!
//! What is left between the proof and the signal, by OS:
//! - Windows: nothing. The creation time is read through the handle the process is then terminated
//!   with (`win::kill_tree`), and a process strobe holds a handle to keeps its pid.
//! - Linux: nothing where the kernel has pidfds (5.3 and later): the pidfd is opened, the proof read,
//!   the process checked to be still there through the pidfd, and the signal sent through it
//!   (`signal_if`). A pid that is now another process's thread (`EINVAL`), gone (`ESRCH`), or a
//!   pidfd that cannot be had for want of descriptors or memory, gets no signal (the last is reported).
//!   Only where pidfds are missing or blocked (`ENOSYS`, `EPERM`): two system calls, the signal sent
//!   with `tgkill(pid, pid)`, which the kernel delivers only while the pid is its own thread group's
//!   leader (never to another process's thread). Start times count clock ticks (10 ms), so two
//!   processes can share a pid and a start time only if the pid space wraps within one tick.
//! - macOS: two system calls (`proc_pidinfo`, `kill`; macOS has no pidfd). Start times are µs.
//!
//! Every place strobe ends or signals a process, a group, a job or an app, with what it holds (the
//! census; the hand-off in AUDIT-FINDINGS keeps it too). "Start" says where the start time compared
//! right before the signal was read.
//!
//! | # | Path | Signals | Start | Proof at the signal |
//! |---|---|---|---|---|
//! | 1 | `kill_launch(pid, start)` | root, its tree, `-pid` | at the spawn (`Owned::start`); tree members from the walk's snapshot | strobe's own child, unreaped (Unix) or handle held (Windows); with `start`, nothing once it names another process; each member by its start, re-read (Linux: through a pidfd) right before |
//! | 2 | `kill_opened(pid, start)` | root, its tree (macOS: what is responsible to it by pid and start); never a group | the caller's (see each row) | root only at `start`, read again after the walk's list (a root that exited during it lends no children; macOS still ends what is responsible to it by pid and start); each member by its snapshot start, re-read right before; a reused root's tree left alone |
//! | 3 | `win::kill_tree` | `TerminateProcess` per tree member | the snapshot's creation time | the creation time read through the handle it terminates with; with a start, the root must have it |
//! | 4 | `spawn::end_launches` (a stop) | child: `kill_launch` + `-pgid`; opened app: `kill_opened`; then every launch cgroup (`cgroup::end_all`, rows 14-15) or job (`TerminateJobObject`); `isolate::end_on_stop` then ends what carries each launch's stamp (`end_roots`) and says what is still running 2 s on | child: at the spawn; opened app: `macos::launched` at the launch | child unreaped; opened app only with its launch-time start (an unadopted one has pid 0 and nothing is signalled); a cgroup or job strobe made; a stamp re-read with the start time right before |
//! | 5 | `Owned::end_group` | as 4, then `TerminateJobObject`, `Child::kill` | as 4 | under the `LIVE` lock, before the reap |
//! | 6 | `close_job`, a job's last handle (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), `tty` Windows | every process of the job | - | a job strobe made, by its handle |
//! | 7 | `Owned::run_to_exit` watchdog | `-pid` at the deadline | - | the child is unreaped until `wait4`; on a wait error the watchdog is joined and the group forgotten before returning |
//! | 8 | `Owned::run_to_exit` sweep (read once at the exit, no grace) | `-pid`, then `kill_opened` per leftover | group members: in the same read as their group (`group_members`); stamped: with the stamp (`stamped_in`, bracketed); Windows job members and Linux launch-cgroup members: none | child unreaped; a member's group or stamp and its start from one read; job members only by the job's end, cgroup members only by the cgroup's end (row 14) |
//! | 9 | `isolate::end_roots` (`release_home`, `release_gone`, `reap_dead_owners`, a failed launch, `live`) | `kill_opened` per stamped root | the stamp scan's, bracketed around the stamp read | start, stamp, start again right before; never a group |
//! | 10 | `macos::open_app` at a stop | `kill_opened` | `macos::launched` (after the request, stamp or same user and executable) | proven, or left alone and reported |
//! | 11 | `webkit::end_browser` | SIGTERM (`sigterm_opened`: through `signal_if`, a pidfd on Linux), then `kill_opened` | `browser_proof` at the launch (after the request, the browser's executable, automation or the driver's tree) | SIGTERM only while it has that start; `kill_opened` ends what the browser left, never a reused pid's process |
//! | 12 | `webkit::Browser::end_with` | `kill_opened` per process of `snapshot` | the walk's snapshot, from the browser's pid only while it has its launch-time start, checked before and after | proven root; members by their snapshot start, re-read right before |
//! | 13 | `webkit` `Started` (an error before the browser was kept) | `end_browser` | the proven pair once `browser_proof` passed at the launch (ended by it even if the browser exited first); else `browser_proof` read at the clean-up against the session request's time | the launch's own proof, bracketed |
//! | 14 | `cgroup::end` (Linux 5.14 and later) | `cgroup.kill` | - | a cgroup strobe made (`launch-<pid>-<start>-<n>`); a dead owner's by its pid and start |
//! | 15 | `cgroup::end` (before 5.14) | SIGKILL per listed pid | - | membership of the cgroup read right before (`signal_if`: through a pidfd where there is one, else `tgkill(pid, pid)`: only to a group leader, checked by the kernel at delivery) |
//! | 16 | `tty` Windows `Proc::end` | `kill_launch(pid, None)`, `TerminateJobObject`, `ClosePseudoConsole` | - | its process handle is held until after, so its pid is its own; the job and console are strobe's |
//! | 17 | `tty` Unix: the pty master dropped (SIGHUP), `TIOCSWINSZ` (SIGWINCH) | the pty's foreground group | - | a pty strobe opened for its own child; the kernel signals only that terminal's group |
//! | 18 | `probe::sample::Capture::{stop, drop}` (macOS) | SIGINT, `Child::kill` | - | its own `sample` child while `try_wait` says it is not reaped |
//! | 19 | `Child::kill` (`engine`, `ffmpeg`, `track` alert shell, `macos::unregister`, `macos::ls_registered`, `spawn`) | own child | - | std signals none it reaped; each is waited for after |
//! | 20 | `unix_ctrl_c` (a third SIGINT or SIGTERM, 300 ms or more after the second; a repeat within 300 ms is the same stop, and the second is a stop now: `spawn::stop_launches`, row 4, then exit 130) | strobe itself (`raise`) | - | itself |
//! | 21 | `probe::suspend` (Windows x64): stack sampling | `SuspendThread`, `ResumeThread`, memory reads | the look's snapshot (the set walked from the launch's root): each process's and thread's creation time | each process handle kept only while its creation time is the look's (`open_process`), each thread handle only while it is a thread of that pid created at the look's time (`open_thread`), both checked on the handle; else closed and counted unopened. A job's new process is opened by its parent's handle and creation time (`open_child`) |
//! | 22 | `android_chrome` (`adb shell am force-stop`) | a Chrome channel on a phone | - | never the stable channel; only a channel strobe started or one opted in with no tabs of the person's |
//! | 23 | `helper` (`schtasks /end`) | strobe's ETW helper task | - | the task name carries the user's RID; strobe's own |
//! | 24 | `up` / `leave` / `update` (`launchctl kickstart -k`, `bootout`, `systemctl --user restart`), MCP `strobe/quit` | the strobe daemon | - | the daemon's own service unit; `strobe/quit` needs the token and no bench running |
//! | 25 | protocol and window closes (CDP `Browser.close`, BiDi `browser.close`, WebDriver `DELETE /session`, the script host's `exit`, `page.window("close")`, `WM_CLOSE`) | a browser, host or app strobe launched | - | strobe's own session; a window close only on windows of a process set walked from a proven root (`Owned::root`, `process_set_of`): a foreign Launch Services answer fails the launch first |
//! | 26 | `sdk/bench/bin/strobe.js` | Unix: forward SIGINT/SIGTERM to its child; after exit re-raise the child's signal on the wrapper | - | the wrapper's unreaped `ChildProcess`; then itself. Windows console Ctrl-C/Break reaches both directly |
//! | 27 | `scripts/dev` (`prune-auto.ts`, `viewer-units.ts`) | their own children | - | the child's handle |
//! | 28 | `scripts/dev/prune-check.ts` | a holder's group | - | only while the holder has no exit code or signal (not reaped) |
//! | 29 | `scripts/dev/bounce/harness.rs` | decoys it started | from the decoy tree's snapshot while the decoy (its unreaped child) ran | `kill_opened` by pid and that start; the decoy itself through its handle |
/// Ends strobe's own child `pid` and its tree now: the group it leads (Unix), then each process of its
/// tree, children first, each only while its start time is the one the tree was read with (Windows:
/// terminated through a handle whose creation time is checked). The caller's proof is that `pid` is
/// its child, not yet reaped on Unix and with its handle held on Windows, so its pid names it; `start`
/// is its start time read at the launch, and with it nothing at all is signalled once the pid names a
/// process started at another time. On macOS a child that exited (not reaped) still roots what it left
/// running that is responsible to it (`macos::kill_set`).
pub fn kill_launch(pid: u32, start: Option<u64>) {
    end_tree(pid, start, true);
}

/// Ends a process strobe proved its own but did not parent (`pid` and the `start` time read when strobe
/// identified it, with its stamp where its environment is readable): its tree, each process by its
/// start time, and on macOS what is responsible to it. Never a process group: strobe made none. Once
/// `pid` names a process started at another time, that process and its tree are left alone; on macOS
/// what the launch left running that started before the reuse (and is responsible to the launch's own
/// pid and start time) is still ended.
pub fn kill_opened(pid: u32, start: u64) {
    end_tree(pid, Some(start), false);
}

fn end_tree(pid: u32, start: Option<u64>, group: bool) {
    if pid == 0 || pid == std::process::id() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = group;
        super::win::kill_tree(pid, start);
    }
    #[cfg(unix)]
    {
        // Whether `pid` still names the process the caller proved: its own child when no start time is
        // given, else the one started at `start`.
        let root = start.is_none_or(|s| super::start_time(pid) == Some(s));
        // (pid, start time) of each process to end, root first, from one snapshot.
        #[cfg(target_os = "macos")]
        let set: Vec<(u32, u64)> = super::macos::kill_set(pid, start).into_iter().filter(|&(p, _)| root || p != pid).collect();
        #[cfg(not(target_os = "macos"))]
        let set: Vec<(u32, u64)> = if root { super::linux::tree_starts(pid, start) } else { Vec::new() };
        if group && root && unsafe { libc::getpgid(pid as i32) } == pid as i32 && start.is_none_or(|s| super::start_time(pid) == Some(s)) {
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        for &(p, s) in set.iter().rev() {
            signal(p, s);
        }
    }
}

/// SIGKILL to `pid` only while it is the process started at `start` (`signal_if`).
#[cfg(unix)]
fn signal(pid: u32, start: u64) {
    signal_if(pid, libc::SIGKILL, || super::start_time(pid) == Some(start));
}

/// SIGTERM to a process strobe proved its own but did not parent (`pid`, the `start` time proven when
/// strobe identified it), only while it has that start time: the polite end before `kill_opened` (the
/// browser a WebDriver server started, `webkit::end_browser`). Whether it was sent.
#[cfg(unix)]
pub fn sigterm_opened(pid: u32, start: u64) -> bool {
    signal_if(pid, libc::SIGTERM, || super::start_time(pid) == Some(start))
}

/// `sig` to `pid` only while `proof` holds of it, read right before the signal; never to pid 0, an id
/// out of range, or strobe itself. Linux: through a pidfd where the kernel has one (5.3 and later): the
/// pidfd names the process that held the pid when it was opened; `proof` is read after that, and the
/// process is then checked to be still there through the pidfd (signal 0: a pid can name another
/// process only once the first is reaped), so what `proof` read was that process; the signal goes
/// through the pidfd, to that process or to none. The signal by pid (`plain_signal`) is used only where
/// the facility is missing or blocked (no `pidfd_open`: an old kernel, a seccomp filter). Elsewhere
/// (macOS): `proof` is read and the signal sent by pid, two system calls apart. Whether it was sent.
#[cfg(unix)]
pub(crate) fn signal_if(pid: u32, sig: i32, proof: impl Fn() -> bool) -> bool {
    if pid == 0 || pid > i32::MAX as u32 || pid == std::process::id() {
        return false;
    }
    #[cfg(target_os = "linux")]
    if let Some(sent) = pidfd_signal(pid, sig, &proof) {
        return sent;
    }
    plain_signal(pid, sig, &proof)
}

/// `sig` to `pid` by its id once `proof` holds. Linux: `tgkill(pid, pid)`, which the kernel delivers
/// only while `pid` is its own thread group's leader, checked at delivery: an id that became another
/// process's thread (where a plain `kill` would end that whole process) gets nothing.
#[cfg(unix)]
fn plain_signal(pid: u32, sig: i32, proof: &dyn Fn() -> bool) -> bool {
    #[cfg(target_os = "linux")]
    let send = || unsafe { libc::syscall(libc::SYS_tgkill, pid as libc::pid_t, pid as libc::pid_t, sig) } == 0;
    #[cfg(not(target_os = "linux"))]
    let send = || unsafe { libc::kill(pid as i32, sig) } == 0;
    proof() && send()
}

/// For the dev harness (`scripts/dev/bounce`): `sigterm_opened` by the path used where pidfds are
/// missing or blocked (`plain_signal`), on a kernel that has them.
#[cfg(target_os = "linux")]
pub fn sigterm_opened_without_pidfd(pid: u32, start: u64) -> bool {
    pid != 0 && pid <= i32::MAX as u32 && pid != std::process::id() && plain_signal(pid, libc::SIGTERM, &|| super::start_time(pid) == Some(start))
}

/// `signal_if` through a pidfd: Some(whether it was sent); None only where pidfds are missing or
/// blocked (`ENOSYS`, `EPERM`), for the plain path. No such process (`ESRCH`) or not a process
/// (`EINVAL`: the id is a thread of another process now) is nothing to signal; out of descriptors or
/// memory, or any other error, signals nothing and says so (`spawn::left_alone`), without reading the
/// proof first: with no descriptor left, that read would fail too.
#[cfg(target_os = "linux")]
fn pidfd_signal(pid: u32, sig: i32, proof: &dyn Fn() -> bool) -> Option<bool> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) } as i32;
    if fd < 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ENOSYS | libc::EPERM) => None,
            Some(libc::ESRCH | libc::EINVAL) => Some(false),
            e => {
                super::spawn::leave(Some(pid), "a process strobe tried to end", format!("strobe could not open a pidfd to check and signal it (error {}), so it left it alone", e.unwrap_or(0)));
                Some(false)
            }
        };
    }
    let send = |sig: i32| unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, sig, std::ptr::null::<libc::siginfo_t>(), 0) } == 0;
    let sent = proof() && send(0) && send(sig);
    unsafe { libc::close(fd) };
    Some(sent)
}
