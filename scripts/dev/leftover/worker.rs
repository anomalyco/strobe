// A bounded command fixture. The driver records its own child's pid; the audit never kills by pid.
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    #[cfg(windows)]
    if mode == "parent" || mode == "launch" {
        use std::os::windows::process::CommandExt;
        #[link(name = "kernel32")]
        extern "system" { fn GetConsoleCP() -> u32; }
        let attached = args.next().unwrap() == "attached";
        if mode == "parent" {
            let flag = if attached { 0x08000000 } else { 8 }; // CREATE_NO_WINDOW or DETACHED_PROCESS
            let status = Command::new(std::env::current_exe().unwrap()).arg("launch").arg(if attached { "attached" } else { "none" }).args(args).creation_flags(flag).status().unwrap();
            std::process::exit(status.code().unwrap_or(1));
        }
        assert_eq!(unsafe { GetConsoleCP() } != 0, attached);
        let exe = args.next().unwrap();
        let mut cmd = Command::new(&exe);
        if !attached && !exe.ends_with("strobew.exe") { cmd.creation_flags(8); }
        std::process::exit(cmd.args(args).status().unwrap().code().unwrap_or(1));
    }
    if mode == "sleep" {
        std::thread::sleep(Duration::from_secs(10));
    } else if mode == "wait" {
        assert!(Command::new(std::env::current_exe().unwrap()).status().unwrap().success());
    } else if mode == "leave" {
        let mut cmd = Command::new(args.next().unwrap());
        cmd.arg("sleep").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        if args.next().as_deref() == Some("separate") {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // A new, invisible console, not the root's console.
        }
        let child = cmd.spawn().unwrap();
        let mut receipt = std::fs::OpenOptions::new().create(true).append(true).open(std::env::var_os("LEFTOVER_RECEIPT").unwrap()).unwrap();
        writeln!(receipt, "{}", child.id()).unwrap();
    } else if mode == "behind" {
        // A worker *below* the real console host. Wait until it exists, not until it exits.
        let host = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/conhost.exe");
        let worker = std::env::current_exe().unwrap();
        let mut host = Command::new(host).arg("--headless").arg(&worker).arg("leave").arg(&worker).arg("separate")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let receipt = std::path::PathBuf::from(std::env::var_os("LEFTOVER_RECEIPT").unwrap());
        let until = std::time::Instant::now() + Duration::from_secs(4);
        while !receipt.exists() {
            assert!(host.try_wait().unwrap().is_none(), "console exited before the worker started");
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
