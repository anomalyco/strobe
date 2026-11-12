// Known-answer fixture for the Windows input-idle watch (`host::win::input_idle`; dev only, not shipped):
// a windowless program whose message loop starts after a delay given on its command line. It opens no
// window and takes no focus. Plain Rust with no crates, built by rustc alone (no C compiler):
//   rustc -O --edition 2024 -o idle.exe scripts/dev/bounce/idle.rs
// The window manager's library (user32) is loaded only by the modes that use it.
//
//   get <ms> [life_ms]      loads user32 at once, sleeps <ms>, then waits in GetMessage (a timer ends it
//                           at [life_ms], 3000, from its start): the loop of an ordinary app
//   wait <ms> [life_ms]     as `get`, waiting in MsgWaitForMultipleObjects (QS_ALLINPUT) instead
//   peek <ms> [life_ms]     as `get`, polling PeekMessage every 5 ms instead (Windows calls a thread that
//                           only peeks idle after it has spun for a while)
//   late <ms> [life_ms]     sleeps <ms> before loading user32, then waits as `get`
//   nopump <ms> [life_ms]   loads user32 at once and never waits for a message: never idle
//   none <ms> [life_ms]     never loads user32 itself: still a GUI-subsystem program, which Windows gives an
//                           input idle state from its creation, never set here
//
// Prints `start_ns=<qpc ns>` at its start, `user32=<0|1>` (whether user32 is loaded then), and `pump_ns=`
// just before its first wait for a message, on the clock of strobe's `host::now_ns` (QPC in ns).
#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::io::Write;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(t: *mut i64) -> i32;
    fn QueryPerformanceFrequency(f: *mut i64) -> i32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

fn qpc_ns() -> u64 {
    let (mut t, mut f) = (0i64, 0i64);
    unsafe {
        QueryPerformanceCounter(&mut t);
        QueryPerformanceFrequency(&mut f);
    }
    (t as i128 * 1_000_000_000 / f.max(1) as i128) as u64
}

#[repr(C)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

const WM_TIMER: u32 = 0x0113;
const QS_ALLINPUT: u32 = 0x04FF;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn main() {
    let start = qpc_ns();
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("get");
    let ms: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(100);
    let life: u64 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(3000);
    let mut out = std::io::stdout();
    let loaded = !unsafe { GetModuleHandleW(wide("user32.dll").as_ptr()) }.is_null();
    let _ = writeln!(out, "start_ns={start}\nuser32={}", u8::from(loaded));
    let _ = out.flush();
    let early = matches!(mode, "get" | "wait" | "peek" | "nopump");
    let mut module = if early { unsafe { LoadLibraryW(wide("user32.dll").as_ptr()) } } else { std::ptr::null_mut() };
    std::thread::sleep(std::time::Duration::from_millis(ms));
    if mode == "late" {
        module = unsafe { LoadLibraryW(wide("user32.dll").as_ptr()) };
    }
    let end = start + life * 1_000_000;
    if !matches!(mode, "get" | "wait" | "peek" | "late") {
        while qpc_ns() < end {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        return;
    }
    let f = |name: &std::ffi::CStr| unsafe { GetProcAddress(module, name.as_ptr() as *const u8) };
    type Get = unsafe extern "system" fn(*mut Msg, *mut c_void, u32, u32) -> i32;
    type Peek = unsafe extern "system" fn(*mut Msg, *mut c_void, u32, u32, u32) -> i32;
    type Wait = unsafe extern "system" fn(u32, *const c_void, i32, u32, u32) -> u32;
    type Timer = unsafe extern "system" fn(*mut c_void, usize, u32, *const c_void) -> usize;
    let (get, peek, wait, timer): (Get, Peek, Wait, Timer) =
        unsafe { (std::mem::transmute(f(c"GetMessageW")), std::mem::transmute(f(c"PeekMessageW")), std::mem::transmute(f(c"MsgWaitForMultipleObjects")), std::mem::transmute(f(c"SetTimer"))) };
    let left = (end.saturating_sub(qpc_ns()) / 1_000_000).max(1) as u32;
    let mut msg: Msg = unsafe { std::mem::zeroed() };
    let at = qpc_ns();
    match mode {
        "peek" => {
            let _ = writeln!(out, "pump_ns={at}");
            let _ = out.flush();
            while qpc_ns() < end {
                while unsafe { peek(&mut msg, std::ptr::null_mut(), 0, 0, 1) } != 0 {}
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        "wait" => {
            let _ = writeln!(out, "pump_ns={at}");
            let _ = out.flush();
            while qpc_ns() < end {
                unsafe { wait(0, std::ptr::null(), 0, ((end.saturating_sub(qpc_ns())) / 1_000_000).max(1) as u32, QS_ALLINPUT) };
                while unsafe { peek(&mut msg, std::ptr::null_mut(), 0, 0, 1) } != 0 {}
            }
        }
        _ => {
            unsafe { timer(std::ptr::null_mut(), 0, left, std::ptr::null()) };
            let at = qpc_ns();
            let _ = writeln!(out, "pump_ns={at}");
            let _ = out.flush();
            while unsafe { get(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                if msg.message == WM_TIMER {
                    break;
                }
            }
        }
    }
}
