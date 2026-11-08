// Known-answer fixture for non-web targets (dev only, not shipped): a program whose true costs are known by
// construction, so what strobe prints for cli(), tty(), native() and `record pid:` can be checked against
// them. Plain Rust with no crates, built by rustc alone (no C compiler):
//   rustc -O -o known scripts/dev/known/known.rs        (known.exe on Windows)
// Every mode prints what it did as `strobe:metric truth_<name>=<value>` lines (a cli() target's metric.*).
//
//   spin <ms> [threads]         each thread runs <ms> of CPU (Linux: its own CPU clock; Windows: wall time on
//                               a core of its own, so CPU = wall when nothing else runs there)
//   sleep <ms>                  wall time, no CPU
//   timer <hz> <ms>             wakes <hz> times a second for <ms> (each wake a voluntary context switch)
//   alloc <mb> <ms> [free_ms]   <mb> MiB private, every page written, held <ms>; then freed, held <free_ms>
//   shm <mb> <ms>               <mb> MiB shared anonymous memory (Linux MAP_SHARED, Windows a pagefile
//                               section), every page written, held <ms>
//   mapfile <mb> <ms>           a <mb> MiB file mapped read-only, every page read, held <ms>
//   tree <k> <ms> [grand]       k children each `spin <ms>`, waited for; with `grand` each child runs its spin
//                               in a grandchild it waits for
//   orphan <ms>                 a child that spins <ms> and outlives its parent by 300 ms (not waited for)
//   breakaway <ms> [hops] [leave]  a child that spins <ms>, waited for; on Windows started with
//                               CREATE_BREAKAWAY_FROM_JOB (it asks to leave the job it was started in); with
//                               hops > 1 the child is `breakaway <ms> <hops - 1>` (each hop breaks away again);
//                               with `leave` the last spin is not waited for and outlives its parent by 300 ms.
//                               A hop that cannot start fails (exit code nonzero)
//   io <write_mb> <read_mb>     writes <write_mb> MiB to a file in 64 KiB writes, reads <read_mb> MiB of it
//   ready <ms> <text>           prints <text> after <ms>, then idles for 60 s
//   duty <pct> <threads> [s]    long-running: each thread busy <pct>% of every 100 ms (wall), for [s] (60) s
//   spawner <ms> <gap_ms> [s]   long-running: runs a child that spins <ms> and exits, then sleeps <gap_ms>,
//                               again for [s] (60) s: (<ms> / (<ms> + <gap_ms>)) of a core, all of it in
//                               children that end
//   wake <hz> [s]               long-running: one thread wakes <hz> times a second, for [s] (60) s
//   tick <hz> <work_ms> [s]     long-running planted work: one thread wakes <hz> times a second on a fixed
//                               schedule and spins <work_ms> of its CPU on each wake (`spin`), nothing else (no
//                               allocation, I/O or request), for [s] (60) s. CPU a second: <hz> x <work_ms> ms
//                               plus the wakes' own cost, which `tick <hz> 0` runs alone; so the true ratio of
//                               two works at one <hz> is known once that is read
//   hold <mb> [s]               long-running: <mb> MiB private written once, then idle, for [s] (60) s
//   tty <bytes> <delay_ms>      terminal app: raw input; on each key, after <delay_ms>, one synchronized
//                               frame that rewrites row 1 with <bytes> cells (alternating A and B); `q` quits.
//                               Prints `ready` on row 0 first.
//   frames <n> <bytes> <gap_ms> terminal app: <n> synchronized frames of <bytes> cells, <gap_ms> apart, then
//                               `done` and idles until `q`
use std::io::{Read, Write};
use std::thread;
use std::time::{Duration, Instant};

const MIB: usize = 1 << 20;
const PAGE: usize = 4096;

fn metric(name: &str, value: f64) {
    println!("strobe:metric truth_{name}={value}");
}

fn arg(args: &[String], i: usize, default: f64) -> f64 {
    args.get(i).and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[cfg(unix)]
mod os {
    // Linux's and macOS's numbers for the same names.
    #[cfg(target_os = "linux")]
    mod c {
        pub const THREAD_CPU: i32 = 3;
        pub const PROCESS_CPU: i32 = 2;
        pub const MAP_ANON: i32 = 0x20;
        pub const TERMIOS: usize = 60;
        pub const MAXRSS_PER_KIB: i64 = 1;
    }
    #[cfg(target_os = "macos")]
    mod c {
        pub const THREAD_CPU: i32 = 16;
        pub const PROCESS_CPU: i32 = 12;
        pub const MAP_ANON: i32 = 0x1000;
        pub const TERMIOS: usize = 72;
        pub const MAXRSS_PER_KIB: i64 = 1024;
    }
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn munmap(addr: *mut u8, len: usize) -> i32;
        fn tcgetattr(fd: i32, t: *mut u8) -> i32;
        fn tcsetattr(fd: i32, act: i32, t: *const u8) -> i32;
        fn getrusage(who: i32, r: *mut i64) -> i32;
    }
    fn clock_ns(clock: i32) -> u64 {
        let mut ts = Timespec { sec: 0, nsec: 0 };
        unsafe { clock_gettime(clock, &mut ts) };
        ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
    }
    /// This thread's CPU time in ns (CLOCK_THREAD_CPUTIME_ID).
    pub fn thread_cpu_ns() -> Option<u64> {
        Some(clock_ns(c::THREAD_CPU))
    }
    /// This process's CPU time in ns, every thread (CLOCK_PROCESS_CPUTIME_ID).
    pub fn process_cpu_ns() -> u64 {
        clock_ns(c::PROCESS_CPU)
    }
    /// (peak resident KiB, voluntary + involuntary switches) of this process.
    pub fn rusage_self() -> (i64, i64) {
        let mut r = [0i64; 18];
        unsafe { getrusage(0, r.as_mut_ptr()) };
        (r[4] / c::MAXRSS_PER_KIB, r[16] + r[17])
    }
    pub fn shared(len: usize) -> *mut u8 {
        unsafe { mmap(std::ptr::null_mut(), len, 3, 0x01 | c::MAP_ANON, -1, 0) }
    }
    pub fn unmap(p: *mut u8, len: usize) {
        unsafe { munmap(p, len) };
    }
    pub fn map_file(f: &std::fs::File, len: usize) -> *mut u8 {
        use std::os::fd::AsRawFd;
        unsafe { mmap(std::ptr::null_mut(), len, 1, 0x01, f.as_raw_fd(), 0) }
    }
    /// Raw input: no line editing, echo or signals, one byte a read.
    pub fn raw_input() {
        let mut t = [0u8; c::TERMIOS];
        unsafe {
            tcgetattr(0, t.as_mut_ptr());
            // Linux: four u32 flags, c_line, c_cc[32] (VTIME 5, VMIN 6). macOS: four u64 flags, c_cc[20]
            // (VMIN 16, VTIME 17), and its flags' other bit numbers.
            if cfg!(target_os = "linux") {
                let mut lflag = u32::from_ne_bytes([t[12], t[13], t[14], t[15]]);
                lflag &= !(0o2 | 0o10 | 0o1); // ICANON, ECHO, ISIG
                t[12..16].copy_from_slice(&lflag.to_ne_bytes());
                t[17 + 6] = 1;
                t[17 + 5] = 0;
            } else {
                let mut b = [0u8; 8];
                b.copy_from_slice(&t[24..32]);
                let mut lflag = u64::from_ne_bytes(b);
                lflag &= !(0x100 | 0x8 | 0x80); // ICANON, ECHO, ISIG
                t[24..32].copy_from_slice(&lflag.to_ne_bytes());
                t[32 + 16] = 1;
                t[32 + 17] = 0;
            }
            tcsetattr(0, 0, t.as_ptr());
        }
    }
}

#[cfg(windows)]
mod os {
    type Handle = isize;
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_ws: usize,
        ws: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn GetProcessTimes(p: Handle, c: *mut u64, e: *mut u64, k: *mut u64, u: *mut u64) -> i32;
        fn K32GetProcessMemoryInfo(p: Handle, c: *mut Counters, cb: u32) -> i32;
        fn CreateFileMappingW(f: Handle, sa: *const u8, prot: u32, hi: u32, lo: u32, name: *const u16) -> Handle;
        fn MapViewOfFile(m: Handle, access: u32, hi: u32, lo: u32, len: usize) -> *mut u8;
        fn UnmapViewOfFile(p: *const u8) -> i32;
        fn GetStdHandle(n: u32) -> Handle;
        fn GetConsoleMode(h: Handle, m: *mut u32) -> i32;
        fn SetConsoleMode(h: Handle, m: u32) -> i32;
    }
    pub fn thread_cpu_ns() -> Option<u64> {
        None
    }
    /// Kernel + user time of this process in ns (clock ticks: 15.6 ms steps).
    pub fn process_cpu_ns() -> u64 {
        let (mut c, mut e, mut k, mut u) = (0, 0, 0, 0);
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
        (k + u) * 100
    }
    /// (peak private commit KiB, 0): Windows keeps no per-process switch count.
    pub fn rusage_self() -> (i64, i64) {
        let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        ((c.peak_pagefile / 1024) as i64, 0)
    }
    pub fn shared(len: usize) -> *mut u8 {
        unsafe {
            let m = CreateFileMappingW(-1, std::ptr::null(), 4, (len >> 32) as u32, len as u32, std::ptr::null());
            MapViewOfFile(m, 0xF001F, 0, 0, len)
        }
    }
    pub fn unmap(p: *mut u8, _len: usize) {
        unsafe { UnmapViewOfFile(p) };
    }
    pub fn map_file(f: &std::fs::File, len: usize) -> *mut u8 {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            let m = CreateFileMappingW(f.as_raw_handle() as Handle, std::ptr::null(), 2, 0, 0, std::ptr::null());
            MapViewOfFile(m, 4, 0, 0, len)
        }
    }
    pub fn raw_input() {
        unsafe {
            let h = GetStdHandle(-10i32 as u32);
            let mut m = 0;
            GetConsoleMode(h, &mut m);
            SetConsoleMode(h, (m & !(0x1 | 0x2 | 0x4)) | 0x200);
            let o = GetStdHandle(-11i32 as u32);
            let mut m = 0;
            GetConsoleMode(o, &mut m);
            SetConsoleMode(o, m | 0x4);
        }
    }
}

/// Runs `ms` of this thread's CPU: by its own CPU clock where there is one, else by wall time.
fn spin(ms: f64) -> f64 {
    let ns = (ms * 1e6) as u64;
    let wall = Instant::now();
    let mut x = 0u64;
    match os::thread_cpu_ns() {
        Some(start) => {
            while os::thread_cpu_ns().unwrap() - start < ns {
                for i in 0..1000 {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
                }
            }
        }
        None => {
            while (wall.elapsed().as_nanos() as u64) < ns {
                for i in 0..1000 {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
                }
            }
        }
    }
    std::hint::black_box(x);
    wall.elapsed().as_secs_f64() * 1e3
}

fn touch(p: *mut u8, len: usize, value: u8) {
    for off in (0..len).step_by(PAGE) {
        unsafe { std::ptr::write_volatile(p.add(off), value) };
    }
}

fn read_pages(p: *const u8, len: usize) -> u64 {
    let mut sum = 0u64;
    for off in (0..len).step_by(PAGE) {
        sum += unsafe { std::ptr::read_volatile(p.add(off)) } as u64;
    }
    sum
}

fn ms(v: f64) -> Duration {
    Duration::from_secs_f64(v / 1e3)
}

fn self_exe() -> std::path::PathBuf {
    std::env::current_exe().unwrap()
}

fn frame(out: &mut impl Write, n: usize, cells: usize) -> usize {
    let ch = if n % 2 == 0 { b'A' } else { b'B' };
    let mut f = Vec::with_capacity(cells + 32);
    f.extend_from_slice(b"\x1b[?2026h\x1b[2;1H");
    f.extend(std::iter::repeat(ch).take(cells));
    f.extend_from_slice(b"\x1b[?2026l");
    out.write_all(&f).unwrap();
    out.flush().unwrap();
    f.len()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("");
    let cpu0 = os::process_cpu_ns();
    match mode {
        "spin" => {
            let (each, threads) = (arg(&args, 2, 100.0), arg(&args, 3, 1.0) as usize);
            let hs: Vec<_> = (0..threads).map(|_| thread::spawn(move || spin(each))).collect();
            for h in hs {
                h.join().unwrap();
            }
            metric("cpu_ms", each * threads as f64);
        }
        "sleep" => {
            thread::sleep(ms(arg(&args, 2, 100.0)));
            metric("cpu_ms", 0.0);
        }
        "timer" => {
            let (hz, total) = (arg(&args, 2, 100.0), arg(&args, 3, 1000.0));
            let start = Instant::now();
            let mut wakes = 0u64;
            while start.elapsed() < ms(total) {
                thread::sleep(ms(1e3 / hz));
                wakes += 1;
            }
            metric("wakes", wakes as f64);
            metric("wakes_per_s", wakes as f64 / start.elapsed().as_secs_f64());
        }
        "alloc" => {
            let mb = arg(&args, 2, 100.0) as usize;
            let mut v = vec![0u8; mb * MIB];
            touch(v.as_mut_ptr(), v.len(), 1);
            thread::sleep(ms(arg(&args, 3, 500.0)));
            std::hint::black_box(&v);
            drop(v);
            thread::sleep(ms(arg(&args, 4, 0.0)));
            metric("mib", mb as f64);
        }
        "shm" => {
            let mb = arg(&args, 2, 100.0) as usize;
            let p = os::shared(mb * MIB);
            touch(p, mb * MIB, 1);
            thread::sleep(ms(arg(&args, 3, 500.0)));
            os::unmap(p, mb * MIB);
            metric("mib", mb as f64);
        }
        "mapfile" => {
            let mb = arg(&args, 2, 100.0) as usize;
            let path = std::env::temp_dir().join(format!("known-map-{}", std::process::id()));
            std::fs::write(&path, vec![7u8; mb * MIB]).unwrap();
            let f = std::fs::File::open(&path).unwrap();
            let p = os::map_file(&f, mb * MIB);
            let sum = read_pages(p, mb * MIB);
            thread::sleep(ms(arg(&args, 3, 500.0)));
            os::unmap(p, mb * MIB);
            drop(f);
            let _ = std::fs::remove_file(&path);
            std::hint::black_box(sum);
            metric("mib", mb as f64);
        }
        "tree" => {
            let (k, each) = (arg(&args, 2, 2.0) as usize, arg(&args, 3, 100.0));
            let grand = args.get(4).is_some_and(|a| a == "grand");
            let kids: Vec<_> = (0..k)
                .map(|_| {
                    let mut c = std::process::Command::new(self_exe());
                    if grand {
                        c.args(["tree", "1", &each.to_string()]);
                    } else {
                        c.args(["spin", &each.to_string()]);
                    }
                    c.stdout(std::process::Stdio::null()).spawn().unwrap()
                })
                .collect();
            for mut c in kids {
                c.wait().unwrap();
            }
            metric("cpu_ms", each * k as f64);
        }
        "breakaway" => {
            let (each, hops) = (arg(&args, 2, 100.0), arg(&args, 3, 1.0) as u32);
            let leave = args.get(4).is_some_and(|a| a == "leave");
            let mut c = std::process::Command::new(self_exe());
            match (hops > 1, leave) {
                (true, _) => c.args(["breakaway", &each.to_string(), &(hops - 1).to_string()]).args(leave.then_some("leave")),
                // Left running: it outlives this one by 300 ms.
                (false, true) => c.args(["spin", &(each + 300.0).to_string()]),
                (false, false) => c.args(["spin", &each.to_string()]),
            };
            c.stdout(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0100_0000);
            }
            let mut child = c.spawn().unwrap();
            if hops > 1 || !leave {
                assert!(child.wait().unwrap().success());
            }
            metric("cpu_ms", each);
        }
        "orphan" => {
            let each = arg(&args, 2, 100.0);
            std::process::Command::new(self_exe())
                .args(["spin", &(each + 300.0).to_string()])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            thread::sleep(ms(each));
            metric("cpu_ms", 0.0);
        }
        "io" => {
            let (w, r) = (arg(&args, 2, 10.0) as usize, arg(&args, 3, 10.0) as usize);
            let path = std::env::temp_dir().join(format!("known-io-{}", std::process::id()));
            let chunk = vec![5u8; 64 * 1024];
            {
                let mut f = std::fs::File::create(&path).unwrap();
                for _ in 0..(w * MIB / chunk.len()) {
                    f.write_all(&chunk).unwrap();
                }
            }
            let mut buf = vec![0u8; 64 * 1024];
            let mut f = std::fs::File::open(&path).unwrap();
            let mut got = 0;
            while got < r * MIB {
                let n = f.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                got += n;
            }
            drop(f);
            let _ = std::fs::remove_file(&path);
            metric("write_bytes", (w * MIB) as f64);
            metric("read_bytes", got as f64);
        }
        "ready" => {
            thread::sleep(ms(arg(&args, 2, 500.0)));
            println!("{}", args.get(3).map(String::as_str).unwrap_or("READY"));
            std::io::stdout().flush().unwrap();
            thread::sleep(Duration::from_secs(60));
            return;
        }
        "duty" => {
            let (pct, threads, secs) = (arg(&args, 2, 50.0), arg(&args, 3, 1.0) as usize, arg(&args, 4, 60.0));
            let hs: Vec<_> = (0..threads)
                .map(|_| {
                    thread::spawn(move || {
                        let start = Instant::now();
                        let mut next = Instant::now();
                        while start.elapsed().as_secs_f64() < secs {
                            spin(pct);
                            next += ms(100.0);
                            if let Some(d) = next.checked_duration_since(Instant::now()) {
                                thread::sleep(d);
                            }
                        }
                    })
                })
                .collect();
            for h in hs {
                h.join().unwrap();
            }
        }
        "spawner" => {
            let (each, gap, secs) = (arg(&args, 2, 100.0), arg(&args, 3, 100.0), arg(&args, 4, 60.0));
            let start = Instant::now();
            while start.elapsed().as_secs_f64() < secs {
                std::process::Command::new(self_exe()).args(["spin", &each.to_string()]).stdout(std::process::Stdio::null()).status().unwrap();
                thread::sleep(ms(gap));
            }
        }
        "wake" => {
            let (hz, secs) = (arg(&args, 2, 100.0), arg(&args, 3, 60.0));
            let start = Instant::now();
            while start.elapsed().as_secs_f64() < secs {
                thread::sleep(ms(1e3 / hz));
            }
        }
        "tick" => {
            let (hz, work, secs) = (arg(&args, 2, 50.0), arg(&args, 3, 5.0), arg(&args, 4, 60.0));
            let period = ms(1e3 / hz);
            let start = Instant::now();
            let mut next = start;
            while start.elapsed().as_secs_f64() < secs {
                if work > 0.0 {
                    spin(work);
                }
                // On a fixed schedule, so a second has <hz> wakes whatever each one's work (below the period).
                next += period;
                if let Some(d) = next.checked_duration_since(Instant::now()) {
                    thread::sleep(d);
                }
            }
        }
        "hold" => {
            let mb = arg(&args, 2, 100.0) as usize;
            let mut v = vec![0u8; mb * MIB];
            touch(v.as_mut_ptr(), v.len(), 1);
            thread::sleep(Duration::from_secs_f64(arg(&args, 3, 60.0)));
            std::hint::black_box(&v);
        }
        "tty" | "frames" => {
            os::raw_input();
            let mut out = std::io::stdout();
            out.write_all(b"\x1b[2J\x1b[1;1Hready").unwrap();
            out.flush().unwrap();
            if mode == "frames" {
                let (n, cells, gap) = (arg(&args, 2, 10.0) as usize, arg(&args, 3, 40.0) as usize, arg(&args, 4, 50.0));
                for i in 0..n {
                    thread::sleep(ms(gap));
                    frame(&mut out, i, cells);
                }
                out.write_all(b"\x1b[3;1Hdone").unwrap();
                out.flush().unwrap();
            }
            let (cells, delay) = (arg(&args, 2, 40.0) as usize, arg(&args, 3, 20.0));
            let mut n = 0;
            let mut buf = [0u8; 64];
            loop {
                let got = std::io::stdin().read(&mut buf).unwrap_or(0);
                if got == 0 || buf[..got].contains(&b'q') {
                    break;
                }
                if mode == "tty" {
                    thread::sleep(ms(delay));
                    frame(&mut out, n, cells);
                    n += 1;
                }
            }
            return;
        }
        _ => {
            eprintln!("usage: known <spin|sleep|timer|alloc|shm|mapfile|tree|breakaway|orphan|io|ready|duty|wake|tick|hold|tty|frames> …");
            std::process::exit(2);
        }
    }
    let (peak_kib, switches) = os::rusage_self();
    eprintln!(
        "known: own cpu {:.1} ms, peak {} KiB, switches {}",
        (os::process_cpu_ns() - cpu0) as f64 / 1e6,
        peak_kib,
        switches
    );
}
