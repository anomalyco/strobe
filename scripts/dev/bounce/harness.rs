// Dev harness for the launch-feedback primitives (dev only, not shipped): runs each OS's primitive on a
// fixture whose signal comes after a planted delay, and prints the primitive's time beside the fixture's
// own clock reading of the same moment, so the error is a subtraction. Every time is ms after the launch
// clock (`Spawned::at_ns`, read just before the launch request) on `host::now_ns`'s clock. The launches
// go through `isolate::spawn` with `App::with_feedback`, as a bench's will.
//   cargo run --manifest-path scripts/dev/bounce/Cargo.toml -- <mode> ...
//
//   notos                                 each primitive's answer where its OS has none
//   idle <idle.exe> <mode> <ms> [deadline_ms]
//                                         Windows: launches the idle.rs fixture (get, wait, peek, late,
//                                         nopump, none) and watches its input idle
//   idle-console                          Windows: the same for a console program (PING.EXE) and one that
//                                         exits at once (`cmd /c exit`)
//   raw-idle <exe> [args...]              Windows: each change of `WaitForInputIdle(h, 0)`'s answer over a
//                                         launch's first second
//   startup <ms> [deadline_ms] [silent]   Linux X11: launches this harness as `fixture-remove` and watches
//                                         for its `remove:` of the startup id the launch announced
//   startup-wayland                       Linux: the same for an app on strobe's Wayland compositor
//   fixture-remove <ms> [silent]          Linux: the fake app: says `remove:` for $DESKTOP_STARTUP_ID after
//                                         <ms> (never with `silent`), then idles 3 s
//   listen <ms>                           Linux: a desktop's startup-notification monitor, printing each message
//   dock [path...]                        macOS: the Dock's settings and each path's bundle
//   checkin <app> <ms> <ls|spawn> [deadline_ms]
//                                         macOS: launches the checkin.swift fixture's bundle through Launch
//                                         Services or directly, watches its check-in, ends it, and says
//                                         what is left
//   reuse <app>                           macOS: the pid-reuse sequences against a decoy this harness
//                                         starts (a shell and its child); the decoy must survive each
//   ctrlc <app> <ms>                      macOS: a stop (SIGINT to this harness) <ms> after a Launch Services
//                                         launch began: how long strobe takes to exit, then the caller
//                                         checks nothing is left
//   ctrlc-look <app> <ms>                 macOS: the same as `look` handles it (the failed launch releases
//                                         the home and exits at once); the caller checks for the app after
//
// Decoy cases (`host::kill`'s standard): every process signalled here is one this harness started (a
// decoy stands in for an unrelated process); each case prints whether the decoy and its child survived.
//   decoys                                every OS: `kill_launch`, `kill_opened` and `isolate::end_roots`
//                                         given a decoy's pid with a stale start time or a stamp it lacks;
//                                         `release_home` of a stamped decoy (it must end); Unix: a stamped
//                                         decoy leading its own group with an unstamped orphan in it
//                                         (`release_home`, `run_to_exit`): the orphan must survive
//   identity <app>                        macOS: `macos::launched` (open_app's proof) on fixture processes
//                                         this harness started: unstamped, another's stamp, too old, the right one
//   xpc <Decoy.app/Contents/MacOS/bounce-dev>
//                                         macOS: a decoy app responsible for itself starts a child (responsible
//                                         to it) and exits; `kill_opened` by its pid with another start time
//                                         must leave the child; with its own start time it ends it
//   decoy-parent <ms>                     Unix: the decoy `xpc` starts: a `sleep` child, then exits after <ms>
//   daemon-ls <app>                       macOS: one long-lived process launches a bundle twice through Launch
//                                         Services with a person's launch of it between: the person's record stays
//   person-copy <app>                     macOS: a copy of the app strobe did not launch runs when strobe's
//                                         launch ends: the Launch Services record stays, and is said
//   safari-fake                           every OS: the WebKit driver's proof and clean-up (`browser_proof`,
//                                         `browser_set`, `end_browser_set`) on a decoy standing in for the
//                                         browser: a pid reuse is staged by handing them the decoy's pid
//                                         with the start time of a browser that ran there before
//   foreign <Single.app>                  macOS: a person's copy of an app that allows one instance runs;
//                                         strobe's Launch Services launch must fail and touch nothing of it
//   stub <Stub.app>                       macOS: an app whose executable re-executes a sibling binary: the
//                                         launch's stamp proves it
//   retry-person <app>                    macOS: a person opens the app while strobe removes the record its
//                                         launch made: the record stays
//   suspend-open                          Windows x64: the stack sampler's opens of a decoy's process and
//                                         thread, with the right and with a wrong creation time
//   thread-signal                         Linux: a thread id of a decoy process handed to the signal path
//                                         (`sigterm_opened`, `kill_opened`): the decoy must survive
//   thread-fallback                       Linux: thread-signal through the path used without pidfds
//                                         (`tgkill`): the thread id gets nothing, the leader id is ended
//   left-log                              the left-alone log (point the state folder at scratch): a note
//                                         made 3 times is logged once, 8 threads' lines stay whole, it
//                                         rotates past 1 MB and the last-day count reads both files
//   threaded <s>                          Linux: thread-signal's decoy, a process with a second thread
//   exit-ls <app> <n>                     macOS: n launches through Launch Services of an app that exits at
//                                         once: how each ends (never "another user's process")

use std::time::{Duration, Instant};
use strobe::host;

fn ms(ns: u64, from: u64) -> String {
    format!("{:.3}", (ns as i128 - from as i128) as f64 / 1e6)
}

fn main() {
    // A question a launch asks of a process of this binary (macOS: `macos::ls_registered`).
    if let Some(code) = host::answer_query() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    let num = |i: usize, d: u64| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    match arg(0).as_str() {
        "notos" => notos(),
        #[cfg(windows)]
        "idle" => idle(&arg(1), &arg(2), num(3, 100), num(4, 3000)),
        #[cfg(windows)]
        "raw-idle" => raw_idle(&args[1..]),
        #[cfg(windows)]
        "idle-console" => {
            // Console programs: Windows keeps no input idle for one, however soon it exits.
            for (exe, a) in [("C:/Windows/System32/PING.EXE", vec!["-n", "3", "127.0.0.1"]), ("C:/Windows/System32/cmd.exe", vec!["/c", "exit"])] {
                let mut cmd = std::process::Command::new(exe);
                cmd.args(&a).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                let asked = host::now_ns();
                // CREATE_NO_WINDOW: no console shows.
                let mut owned = host::spawn_owned(&mut cmd, None, 0x0800_0000, None).expect("spawn");
                let got = owned.input_idle(Instant::now() + Duration::from_secs(3)).wait(Instant::now() + Duration::from_secs(4));
                println!("console {}: {got:?} at {} ms", a.join(" "), ms(host::now_ns(), asked));
                owned.end();
            }
        }
        #[cfg(target_os = "linux")]
        "startup" => startup(num(1, 120), num(2, 2000), arg(3) == "silent"),
        #[cfg(target_os = "linux")]
        "startup-wayland" => {
            use strobe::target::isolate;
            let app = isolate::App::bare(std::path::Path::new("/bin/true")).as_kind(isolate::AppKind::Native).with_display(Some(isolate::Display::Wayland));
            println!("wayland: {:?}", isolate::startup_notice(&app, Instant::now() + Duration::from_secs(1)).err());
        }
        #[cfg(target_os = "linux")]
        "fixture-remove" => fixture_remove(num(1, 120), arg(2) == "silent"),
        #[cfg(target_os = "linux")]
        "listen" => listen(num(1, 3000)),
        #[cfg(target_os = "macos")]
        "dock" => {
            println!("{:?} hides the bounce: {:?}", host::dock_settings(), host::dock_settings().and_then(|d| d.hides_bounce()));
            for p in &args[1..] {
                let app = strobe::target::isolate::App::bare(std::path::Path::new(p));
                println!(
                    "{p}: {:?} · dock_launch {:?} · path {:?}",
                    host::macos::bundle_of(std::path::Path::new(p)),
                    strobe::target::isolate::dock_launch(&app),
                    strobe::target::isolate::launch_path(&app, false)
                );
            }
            println!("AppKit loaded after these reads: {}", appkit_loaded());
        }
        // A project's relative `exe` resolved from its root while strobe runs elsewhere.
        "fromdir" => println!(
            "from_dir({:?}, {:?}) = {:?} (cwd {:?})",
            arg(1),
            arg(2),
            strobe::target::isolate::from_dir(std::path::Path::new(&arg(1)), std::path::Path::new(&arg(2))),
            std::env::current_dir().ok()
        ),
        #[cfg(target_os = "macos")]
        "checkin" => checkin(&arg(1), num(2, 200), &arg(3), num(4, 5000)),
        #[cfg(target_os = "macos")]
        "reuse" => reuse(&arg(1)),
        #[cfg(target_os = "macos")]
        "ctrlc" => ctrlc(&arg(1), num(2, 5)),
        "decoys" => decoys(),
        #[cfg(target_os = "macos")]
        "identity" => identity(&arg(1)),
        #[cfg(target_os = "macos")]
        "xpc" => xpc(&arg(1)),
        #[cfg(unix)]
        "decoy-parent" => decoy_parent(num(1, 1500)),
        #[cfg(unix)]
        m @ ("sleeper" | "decoy-tree" | "decoy-group" | "orphaner" | "leave-session") => decoy_part(m, num(1, 60)),
        #[cfg(target_os = "macos")]
        "ctrlc-look" => ctrlc_look(&arg(1), num(2, 5)),
        #[cfg(target_os = "macos")]
        "daemon-ls" => daemon_ls(&arg(1)),
        #[cfg(target_os = "macos")]
        "daemon-ls2" => daemon_ls2(&arg(1)),
        #[cfg(target_os = "macos")]
        "person-ls" => person_ls(&arg(1)),
        #[cfg(target_os = "macos")]
        "person-copy" => person_copy(&arg(1)),
        "safari-fake" => safari_fake(),
        #[cfg(all(windows, target_arch = "x86_64"))]
        "suspend-open" => suspend_open(),
        #[cfg(target_os = "linux")]
        "thread-signal" => thread_signal(),
        #[cfg(target_os = "linux")]
        "thread-fallback" => thread_fallback(),
        "left-log" => left_log(),
        #[cfg(target_os = "linux")]
        "threaded" => threaded(num(1, 60)),
        #[cfg(target_os = "macos")]
        "exit-ls" => exit_ls(&arg(1), num(2, 10)),
        #[cfg(target_os = "macos")]
        "foreign" => foreign(&arg(1)),
        #[cfg(target_os = "macos")]
        "stub" => stub(&arg(1)),
        #[cfg(target_os = "macos")]
        "retry-person" => retry_person(&arg(1)),
        m => eprintln!("unknown mode {m:?}"),
    }
}

fn notos() {
    let soon = Instant::now() + Duration::from_millis(50);
    println!("checkin: {:?}", host::checkin(std::process::id(), host::start_time(std::process::id()), soon).wait(soon));
    let mut cmd = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" });
    if cfg!(windows) {
        cmd.args(["/c", "exit"]);
    }
    // CREATE_NO_WINDOW on Windows: no console shows.
    if let Ok(o) = host::spawn_owned(&mut cmd, None, if cfg!(windows) { 0x0800_0000 } else { 0 }, None) {
        let f = o.input_idle(soon);
        println!("input_idle: {:?}", f.wait(soon + Duration::from_secs(1)));
    }
    let app = strobe::target::isolate::App::bare(std::path::Path::new("x"));
    println!("startup_notice: {:?}", strobe::target::isolate::startup_notice(&app, soon).err());
    println!("dock_launch: {:?}", strobe::target::isolate::dock_launch(&app));
    println!("dock_settings: {:?}", host::dock_settings());
    println!("launch_path: {:?}", strobe::target::isolate::launch_path(&app, false));
    println!("DOCK_BOUNCE_MS: {} · ms_between(5, 3) = {}", host::DOCK_BOUNCE_MS, host::ms_between(5, 3));
}

fn field(lines: &[String], key: &str) -> Option<u64> {
    lines.iter().find_map(|l| l.strip_prefix(&format!("{key}="))?.trim().parse().ok())
}

/// A launch through `isolate::spawn` with its feedback watch, and its output lines (its own clock
/// readings) once `wait` came back.
#[cfg(any(windows, target_os = "linux"))]
fn launch_watched(app: strobe::target::isolate::App, deadline_ms: u64, settle_ms: u64) -> (u64, Result<u64, host::NoFeedback>, Vec<String>) {
    use strobe::target::isolate;
    let app = app.with_feedback(Duration::from_millis(deadline_ms));
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let mut s = isolate::spawn(&app, &home, &[], &[], true, None).expect("spawn");
    let got = s.feedback.as_ref().expect("feedback").wait(Instant::now() + Duration::from_millis(deadline_ms + 500));
    std::thread::sleep(Duration::from_millis(settle_ms));
    let lines: Vec<String> = s.output.take().map(|rx| rx.try_iter().map(|l| l.2).collect()).unwrap_or_default();
    let at = s.at_ns;
    isolate::end_launch(Some(s.child), Some(&home.dir), true);
    (at, got, lines)
}

#[cfg(windows)]
fn idle(exe: &str, mode: &str, delay: u64, deadline_ms: u64) {
    let app = strobe::target::isolate::App::bare(std::path::Path::new(exe)).with_extra(&[mode.to_string(), delay.to_string(), (deadline_ms + 1000).to_string()], &[]);
    let (at, got, lines) = launch_watched(app, deadline_ms, 100);
    let (start, pump) = (field(&lines, "start_ns"), field(&lines, "pump_ns"));
    println!(
        "mode={mode} planted={delay} fixture_main_ms={} fixture_pump_ms={} idle={}",
        start.map_or("-".into(), |s| ms(s, at)),
        pump.map_or("-".into(), |p| ms(p, at)),
        match got {
            Ok(t) => format!("{} (after pump {})", ms(t, at), pump.map_or("-".into(), |p| ms(t, p))),
            Err(e) => format!("{e:?}"),
        }
    );
}

/// What `WaitForInputIdle(h, 0)` answers over a launch's first second, each change of answer with its
/// time: how Windows treats a process before and without a message queue.
#[cfg(windows)]
fn raw_idle(cmdline: &[String]) {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::WaitForInputIdle;
    let mut cmd = std::process::Command::new(&cmdline[0]);
    // CREATE_NO_WINDOW: a console program shows no console.
    cmd.creation_flags(0x0800_0000).args(&cmdline[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let asked = host::now_ns();
    let child = cmd.spawn().expect("spawn");
    let h = child.as_raw_handle();
    let mut last = (u32::MAX - 1, 0);
    while host::now_ns() < asked + 1_000_000_000 {
        let r = unsafe { WaitForInputIdle(h as _, 0) };
        let e = if r == u32::MAX { unsafe { GetLastError() } } else { 0 };
        if (r, e) != last {
            println!("{} ms: {r:#x} error {e}", ms(host::now_ns(), asked));
            last = (r, e);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut child = child;
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
fn startup(delay: u64, deadline_ms: u64, silent: bool) {
    let me = std::env::current_exe().expect("exe");
    let mut extra = vec!["fixture-remove".to_string(), delay.to_string()];
    if silent {
        extra.push("silent".into());
    }
    let app = strobe::target::isolate::App::bare(&me).with_extra(&extra, &[]);
    // A silent fixture hears strobe's own `remove:`, said at the deadline.
    let (at, got, lines) = launch_watched(app, deadline_ms, if silent { 600 } else { 100 });
    let (start, sent, cleanup) = (field(&lines, "start_ns"), field(&lines, "remove_ns"), field(&lines, "cleanup_ns"));
    println!(
        "planted={delay} silent={silent} fixture_main_ms={} fixture_remove_ms={} strobe_remove_heard_ms={} remove={}",
        start.map_or("-".into(), |s| ms(s, at)),
        sent.map_or("-".into(), |s| ms(s, at)),
        cleanup.map_or("-".into(), |s| ms(s, at)),
        match got {
            Ok(t) => format!("{} (after send {})", ms(t, at), sent.map_or("-".into(), |s| ms(t, s))),
            Err(e) => format!("{e:?}"),
        }
    );
}

/// A startup-notification monitor, as a desktop runs one: prints every message on the display for
/// `ms`, each with its time (what a desktop would show feedback for: a `new:` without `SILENT=1`).
#[cfg(target_os = "linux")]
fn listen(ms_total: u64) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask};
    let (conn, screen) = x11rb::connect(None).expect("display");
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).unwrap().reply().unwrap().atom;
    let (begin, more) = (atom("_NET_STARTUP_INFO_BEGIN"), atom("_NET_STARTUP_INFO"));
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).unwrap();
    conn.flush().unwrap();
    let t0 = host::now_ns();
    let mut texts: std::collections::HashMap<u32, Vec<u8>> = std::collections::HashMap::new();
    while host::now_ns() < t0 + ms_total * 1_000_000 {
        match conn.poll_for_event() {
            Ok(Some(x11rb::protocol::Event::ClientMessage(m))) if m.type_ == begin || m.type_ == more => {
                let text = texts.entry(m.window).or_default();
                if m.type_ == begin {
                    text.clear();
                }
                text.extend_from_slice(&m.data.as_data8());
                if let Some(end) = text.iter().position(|b| *b == 0) {
                    println!("monitor {} ms: {}", ms(host::now_ns(), t0), String::from_utf8_lossy(&text[..end]));
                    text.clear();
                }
            }
            Ok(_) => std::thread::sleep(Duration::from_millis(1)),
            Err(e) => return println!("monitor: {e}"),
        }
    }
}

/// The fake X11 app: says `remove:` for its startup id after `delay` ms, as a toolkit does when it maps
/// its first window.
#[cfg(target_os = "linux")]
fn fixture_remove(delay: u64, silent: bool) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, WindowClass};
    let start = host::now_ns();
    println!("start_ns={start}");
    let id = std::env::var("DESKTOP_STARTUP_ID").unwrap_or_default();
    let (conn, screen) = x11rb::connect(None).expect("display");
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).unwrap().reply().unwrap().atom;
    let (begin, more) = (atom("_NET_STARTUP_INFO_BEGIN"), atom("_NET_STARTUP_INFO"));
    let win = conn.generate_id().unwrap();
    conn.create_window(0, win, root, -100, -100, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1)).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(delay));
    if silent {
        // Listens for the `remove:` of its id that its launcher must say for it once it gives up.
        use x11rb::protocol::xproto::ChangeWindowAttributesAux;
        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).unwrap();
        conn.flush().unwrap();
        let until = Instant::now() + Duration::from_secs(4);
        let mut text = Vec::new();
        while Instant::now() < until {
            match conn.poll_for_event() {
                Ok(Some(x11rb::protocol::Event::ClientMessage(m))) if m.type_ == begin || m.type_ == more => {
                    if m.type_ == begin {
                        text.clear();
                    }
                    text.extend_from_slice(&m.data.as_data8());
                    if let Some(end) = text.iter().position(|b| *b == 0) {
                        let msg = String::from_utf8_lossy(&text[..end]).to_string();
                        if strobe::host::x11::startup_message(&msg).is_some_and(|(k, keys)| k == "remove" && keys.iter().any(|(n, v)| n == "ID" && *v == id)) {
                            println!("cleanup_ns={}", host::now_ns());
                            return;
                        }
                        text.clear();
                    }
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(1)),
                Err(_) => return,
            }
        }
        return;
    }
    let mut bytes = format!("remove: ID=\"{id}\"").into_bytes();
    bytes.push(0);
    let at = host::now_ns();
    for (i, piece) in bytes.chunks(20).enumerate() {
        let mut data = [0u8; 20];
        data[..piece.len()].copy_from_slice(piece);
        conn.send_event(false, root, EventMask::PROPERTY_CHANGE, ClientMessageEvent::new(8, win, if i == 0 { begin } else { more }, data)).unwrap();
    }
    conn.flush().unwrap();
    println!("remove_ns={at}");
    std::thread::sleep(Duration::from_secs(3));
}

/// Whether this process has AppKit loaded (dyld's image list).
#[cfg(target_os = "macos")]
fn appkit_loaded() -> bool {
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(i: u32) -> *const std::ffi::c_char;
    }
    (0..unsafe { _dyld_image_count() }).any(|i| {
        let p = unsafe { _dyld_get_image_name(i) };
        !p.is_null() && unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().ends_with("/AppKit")
    })
}

/// The checkin.swift fixture as an app of `isolate`, with its marks file.
#[cfg(target_os = "macos")]
fn fixture_app(app_path: &str, how: &str, via: &str) -> (strobe::target::isolate::App, std::path::PathBuf) {
    use strobe::target::isolate;
    let marks = std::env::temp_dir().join(format!("bounce-marks-{}-{}", std::process::id(), host::now_ns()));
    let _ = std::fs::remove_file(&marks);
    let app = isolate::App::bare(std::path::Path::new(app_path)).as_kind(isolate::AppKind::Native).with_extra(&[how.to_string(), marks.display().to_string()], &[]);
    let path = if via == "ls" { isolate::LaunchPath::LaunchServices } else { isolate::LaunchPath::Spawn };
    (app.with_launch_path(path), marks)
}

#[cfg(target_os = "macos")]
fn checkin(app_path: &str, delay: u64, via: &str, deadline_ms: u64) {
    use strobe::target::isolate;
    let (app, marks) = fixture_app(app_path, &delay.to_string(), via);
    let app = app.with_feedback(Duration::from_millis(deadline_ms));
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).ok();
    let registered = || bundle.as_ref().and_then(host::macos::ls_registered);
    println!("Launch Services record before: {:?}", registered());
    let s = match isolate::spawn(&app, &home, &[], &[], false, None) {
        Ok(s) => s,
        Err(e) => return println!("spawn: {}", e.message),
    };
    let (returned, at, pid) = (host::now_ns(), s.at_ns, s.pid);
    // This process's CPU over the watch (the watch's thread is nearly all of it: the main thread waits).
    let (cpu0, t0) = (host::process_cycles(std::process::id()).unwrap_or(0), host::now_ns());
    let got = s.feedback.as_ref().expect("feedback").wait(Instant::now() + Duration::from_millis(deadline_ms + 500));
    let (cpu1, t1) = (host::process_cycles(std::process::id()).unwrap_or(0), host::now_ns());
    println!("watch: {} ms of CPU over {} ms ({:.2}% of a core)", ms(cpu1, cpu0), ms(t1, t0), cpu1.saturating_sub(cpu0) as f64 / t1.saturating_sub(t0).max(1) as f64 * 100.0);
    // The fixture's own marks (mach ns): main, just before `run`, its will- and did-finish-launching.
    let until = Instant::now() + Duration::from_secs(2);
    let mut text = String::new();
    while Instant::now() < until {
        text = std::fs::read_to_string(&marks).unwrap_or_default();
        if text.contains("did_ns=") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (main, will, did) = (field(&lines, "main_ns"), field(&lines, "will_ns"), field(&lines, "did_ns"));
    let at_of = |v: Option<u64>| v.map_or("-".into(), |v| ms(v, at));
    println!(
        "via={via} path={:?} planted={delay} pid={pid} launch_returned_ms={} fixture_main_ms={} will_ms={} did_ms={} checkin={}",
        s.path,
        ms(returned, at),
        at_of(main),
        at_of(will),
        at_of(did),
        match got {
            Ok(t) => format!("{} (after will {}, before did {})", ms(t, at), will.map_or("-".into(), |w| ms(t, w)), did.map_or("-".into(), |d| ms(d, t))),
            Err(e) => format!("{e:?}"),
        }
    );
    let start = s.child.start();
    isolate::end_launch(Some(s.child), Some(&home.dir), true);
    let left: Vec<u32> = host::running(&host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle").executable);
    println!(
        "after end: pid alive={} same-exe processes={left:?} home removed={} Launch Services record: {:?} left alone: {}",
        start.is_some() && host::start_time(pid) == start,
        !home.dir.exists(),
        registered(),
        left_text()
    );
    let _ = std::fs::remove_file(&marks);
}

/// The pid-reuse sequences, each against a decoy this harness starts itself (a shell and its child
/// `sleep`), standing in for an unrelated process that took a launch's pid: each must end without
/// touching the decoy. Ends the decoy by its exact pids at the end.
#[cfg(target_os = "macos")]
fn reuse(app_path: &str) {
    use strobe::target::isolate;
    let (decoy, d, child, d_start) = decoy(&[], Kind::Tree);
    println!("decoy pid {d} (start {d_start}), its child {child:?}");
    // 1. A launch whose root pid now names the decoy: its start time is the launch's, not the decoy's.
    host::kill_launch(d, Some(d_start - 1));
    host::kill_opened(d, d_start - 1);
    println!("1. kill_launch / kill_opened with the launch's start time: decoy alive {} · its child alive {}", runs(d), alive(child));
    // 2. The check-in watch of a launch whose pid now names a Cocoa app that has checked in: Ended, never Ok.
    let (app, marks) = fixture_app(app_path, "50", "spawn");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let s = isolate::spawn(&app, &home, &[], &[], false, None).expect("spawn");
    let real = s.child.start();
    std::thread::sleep(Duration::from_millis(1500));
    let wrong = host::checkin(s.pid, real.map(|r| r - 1), Instant::now() + Duration::from_secs(1)).wait(Instant::now() + Duration::from_secs(2));
    let right = host::checkin(s.pid, real, Instant::now() + Duration::from_secs(1)).wait(Instant::now() + Duration::from_secs(2)).map(|_| "checked in");
    println!("2. checkin of a checked-in app by its pid with another start time: {wrong:?} · with its own: {right:?}");
    isolate::end_launch(Some(s.child), Some(&home.dir), true);
    let _ = std::fs::remove_file(&marks);
    // 3. An app launched through Launch Services that exits at once: nothing is signalled by its pid.
    let (app, marks) = fixture_app(app_path, "exit", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    match isolate::spawn(&app, &home, &[], &[], false, None) {
        Ok(s) => {
            let (pid, start) = (s.pid, s.child.start());
            std::thread::sleep(Duration::from_millis(500));
            let mut c = s.child;
            let ended = c.ended();
            isolate::end_launch(Some(c), Some(&home.dir), true);
            println!("3. an app that exits at once: pid {pid}, start read {:?}, ended {:?} ({}), left alone {}", start.is_some(), ended, ended.map(|e| e.to_string()).unwrap_or_default(), left_text());
        }
        Err(e) => println!("3. spawn: {}", e.message),
    }
    let _ = std::fs::remove_file(&marks);
    // 4. A stop's sweep (`end_launches`) with only the decoy's pid listed would need a reused pid; the
    //    decoy must still be alive after everything above.
    println!("after all: decoy alive {} · its child alive {}", runs(d), alive(child));
    end_decoy(decoy, &[child]);
}

/// A stop <ms> after a Launch Services launch began: `end_on_stop` ends the launch and its home (the
/// Launch Services record it made included) and exits 130; the time to that exit is printed by the
/// stop's own thread just before.
#[cfg(target_os = "macos")]
fn ctrlc(app_path: &str, after_ms: u64) {
    use strobe::target::isolate;
    let (app, _marks) = fixture_app(app_path, "200", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let _stop = isolate::end_on_stop(Some(&home));
    let t0 = host::now_ns();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(after_ms));
        println!("stop sent at {} ms", ms(host::now_ns(), t0));
        unsafe { libc::raise(libc::SIGINT) };
    });
    match isolate::spawn(&app, &home, &[], &[], false, None) {
        Ok(s) => println!("launched pid {} at {} ms", s.pid, ms(host::now_ns(), t0)),
        Err(e) => println!("spawn at {} ms: {}", ms(host::now_ns(), t0), e.message),
    }
    std::thread::sleep(Duration::from_secs(10));
    println!("still running 10 s after the stop: the stop did not end strobe");
}

// ---- Decoy cases (`host::kill`'s standard) ------------------------------------------------------

/// A process this harness's decoys started, by its pid and the start time read from a snapshot of the
/// decoy's tree while the decoy (this harness's own child, unreaped) ran: the proof every check and
/// every signal here goes by.
type Proc = (u32, u64);

/// Whether a process still runs: its pid with the start time it was found with (a zombie has none).
fn alive(p: Option<Proc>) -> bool {
    p.is_some_and(|(p, s)| host::start_time(p) == Some(s))
}

/// Whether this harness's own child (unreaped: its pid is its own) still runs.
fn runs(pid: u32) -> bool {
    host::start_time(pid).is_some()
}

/// What strobe left alone so far, as the end of a run prints it.
fn left_text() -> String {
    let left = host::spawn::left_alone();
    if left.is_empty() { "none".into() } else { format!("\n{}", host::spawn::left_alone_text(&left).trim_end()) }
}

/// What a decoy is: a process with a child (`Tree`), or (Unix) one leading its own process group with
/// an orphan in it that carries no stamp (`Group`: its first child started the orphan and exited).
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Tree,
    #[cfg(unix)]
    Group,
}

/// The stamp variables, which an unstamped decoy process goes without.
#[cfg(unix)]
const STAMP_VARS: [&str; 2] = [host::spawn::HOME_VAR, host::spawn::OWNER_VAR];

/// A decoy this harness starts, standing in for an unrelated process (with `env`, for a stamped one).
/// Windows: `cmd` running `ping` (no console). Unix: this harness's own binary (`decoy-tree`,
/// `decoy-group`): macOS hides a platform binary's environment (`/bin/sh`'s) from every other process,
/// so a shell could never show a stamp. (process, pid, its child, start time.)
fn decoy(env: &[(String, String)], kind: Kind) -> (std::process::Child, u32, Option<Proc>, u64) {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let _ = kind;
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "ping -n 60 127.0.0.1 >nul"]).creation_flags(0x0800_0000);
        c
    };
    #[cfg(unix)]
    let mut cmd = {
        use std::os::unix::process::CommandExt;
        let mut c = std::process::Command::new(std::env::current_exe().expect("exe"));
        c.args([if kind == Kind::Tree { "decoy-tree" } else { "decoy-group" }, "60"]);
        if kind == Kind::Group {
            c.process_group(0);
        }
        c
    };
    cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let c = cmd.spawn().expect("decoy");
    let d = c.id();
    std::thread::sleep(Duration::from_millis(500));
    // Its command's process (Windows: not the console host a console program gets).
    let child = host::process_tree(d).into_iter().find(|p| p.pid != d && !p.name.to_lowercase().starts_with("conhost")).map(|p| (p.pid, p.start));
    let start = host::start_time(d).expect("decoy start");
    (c, d, child, start)
}

/// Ends a process a decoy started, by its pid and the start time it was found with, through strobe's own
/// proven path (`host::kill_opened`, which reads the start time again right before the signal): a pid
/// another process took meanwhile is left alone.
fn end_pid(p: Option<Proc>) {
    if let Some((p, s)) = p {
        host::kill_opened(p, s);
    }
}

/// Ends a decoy: the processes named, everything still in its tree (from a snapshot of it while it is
/// this harness's unreaped child), then the decoy itself through its own handle.
fn end_decoy(mut c: std::process::Child, kids: &[Option<Proc>]) {
    for k in kids {
        end_pid(*k);
    }
    for p in host::process_tree(c.id()).into_iter().filter(|p| p.pid != c.id()) {
        end_pid(Some((p.pid, p.start)));
    }
    let _ = c.kill();
    let _ = c.wait();
}

/// A scratch folder standing in for a launch's home, stamped by this harness (`launch_env`).
fn scratch_home(tag: &str) -> (std::path::PathBuf, [(String, String); 2]) {
    let h = std::env::temp_dir().join(format!("bounce-home-{tag}-{}-{}", std::process::id(), host::now_ns()));
    std::fs::create_dir_all(&h).expect("home");
    let env = strobe::target::isolate::launch_env(&h);
    (h, env)
}

/// This harness's stamp for `home`.
fn our_stamp(home: &std::path::Path) -> host::spawn::Stamp {
    let (pid, start) = host::spawn::parse_owner(host::spawn::owner_stamp()).expect("owner");
    host::spawn::Stamp { home: host::spawn::home_key(home), pid, start }
}

/// Unix: the processes of process group `pgid` other than `but`.
#[cfg(unix)]
fn group_of(pgid: u32, but: u32) -> Vec<Proc> {
    host::process_list().into_iter().filter(|&(p, _, _)| p != but && unsafe { libc::getpgid(p as i32) } == pgid as i32).map(|(p, _, s)| (p, s)).collect()
}

fn decoys() {
    use host::spawn::Stamped;
    use strobe::target::isolate;
    #[cfg(target_os = "linux")]
    {
        // Whether this kernel gives pidfds, through which strobe's signals go here (`host::kill`).
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, std::process::id() as libc::pid_t, 0) } as i32;
        println!("pidfd on this kernel: {}", fd >= 0);
        if fd >= 0 {
            unsafe { libc::close(fd) };
        }
    }
    // 1. kill_launch / kill_opened given an unrelated process's pid with a launch's start time.
    let (c1, d, k, ds) = decoy(&[], Kind::Tree);
    let stale = ds - 1;
    host::kill_launch(d, Some(stale));
    host::kill_opened(d, stale);
    println!("1. kill_launch / kill_opened, decoy pid with another start time: decoy alive {} · its child alive {}", runs(d), alive(k));
    // 2. end_roots (release_home, reap_dead_owners, a failed launch's sweep) given a scan entry that is not
    //    the process now: a stamped decoy named with another start time; an unstamped decoy (environment
    //    readable) named with its own start time and a stamp it does not carry.
    let (h, env) = scratch_home("a");
    let (c2, d2, k2, ds2) = decoy(&env, Kind::Tree);
    let ended_a = isolate::end_roots(&[Stamped { pid: d2, parent: 0, start: ds2 - 1, stamp: our_stamp(&h) }]).len();
    let ended_b = isolate::end_roots(&[Stamped { pid: d, parent: 0, start: ds, stamp: our_stamp(&h) }]).len();
    println!(
        "2. end_roots: stamped decoy with another start time: ended {ended_a}, alive {} · child {} | unstamped decoy (environment readable {}, stamp {:?}) with its own start time: ended {ended_b}, alive {} · child {}",
        runs(d2),
        alive(k2),
        host::env_of(d).is_some(),
        host::spawn::stamp_of(d),
        runs(d),
        alive(k)
    );
    // 3. The positive control: release_home of the stamped decoy's home ends it (pid, start and stamp its own).
    println!("   stamped decoy {d2}: stamp {:?} · scan of its home: {:?}", host::spawn::stamp_of(d2), isolate::stamped_home(&h).iter().map(|s| (s.pid, s.parent)).collect::<Vec<_>>());
    isolate::release_home(&h);
    std::thread::sleep(Duration::from_millis(300));
    println!("3. release_home of the stamped decoy's home: stamped decoy alive {} · child {} (both must be ended) | the unstamped decoy alive {} · child {}", runs(d2), alive(k2), runs(d), alive(k));
    end_decoy(c2, &[k2]);
    #[cfg(unix)]
    group_cases();
    println!("after all: the unstamped decoy alive {} · its child alive {}", runs(d), alive(k));
    end_decoy(c1, &[k]);
    let _ = std::fs::remove_dir_all(&h);
    println!("left alone in this run: {}", left_text());
}

/// Unix: a stamped process leading its own group, with a process in that group that is not stamped (an
/// orphan its first child left): only the group of strobe's own child may be signalled, so the orphan stays.
#[cfg(unix)]
fn group_cases() {
    use strobe::target::isolate;
    // 4. release_home (end_roots): the stamped leader ends, the unstamped orphan in its group stays.
    let (h, env) = scratch_home("b");
    let (c3, d3, _, _) = decoy(&env, Kind::Group);
    let orphans = group_of(d3, d3);
    let o = orphans.first().copied();
    println!(
        "4. stamped decoy {d3} leads its group; orphan in it {o:?}: parent {:?}, environment readable {}, stamp {:?}",
        o.and_then(|o| host::process_list().into_iter().find(|p| p.0 == o.0).map(|p| p.1)),
        o.is_some_and(|o| host::env_of(o.0).is_some()),
        o.and_then(|o| host::spawn::stamp_of(o.0))
    );
    isolate::release_home(&h);
    std::thread::sleep(Duration::from_millis(300));
    println!("   release_home: stamped decoy alive {} · the unstamped orphan in its group alive {}", runs(d3), alive(o));
    end_decoy(c3, &[o]);
    let _ = std::fs::remove_dir_all(&h);
    // 5. run_to_exit: a command that leaves a stamped process leading a session of its own (setsid) with an
    //    unstamped orphan in its group; the sweep ends the stamped one and leaves the orphan.
    let (h, env) = scratch_home("c");
    let mut cmd = std::process::Command::new(std::env::current_exe().expect("exe"));
    cmd.args(["leave-session", "63"])
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let owned = host::spawn_owned(&mut cmd, None, 0, Some(&h)).expect("command");
    let leaders: std::cell::RefCell<Vec<(Proc, Vec<Proc>)>> = std::cell::RefCell::new(Vec::new());
    let usage = owned
        .run_to_exit(Duration::from_secs(10), Some(&h), || {
            // Read at the command's exit, before the sweep: each stamped `sleep` and its group.
            for s in isolate::stamped_home(&h) {
                if unsafe { libc::getpgid(s.pid as i32) } == s.pid as i32 {
                    leaders.borrow_mut().push(((s.pid, s.start), group_of(s.pid, s.pid)));
                }
            }
        })
        .expect("run");
    std::thread::sleep(Duration::from_millis(300));
    for (l, members) in leaders.borrow().iter() {
        println!(
            "5. run_to_exit left {:?}: stamped leader {} alive {} · unstamped orphans in its group {:?} alive {:?}{}",
            usage.left,
            l.0,
            alive(Some(*l)),
            members.iter().map(|m| m.0).collect::<Vec<_>>(),
            members.iter().map(|m| alive(Some(*m))).collect::<Vec<_>>(),
            if cfg!(target_os = "linux") { " (Linux: the launch's cgroup, which strobe made, ends what is in it)" } else { "" }
        );
        for m in members {
            end_pid(Some(*m));
        }
        end_pid(Some(*l));
    }
    if leaders.borrow().is_empty() {
        println!("5. run_to_exit: no stamped leader seen at the command's exit (left {:?})", usage.left);
    }
    isolate::release_home(&h);
    let _ = std::fs::remove_dir_all(&h);
}

/// The checkin.swift fixture's executable started by this harness directly (`mode` its first argument),
/// with `env` added and strobe's stamp variables removed first: a copy of the app no launch of strobe's
/// made, unless `env` stamps it.
#[cfg(target_os = "macos")]
fn fixture_process(bundle: &host::macos::Bundle, mode: &str, env: &[(String, String)]) -> std::process::Child {
    let mut cmd = std::process::Command::new(&bundle.executable);
    cmd.args([mode, "/dev/null"]).env_remove(host::spawn::HOME_VAR).env_remove(host::spawn::OWNER_VAR);
    cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().expect("fixture")
}

/// Microseconds since 1970 (`start_time`'s units on macOS).
#[cfg(target_os = "macos")]
fn epoch_us() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

/// F1: `open_app`'s proof (`macos::launched`) on processes of the app this harness started.
#[cfg(target_os = "macos")]
fn identity(app_path: &str) {
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    let (h, env) = scratch_home("id");
    let (other, other_env) = scratch_home("other");
    let since = epoch_us();
    let mut runs = Vec::new();
    for (what, e) in [("not stamped", Vec::new()), ("stamped for another launch's home", other_env.to_vec()), ("stamped for this launch", env.to_vec())] {
        let c = fixture_process(&bundle, "2000", &e);
        std::thread::sleep(Duration::from_millis(300));
        let p = c.id();
        let s = host::start_time(p);
        // What the filter before this fix did: a process whose environment shows no stamp passed.
        let old = host::spawn::stamp_of(p).is_none_or(|s| s.home == host::spawn::home_key(&h));
        println!(
            "{what}: pid {p}, environment readable {} · the filter before (stamp_of is None, or names the home) accepted it: {old} · launched(since the request): {:?} · launched(request after its start): {:?}",
            host::env_of(p).is_some(),
            host::macos::launched(p, &bundle, Some(&h), since),
            host::macos::launched(p, &bundle, Some(&h), s.unwrap_or(0) + 1_000_000)
        );
        runs.push(c);
    }
    // A process of another executable (the decoy shell) with this launch's stamp.
    let (c, d, k, _) = decoy(&env, Kind::Tree);
    println!("another executable, stamped for this launch: launched {:?}", host::macos::launched(d, &bundle, Some(&h), since));
    end_decoy(c, &[k]);
    for mut c in runs {
        let _ = c.kill();
        let _ = c.wait();
    }
    let _ = (std::fs::remove_dir_all(&h), std::fs::remove_dir_all(&other));
}

/// Unix: the decoy processes (`decoy`), each this harness's own binary, ending by itself after `secs`:
/// `sleeper` sleeps; `decoy-tree` starts a `sleeper` child (stamped as it is); `decoy-group` starts an
/// `orphaner` without the stamp variables, which starts a `sleeper` and exits (the sleeper stays in the
/// group, launchd's or init's child); `leave-session` starts a `decoy-group` in a session of its own
/// (`setsid`: leading its own group) and exits after 0.5 s.
#[cfg(unix)]
fn decoy_part(mode: &str, secs: u64) {
    use std::os::unix::process::CommandExt;
    let me = std::env::current_exe().expect("exe");
    let quiet = |c: &mut std::process::Command| {
        c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    };
    let start = |args: &[&str], unstamped: bool, session: bool| {
        let mut c = std::process::Command::new(&me);
        c.args(args);
        quiet(&mut c);
        if unstamped {
            for v in STAMP_VARS {
                c.env_remove(v);
            }
        }
        if session {
            unsafe {
                c.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        c.spawn().expect("decoy part")
    };
    let n = secs.to_string();
    match mode {
        "decoy-tree" => std::mem::forget(start(&["sleeper", &n], false, false)),
        "decoy-group" => {
            let _ = start(&["orphaner", &n], true, false).wait();
        }
        "orphaner" => std::mem::forget(start(&["sleeper", &n], false, false)),
        "leave-session" => {
            std::mem::forget(start(&["decoy-group", &n], false, true));
            std::thread::sleep(Duration::from_millis(500));
            return;
        }
        _ => {}
    }
    if mode != "orphaner" {
        std::thread::sleep(Duration::from_secs(secs));
    }
}

/// Unix: `xpc`'s decoy app: starts a `sleep` (its child, responsible to it), then exits after `ms`.
#[cfg(unix)]
fn decoy_parent(ms_alive: u64) {
    let c = std::process::Command::new("/bin/sleep").arg("30").stdin(std::process::Stdio::null()).spawn().expect("sleep");
    println!("child {}", c.id());
    std::mem::forget(c);
    std::thread::sleep(Duration::from_millis(ms_alive));
}

/// F2: a process responsible to an app that exited, then `kill_opened` by the app's pid with the start
/// time of an earlier process at that pid (a launch whose pid the decoy app took): the process stays.
#[cfg(target_os = "macos")]
fn xpc(decoy_exe: &str) {
    use std::io::BufRead;
    let mut cmd = std::process::Command::new(decoy_exe);
    cmd.args(["decoy-parent", "1500"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let disclaimed = host::macos::own_responsibility(&mut cmd);
    let mut u = cmd.spawn().expect("decoy app");
    let up = u.id();
    let us = host::start_time(up).expect("start");
    let mut line = String::new();
    let _ = std::io::BufReader::new(u.stdout.take().expect("stdout")).read_line(&mut line);
    let named: Option<u32> = line.trim().strip_prefix("child ").and_then(|p| p.parse().ok());
    // strobe reads who is responsible for what while the app runs (any process set it measures).
    std::thread::sleep(Duration::from_millis(300));
    let tree = host::process_tree(up);
    let set: Vec<u32> = tree.iter().map(|p| p.pid).collect();
    // The child by pid and the start time of the tree's snapshot, taken while the decoy app (this
    // harness's unreaped child) ran.
    let x: Option<Proc> = tree.iter().find(|p| Some(p.pid) == named && p.parent == up).map(|p| (p.pid, p.start));
    println!("decoy app {up} (disclaimed {disclaimed}, responsible to itself: {:?}), its child {x:?}, its set {set:?}", host::macos::unattributed(up).is_none());
    let _ = u.wait();
    std::thread::sleep(Duration::from_millis(300));
    println!("the decoy app exited (reaped, its pid free: {}); its child alive {} (now launchd's)", host::start_time(up).is_none(), alive(x));
    // A launch that ran at this pid a second before the decoy app: kill_opened by that launch's start time.
    host::kill_opened(up, us - 1_000_000);
    std::thread::sleep(Duration::from_millis(200));
    println!("kill_opened(the decoy app's pid, an earlier start time): its child alive {}", alive(x));
    // The control: by the decoy app's own start time, its child is its leftover and ends.
    host::kill_opened(up, us);
    std::thread::sleep(Duration::from_millis(200));
    println!("kill_opened(the decoy app's pid, its own start time): its child alive {} (must be ended)", alive(x));
    end_pid(x);
}

/// F3: a stop `after_ms` into a Launch Services launch, handled as `look` handles it: the failed launch
/// releases the home and strobe exits at once. The caller checks for the app afterwards.
#[cfg(target_os = "macos")]
fn ctrlc_look(app_path: &str, after_ms: u64) {
    use strobe::target::isolate;
    let (app, _marks) = fixture_app(app_path, "200", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let stop = isolate::end_on_stop(Some(&home));
    let t0 = host::now_ns();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(after_ms));
        unsafe { libc::raise(libc::SIGINT) };
    });
    match isolate::spawn(&app, &home, &[], &[], false, None) {
        Ok(s) => {
            println!("launched pid {} at {} ms", s.pid, ms(host::now_ns(), t0));
            std::thread::sleep(Duration::from_secs(10));
        }
        Err(e) => {
            drop(stop);
            isolate::release_home(&home.dir);
            if home.fresh {
                isolate::remove_home(&home.dir);
            }
            println!("exit 130 at {} ms: {} · left alone: {}", ms(host::now_ns(), t0), e.message, left_text());
            std::process::exit(130);
        }
    }
}

/// One launch through Launch Services, ended as a bench ends a block.
#[cfg(target_os = "macos")]
fn ls_launch(app_path: &str) -> String {
    use strobe::target::isolate;
    let (app, marks) = fixture_app(app_path, "50", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let r = match isolate::spawn(&app, &home, &[], &[], false, None) {
        Ok(s) => {
            std::thread::sleep(Duration::from_millis(800));
            let pid = s.pid;
            isolate::end_launch(Some(s.child), Some(&home.dir), true);
            format!("launched pid {pid} and ended it")
        }
        Err(e) => format!("spawn: {}", e.message),
    };
    let _ = std::fs::remove_file(&marks);
    r
}

/// F4: one long-lived process (as `strobe daemon`) launches a bundle Launch Services has no record of,
/// then a person launches it (a copy this harness starts directly, unstamped: it is recorded as it checks
/// in), then strobe launches it again: the person's record must stay.
#[cfg(target_os = "macos")]
fn daemon_ls(app_path: &str) {
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    let view = || host::macos::ls_registered(&bundle);
    host::macos::unregister(&bundle.path);
    std::thread::sleep(Duration::from_secs(1));
    println!("0. unregistered for the test: record {:?}", view());
    println!("1. strobe: {} · record after {:?}", ls_launch(app_path), view());
    // A person's launch through Launch Services, as Finder's, from another process (`person-ls`, no
    // stamp); its app quits by itself 10 s after it checks in, and is never signalled here.
    let _ = std::process::Command::new(std::env::current_exe().expect("exe")).args(["person-ls", app_path]).env_remove(host::spawn::HOME_VAR).env_remove(host::spawn::OWNER_VAR).status();
    let until = Instant::now() + Duration::from_secs(20);
    while !host::running(&bundle.executable).is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("2. a person's launch of it through Launch Services, quit by itself: record {:?}", view());
    println!("3. strobe again, same process: {} · record after {:?} (must be Some(true): the person's)", ls_launch(app_path), view());
    println!("left alone: {}", left_text());
}

/// macOS: a person's launch of the app through Launch Services (no stamp, not in the Dock's recent
/// items), from this process, which then exits: the app quits by itself 10 s after it checks in.
#[cfg(target_os = "macos")]
fn person_ls(app_path: &str) {
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    let r = host::macos::open_app(&bundle, &["50".into(), "/dev/null".into()], &[], false, None, Duration::from_secs(30));
    println!("person's launch: {:?}", r.map(|o| o.pid));
}

/// F4: one long-lived process reads Launch Services' record of a bundle (present), the record is then
/// removed (as `lsregister -u` or a person's clean-up would), and strobe launches it: the record that
/// launch makes must go again, back to the state before it.
#[cfg(target_os = "macos")]
fn daemon_ls2(app_path: &str) {
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    let view = || host::macos::ls_registered(&bundle);
    println!("0. recorded (macOS records a copied bundle itself): {:?}", view());
    host::macos::unregister(&bundle.path);
    std::thread::sleep(Duration::from_secs(1));
    println!("1. removed outside strobe: {:?}", view());
    println!("2. strobe, same process: {} · record after {:?} (must be Some(false): the state before the launch)", ls_launch(app_path), view());
}

/// F4: a copy of the app strobe did not launch runs when strobe's launch of a bundle Launch Services had
/// no record of ends: the record stays (it may be the person's), and `left_alone` says so.
#[cfg(target_os = "macos")]
fn person_copy(app_path: &str) {
    use strobe::target::isolate;
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    host::macos::unregister(&bundle.path);
    std::thread::sleep(Duration::from_secs(1));
    println!("0. unregistered for the test: record {:?}", host::macos::ls_registered(&bundle));
    let (app, marks) = fixture_app(app_path, "50", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let s = isolate::spawn(&app, &home, &[], &[], false, None).expect("spawn");
    let mut person = fixture_process(&bundle, "50", &[]);
    std::thread::sleep(Duration::from_millis(1500));
    isolate::end_launch(Some(s.child), Some(&home.dir), true);
    println!("1. strobe's launch ended with a person's copy running: record {:?} · left alone: {}", host::macos::ls_registered(&bundle), left_text());
    let _ = person.kill();
    let _ = person.wait();
    let _ = std::fs::remove_file(&marks);
}

/// A decoy standing in for a browser strobe launched: this harness's binary (`decoy-tree`) with
/// `--automation` among its arguments (macOS: what marks Safari as safaridriver's), its child a
/// `sleeper`. Windows: `decoy`'s.
fn browser_decoy() -> (std::process::Child, u32, Option<Proc>, u64) {
    #[cfg(unix)]
    {
        let mut c = std::process::Command::new(std::env::current_exe().expect("exe"));
        c.args(["decoy-tree", "60", "--automation"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let c = c.spawn().expect("decoy");
        let d = c.id();
        std::thread::sleep(Duration::from_millis(500));
        let child = host::process_tree(d).into_iter().find(|p| p.pid != d).map(|p| (p.pid, p.start));
        let start = host::start_time(d).expect("decoy start");
        (c, d, child, start)
    }
    #[cfg(windows)]
    {
        decoy(&[], Kind::Tree)
    }
}

/// The WebKit driver's proof and clean-up on a decoy (review 3, finding 1). The session itself is not
/// staged (Safari's remote automation is a setting of the person's), and a pid cannot be made to
/// recur: the reuse is staged by handing the clean-up the decoy's pid with the start time of a browser
/// that ran at that pid before (the decoy's own, less a second).
fn safari_fake() {
    use strobe::target::webkit;
    // The "browser's executable": the decoy's (this harness; Windows: cmd), and another one.
    let (me, other) = if cfg!(windows) {
        (std::path::PathBuf::from("C:/Windows/System32/cmd.exe"), std::env::current_exe().expect("exe"))
    } else {
        (std::env::current_exe().expect("exe"), std::path::PathBuf::from("/bin/ls"))
    };
    let since = host::start_now();
    let (c, d, k, ds) = browser_decoy();
    // Off macOS the browser is the driver's child: this harness stands in for the driver.
    let driver = (!cfg!(target_os = "macos")).then_some(std::process::id());
    println!("decoy {d} (start {ds}), its child {:?}", k.map(|k| k.0));
    println!("proof, the decoy as the browser the request started: {:?}", webkit::browser_proof(d, &me, since, driver));
    println!("proof, a request after it started: {:?}", webkit::browser_proof(d, &me, Some(ds + 1), driver));
    println!("proof, another executable: {:?}", webkit::browser_proof(d, &other, since, driver));
    if cfg!(target_os = "macos") {
        let (c2, d2, k2, _) = decoy(&[], Kind::Tree);
        println!("proof, a process of the executable not in automation mode: {:?}", webkit::browser_proof(d2, &me, since, None));
        end_decoy(c2, &[k2]);
    } else {
        // A "driver" the decoy is not under: its own child.
        println!("proof, not in the driver's tree: {:?}", webkit::browser_proof(d, &me, since, k.map(|k| k.0)));
    }
    // A browser that ran at this pid before (started a second earlier, in start_time's units).
    let earlier = ds.saturating_sub(if cfg!(windows) {
        10_000_000
    } else if cfg!(target_os = "macos") {
        1_000_000
    } else {
        100
    });
    // The samplers' set (`host::process_set_pids_of`, finding 3 of the fourth check).
    println!("sampler's set by the earlier browser's start time: {:?} · by its own: {:?}", host::process_set_pids_of(d, Some(earlier), None), host::process_set_pids_of(d, Some(ds), None));
    let set = webkit::browser_set(d, earlier, None, 0);
    println!("snapshot by the earlier browser's start time: {set:?}");
    webkit::end_browser_set(d, earlier, set);
    std::thread::sleep(Duration::from_millis(300));
    println!("clean-up by the earlier browser's start time: decoy alive {} · its child alive {}", runs(d), alive(k));
    // The control: by the decoy's own start time it is the browser, and it and its child end.
    let set = webkit::browser_set(d, ds, None, 0);
    println!("snapshot by its own start time: {set:?}");
    webkit::end_browser_set(d, ds, set);
    std::thread::sleep(Duration::from_millis(300));
    println!("clean-up by its own start time: decoy alive {} · its child alive {} (both must be ended)", runs(d), alive(k));
    end_decoy(c, &[k]);
    println!("left alone in this run: {}", left_text());
}

/// Finding 2: a person's copy of an app that allows one instance (`LSMultipleInstancesProhibited`) runs
/// (this harness starts it directly, unstamped); strobe's launch through Launch Services is handed to
/// it. The launch must fail, and the person's copy must be untouched: alive, never signalled.
#[cfg(target_os = "macos")]
fn foreign(app_path: &str) {
    use strobe::target::isolate;
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    let mut person = fixture_process(&bundle, "50", &[]);
    std::thread::sleep(Duration::from_millis(1500));
    let p = person.id();
    let ps = host::start_time(p);
    let (app, marks) = fixture_app(app_path, "50", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    match isolate::spawn(&app, &home, &[], &[], false, None) {
        Ok(s) => {
            println!("launch returned pid {} (the person's copy is {p}): {}", s.pid, if s.pid == p { "the person's copy, adopted" } else { "another process" });
            isolate::end_launch(Some(s.child), Some(&home.dir), true);
        }
        Err(e) => {
            println!("launch refused ({}): {}", e.validator().map_or("-", |v| v.name()), e.message);
            isolate::release_home(&home.dir);
            isolate::remove_home(&home.dir);
        }
    }
    println!("the person's copy {p} after: alive {} (its start time unchanged: {})", host::start_time(p).is_some() && host::start_time(p) == ps, host::start_time(p) == ps);
    println!("left alone in this run: {}", left_text());
    let _ = person.kill();
    let _ = person.wait();
    let _ = std::fs::remove_file(&marks);
}

/// Finding 7: an app whose executable (`stub`) re-executes its sibling binary (`checkin`) in the same
/// process: the launch's stamp in its readable environment proves it, whatever executable runs.
#[cfg(target_os = "macos")]
fn stub(app_path: &str) {
    use strobe::target::isolate;
    // The proof itself, on a process of the bundle this harness starts with a launch's stamp: once its
    // stub has re-executed the sibling binary (300 ms), it runs `checkin`, not the bundle's executable.
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    let (h, env) = scratch_home("stub");
    let since = host::start_now().unwrap_or(0);
    let mut p = fixture_process(&bundle, "2000", &env);
    std::thread::sleep(Duration::from_millis(300));
    let pid = p.id();
    println!(
        "proof of a stamped process that re-executed a sibling binary (it runs {:?}): {:?}",
        host::exe_of(pid).and_then(|e| e.file_name().map(|n| n.to_string_lossy().to_string())),
        host::macos::launched(pid, &bundle, Some(&h), since)
    );
    let _ = p.kill();
    let _ = p.wait();
    let _ = std::fs::remove_dir_all(&h);
    let (app, marks) = fixture_app(app_path, "50", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    match isolate::spawn(&app, &home, &[], &[], false, None) {
        Ok(s) => {
            let pid = s.pid;
            println!(
                "launch: pid {pid}, start {} (proven: {}), it runs {:?}",
                s.child.start().map_or("-".into(), |v| v.to_string()),
                s.child.start().is_some(),
                host::exe_of(pid).and_then(|e| e.file_name().map(|n| n.to_string_lossy().to_string()))
            );
            std::thread::sleep(Duration::from_millis(800));
            isolate::end_launch(Some(s.child), Some(&home.dir), true);
            println!("after the end: the app alive {}", host::start_time(pid).is_some());
        }
        Err(e) => println!("launch: {}", e.message),
    }
    println!("left alone in this run: {}", left_text());
    let _ = std::fs::remove_file(&marks);
}

/// Finding 9: a person opens the app (a copy this harness starts directly, unstamped, which Launch
/// Services records as it checks in) right after strobe first removed the record its launch made,
/// while strobe checks whether it came back: the record is the person's now, and stays.
#[cfg(target_os = "macos")]
fn retry_person(app_path: &str) {
    use strobe::target::isolate;
    let bundle = host::macos::bundle_of(std::path::Path::new(app_path)).expect("bundle");
    host::macos::unregister(&bundle.path);
    std::thread::sleep(Duration::from_secs(1));
    println!("0. unregistered for the test: record {:?}", host::macos::ls_registered(&bundle));
    let (app, marks) = fixture_app(app_path, "50", "ls");
    let home = isolate::prepare_home(&app, None, None).expect("home");
    let s = isolate::spawn(&app, &home, &[], &[], false, None).expect("spawn");
    std::thread::sleep(Duration::from_millis(800));
    // The person: once the record is gone (strobe's first removal), opens the app.
    let b = bundle.clone();
    let watcher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        let until = Instant::now() + Duration::from_secs(8);
        while Instant::now() < until {
            if host::macos::ls_registered(&b) == Some(false) {
                return Some(fixture_process(&b, "50", &[]));
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        None
    });
    isolate::end_launch(Some(s.child), Some(&home.dir), true);
    let person = watcher.join().ok().flatten();
    println!(
        "1. a person opened it after strobe's first removal (pid {:?}); strobe's clean-up ended: record {:?} (must be Some(true): the person's)",
        person.as_ref().map(|p| p.id()),
        host::macos::ls_registered(&bundle)
    );
    println!("left alone in this run: {}", left_text());
    if let Some(mut p) = person {
        let _ = p.kill();
        let _ = p.wait();
    }
    let _ = std::fs::remove_file(&marks);
}

/// Windows x64, the stack sampler's opens (`probe::suspend::would_open`) of a decoy's process and one of
/// its threads: with the look's creation times they open; with another creation time for the process,
/// another for the thread, or another process's id for the thread, the handle is closed again at once
/// (the sampler never suspends or reads it).
#[cfg(all(windows, target_arch = "x86_64"))]
fn suspend_open() {
    use strobe::probe::suspend::would_open;
    let (c, d, k, ds) = decoy(&[], Kind::Tree);
    let (_, threads) = host::process_set_threads(d, None);
    let Some(t) = threads.iter().find(|t| t.pid == d) else { return println!("no thread of the decoy in the look") };
    println!("decoy {d} (created {ds}), its thread {} (created {})", t.tid, t.born);
    println!("the look's times: process, thread opened {:?}", would_open(d, ds, t.tid, t.born));
    println!("the process with another creation time: {:?}", would_open(d, ds - 1, t.tid, t.born));
    println!("the thread with another creation time: {:?}", would_open(d, ds, t.tid, t.born - 1));
    let other = k.map_or(std::process::id(), |k| k.0);
    println!("the thread under another process's id ({other}): {:?}", would_open(other, ds, t.tid, t.born));
    println!("decoy alive after {} · its child alive {}", runs(d), alive(k));
    end_decoy(c, &[k]);
}

/// Linux: thread-signal's decoy: a second thread, then sleeps.
#[cfg(target_os = "linux")]
fn threaded(secs: u64) {
    let t = std::thread::spawn(move || std::thread::sleep(Duration::from_secs(secs)));
    std::thread::sleep(Duration::from_secs(secs));
    let _ = t.join();
}

/// The left-alone log (`host::spawn::leave`), in the state folder the environment names.
fn left_log() {
    use host::spawn::{leave, left_alone_log, left_alone_recent, left_scope};
    let log = left_alone_log();
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(log.with_extension("log.old"));
    let day = Duration::from_secs(86_400);
    let scope = left_scope();
    for _ in 0..3 {
        leave(Some(1), "a test note", "made three times in one invocation");
    }
    println!("one note made 3 times: {} line(s) logged, {} reported", std::fs::read_to_string(&log).unwrap_or_default().lines().count(), scope.take().len());
    let threads: Vec<_> = (0..8u32).map(|t| std::thread::spawn(move || (0..200u32).for_each(|i| leave(Some(t * 1000 + i), "a test note", "one of many threads writing at once")))).collect();
    threads.into_iter().for_each(|t| t.join().expect("thread"));
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let whole = |l: &str| {
        l.split_once(' ').is_some_and(|(s, rest)| {
            s.parse::<u64>().is_ok() && matches!(rest, "a test note: made three times in one invocation" | "a test note: one of many threads writing at once")
        })
    };
    println!("8 threads x 200 notes: {} lines, {} not whole", text.lines().count(), text.lines().filter(|l| !whole(l)).count());
    println!("last-day count: {}", left_alone_recent(day));
    // Past 1 MB (lines from two days ago): the next note rotates the log, and the count reads both.
    let old = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) - 2 * 86_400;
    let filler: String = (0..20_000).map(|i| format!("{old} pid {i}, a test note: filler from two days ago\n")).collect();
    let _ = std::fs::write(&log, format!("{text}{filler}"));
    leave(Some(2), "a test note", "after the log passed 1 MB");
    let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    println!("after a note past 1 MB: log {} bytes, log.old {} bytes, last-day count {}", size(&log), size(&log.with_extension("log.old")), left_alone_recent(day));
    drop(scope);
}

/// Linux: a thread id of a decoy process, through the path used where pidfds are missing or blocked
/// (forced here only: `sigterm_opened_without_pidfd`): no signal; the decoy's own id then ends it.
#[cfg(target_os = "linux")]
fn thread_fallback() {
    let mut c = std::process::Command::new(std::env::current_exe().expect("exe")).args(["threaded", "60"]).spawn().expect("decoy");
    let d = c.id();
    std::thread::sleep(Duration::from_millis(400));
    let tid = std::fs::read_dir(format!("/proc/{d}/task")).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).find(|&t| t != d);
    let (Some(tid), Some(ds)) = (tid, host::start_time(d)) else {
        let _ = c.kill();
        let _ = c.wait();
        return println!("no second thread");
    };
    let ts = host::start_time(tid).expect("thread start");
    println!("decoy {d} (start {ds}), its thread {tid} (start {ts})");
    let sent = host::kill::sigterm_opened_without_pidfd(tid, ts);
    std::thread::sleep(Duration::from_millis(300));
    println!("without pidfds (tgkill), the thread id with its start: sent {sent} · decoy exited {:?}", c.try_wait().ok().flatten());
    let sent = host::kill::sigterm_opened_without_pidfd(d, ds);
    std::thread::sleep(Duration::from_millis(300));
    println!("without pidfds (tgkill), the decoy's own id with its start: sent {sent} · decoy exited {:?}", c.try_wait().ok().flatten());
    let _ = c.kill();
    let _ = c.wait();
}

/// Linux: a thread id of a decoy process (this harness's `threaded`) handed to the signal path with that
/// thread's own start time, as a pid that became another process's thread would be: no signal reaches
/// it (`pidfd_open` refuses a thread id: `EINVAL`), and the decoy survives. Then, last, what the WebKit
/// driver did before (its start-time check, then `kill` by that id): the whole decoy ends.
#[cfg(target_os = "linux")]
fn thread_signal() {
    let mut c = std::process::Command::new(std::env::current_exe().expect("exe")).args(["threaded", "60"]).spawn().expect("decoy");
    let d = c.id();
    std::thread::sleep(Duration::from_millis(400));
    let tid = std::fs::read_dir(format!("/proc/{d}/task")).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).find(|&t| t != d);
    let Some(tid) = tid else {
        let _ = c.kill();
        let _ = c.wait();
        return println!("no second thread");
    };
    let ts = host::start_time(tid).expect("thread start");
    println!("decoy {d}, its thread {tid} (start {ts})");
    println!("sigterm_opened(thread id, its start): sent {} · decoy alive {}", host::kill::sigterm_opened(tid, ts), runs(d));
    host::kill_opened(tid, ts);
    std::thread::sleep(Duration::from_millis(200));
    println!("kill_opened(thread id, its start): decoy alive {}", runs(d));
    // Before (ac46576e's webkit::end_browser, emulated): the start-time check passes for the thread,
    // and the plain signal by that id ends the whole process.
    if host::start_time(tid) == Some(ts) {
        unsafe { libc::kill(tid as i32, libc::SIGTERM) };
    }
    std::thread::sleep(Duration::from_millis(300));
    let _ = c.try_wait();
    println!("before the fix (emulated: start check, then kill by the thread id): decoy alive {}", host::start_time(d).is_some());
    let _ = c.kill();
    let _ = c.wait();
    println!("left alone in this run: {}", left_text());
}

/// macOS: `n` launches through Launch Services of an app that exits at once (`checkin exit`): each ends
/// as exited (`NoProcess`, or not adopted), never refused as a foreign process.
#[cfg(target_os = "macos")]
fn exit_ls(app_path: &str, n: u64) {
    use strobe::target::isolate;
    for i in 0..n {
        let (app, marks) = fixture_app(app_path, "exit", "ls");
        let home = isolate::prepare_home(&app, None, None).expect("home");
        let out = match isolate::spawn(&app, &home, &[], &[], false, None) {
            Ok(s) => {
                let r = format!("launched, pid {} (0: not adopted), start proven {}", s.pid, s.child.start().is_some());
                isolate::end_launch(Some(s.child), Some(&home.dir), true);
                r
            }
            Err(e) => {
                isolate::release_home(&home.dir);
                isolate::remove_home(&home.dir);
                format!("refused: {}", e.message)
            }
        };
        println!("{}: {out}", i + 1);
        let _ = std::fs::remove_file(&marks);
    }
    println!("left alone in this run: {}", left_text());
}
