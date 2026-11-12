//! A native app on Wayland (SPEC §13.7 "Wayland"): Wayland shows no client another client's windows
//! and takes no input from one, so strobe runs the compositor itself, one per launch: a headless sway
//! in a private folder (`<state>/run/wl-<pid>-<id>`, 0700, its owner file naming this strobe), with
//! one virtual output (1920×1080 at scale 1, every window floating, no borders, no XWayland). Only the
//! app and strobe connect to it; strobe finds and places the app's windows through its IPC, copies its
//! pixels and gives its input through its seat (`host::wayland`).
//!
//! The compositor composites for no display: its frames tick on a timer, so what reached the screen
//! (frames, input latency, the display's rate, a launch's pixels) is refused `Headless`. It runs on
//! strobe's CPUs and outside the app's process set, so `cpu.work` never holds it. Without a GPU
//! render node it composites in software (pixman).
//!
//! The launch has a session of its own, never the user's (rule 10): a session bus (`dbus-daemon
//! --session`) and the accessibility bus on it (`at-spi-bus-launcher`), with the app's home as their
//! home, so what they activate (a settings service, a notification daemon, a keyring) writes into the
//! home and never reaches the user's desktop. Every process the session starts carries its folder's
//! stamp, so a strobe that dies leaves nothing the next one does not end (`isolate::reap_dead_owners`).

use super::isolate;
use crate::host::dbus::{Conn, Value};
use crate::host::spawn::Owned;
use crate::host::wayland::Compositor;
use crate::model::{Error, Refusal, Result, ValidatorId};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The compositor's configuration: one output, every window floating and borderless, no XWayland.
const CONFIG: &str = "output HEADLESS-1 resolution 1920x1080@60Hz position 0 0 scale 1
for_window [all] floating enable
default_border none
default_floating_border none
xwayland disable
";

/// One launch's display: the compositor and the buses strobe started for it.
pub struct Session {
    pub dir: PathBuf,
    /// Set once it answers (`start`).
    compositor: Option<Arc<Compositor>>,
    /// The session bus, and the accessibility bus on it.
    bus: String,
    a11y: Option<String>,
    /// The processes, ended last to first: the compositor, then the buses.
    children: Vec<Owned>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session({}, {})", self.dir.display(), self.describe())
    }
}

/// The programs a launch on Wayland needs, and what installs each.
const PROGRAMS: &[(&str, &str)] = &[
    ("sway", "sway (apt install sway, dnf install sway, pacman -S sway)"),
    ("dbus-daemon", "dbus (apt install dbus-daemon, dnf install dbus-daemon, pacman -S dbus)"),
    ("at-spi-bus-launcher", "at-spi2-core (apt install at-spi2-core, dnf install at-spi2-core, pacman -S at-spi2-core)"),
];

/// The first of `names` on PATH or in the usual libexec folders.
fn program(name: &str) -> Option<PathBuf> {
    let libexec: Vec<PathBuf> = ["/usr/libexec", "/usr/lib/at-spi2-core", "/usr/lib/at-spi2", "/usr/lib"].iter().map(|d| Path::new(d).join(name)).collect();
    crate::host::find_exe(name, &libexec)
}

/// Why this machine cannot run strobe's compositor and the launch's buses, if it cannot.
pub fn unavailable() -> Option<String> {
    let missing: Vec<&str> = PROGRAMS.iter().filter(|(p, _)| program(p).is_none()).map(|(_, how)| *how).collect();
    (!missing.is_empty()).then(|| format!("install {}", missing.join("; ")))
}

/// Whether the machine has a GPU render node for the compositor; without one it composites in software.
fn render_node() -> bool {
    std::fs::read_dir("/dev/dri").into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
}

/// The compositor (and the app on it) would draw in software here: no GPU render node.
pub fn software() -> bool {
    !render_node()
}

fn last_lines(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join(" · ")
}

impl Session {
    /// Starts the launch's session bus and accessibility bus with `home` (the app home's `HOME` and
    /// `XDG_*` folders), then the compositor. `auto`: the launch is on Wayland only because this
    /// session has no X11 display, which a refusal says.
    pub fn start(home: &[(String, String)], auto: bool) -> Result<Session> {
        if let Some(why) = unavailable() {
            let what = if auto {
                "this session has no X11 display (DISPLAY is unset), so strobe runs the native app on a Wayland compositor it starts"
            } else {
                "the target runs on a Wayland compositor strobe starts"
            };
            return Err(Refusal::new(ValidatorId::NotOnThisOs, format!("{what}, and this machine lacks what it needs"))
                .fix(format!("{why}; or run on an X11 display (xvfb-run -a strobe …, display: \"x11\")"))
                .into());
        }
        let program = |p: &str| program(p).ok_or_else(|| Error::msg(format!("{p} is not installed")));
        let dir = crate::host::paths::run().join(format!("wl-{}-{}", std::process::id(), crate::host::paths::csprng_hex(3)?));
        crate::host::paths::owner_only_dir(&dir);
        isolate::claim(&dir)?;
        // Stamps for everything the session starts: its folder, and this strobe.
        let stamps = isolate::launch_env(&dir);
        let bus = format!("unix:path={}", dir.join("bus").display());
        let mut s = Session { dir: dir.clone(), compositor: None, bus: bus.clone(), a11y: None, children: Vec::new() };
        // Every process of the session: the app's home and the session's runtime folder, the launch's
        // bus, no display of the user's, and stamped; on strobe's CPUs (it inherits them).
        let spawn = |s: &mut Session, cmd: &mut Command, log: &str| -> Result<()> {
            for k in ["DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK", "I3SOCK", "AT_SPI_BUS_ADDRESS"] {
                cmd.env_remove(k);
            }
            cmd.envs(home.iter().cloned()).env("XDG_RUNTIME_DIR", &dir).env("DBUS_SESSION_BUS_ADDRESS", &bus).envs(stamps.clone());
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(std::fs::File::create(dir.join(log)).map(Stdio::from).unwrap_or_else(|_| Stdio::null()));
            let child = crate::host::spawn_owned(cmd, None, 0, Some(&dir)).map_err(|e| Error::msg(format!("start {}: {e}", cmd.get_program().to_string_lossy())))?;
            s.children.push(child);
            Ok(())
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut cmd = Command::new(program("dbus-daemon")?);
        cmd.args(["--session", "--nofork", "--nopidfile", "--nosyslog"]).arg(format!("--address={bus}"));
        spawn(&mut s, &mut cmd, "dbus.log")?;
        while Conn::open(&bus, false).is_err() {
            if Instant::now() > deadline {
                return Err(Error::msg(format!("the session bus strobe started did not answer: {}", last_lines(&dir.join("dbus.log")))));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        spawn(&mut s, &mut Command::new(program("at-spi-bus-launcher")?), "at-spi.log")?;
        // The accessibility bus's address, once it answers.
        while s.a11y.is_none() {
            s.a11y = Conn::open(&bus, false).ok().and_then(|mut c| {
                let addr = c.call("org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress", &[]).ok()?.first().and_then(Value::as_str).map(str::to_string)?;
                Conn::open(&addr, false).ok().map(|_| addr)
            });
            if s.a11y.is_none() {
                if Instant::now() > deadline {
                    return Err(Error::msg(format!("the accessibility bus strobe started did not answer: {}", last_lines(&dir.join("at-spi.log")))));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        std::fs::write(dir.join("config"), CONFIG)?;
        let renderer = if render_node() { "gles2" } else { "pixman" };
        let sway = program("sway")?;
        let mut cmd = Command::new(&sway);
        cmd.arg("-c").arg(dir.join("config")).env("WLR_BACKENDS", "headless").env("WLR_LIBINPUT_NO_DEVICES", "1").env("WLR_RENDERER", renderer);
        spawn(&mut s, &mut cmd, "sway.log")?;
        let (socket, ipc) = loop {
            let names: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
            let socket = names.iter().find(|n| n.starts_with("wayland-") && !n.ends_with(".lock"));
            let ipc = names.iter().find(|n| n.starts_with("sway-ipc.") && n.ends_with(".sock"));
            if let (Some(a), Some(b)) = (socket, ipc) {
                break (dir.join(a), dir.join(b));
            }
            if s.children.last_mut().and_then(|c| c.exited()).is_some() || Instant::now() > deadline {
                return Err(Error::msg(format!("the Wayland compositor (sway) did not start: {}", last_lines(&dir.join("sway.log")))));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let version = crate::host::output(Command::new(&sway).arg("--version")).unwrap_or_else(|| "sway".into());
        let compositor = crate::host::wayland::register(&socket, &ipc, renderer, &version, &bus);
        s.compositor = Some(compositor.clone());
        while compositor.output().is_none() {
            if Instant::now() > deadline {
                return Err(Error::msg(format!("the Wayland compositor (sway) shows no output: {}", last_lines(&dir.join("sway.log")))));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // A keyboard and a pointer on the seat before the app starts, as a person's seat has.
        super::a11y::input::prepare_seat(&compositor)?;
        Ok(s)
    }

    /// What the app's environment gets: the compositor's socket and the session type, the toolkits'
    /// Wayland backends, the session's runtime folder and its buses. `DISPLAY` is removed (`UNSET`).
    pub fn env(&self) -> Vec<(String, String)> {
        let socket = self.compositor.as_ref().map(|c| c.socket.display().to_string()).unwrap_or_default();
        let mut env: Vec<(String, String)> = [
            ("WAYLAND_DISPLAY", socket),
            ("XDG_SESSION_TYPE", "wayland".into()),
            ("GDK_BACKEND", "wayland".into()),
            ("QT_QPA_PLATFORM", "wayland".into()),
            ("SDL_VIDEODRIVER", "wayland".into()),
            ("XDG_RUNTIME_DIR", self.dir.display().to_string()),
            ("DBUS_SESSION_BUS_ADDRESS", self.bus.clone()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        if let Some(a) = &self.a11y {
            env.push(("AT_SPI_BUS_ADDRESS".into(), a.clone()));
        }
        env
    }

    /// The compositor and how it composites: `sway version 1.10.1, headless, pixman`.
    pub fn describe(&self) -> String {
        self.compositor.as_ref().map_or_else(|| "sway (not started)".into(), |c| format!("{}, headless, {}", c.name, c.renderer))
    }

    /// It composites in software.
    pub fn software(&self) -> bool {
        self.compositor.as_ref().is_some_and(|c| c.renderer == "pixman")
    }

    pub fn compositor(&self) -> Option<&Arc<Compositor>> {
        self.compositor.as_ref()
    }
}

/// Variables a launch on strobe's compositor never inherits: another display, and the user's
/// accessibility bus.
pub const UNSET: &[&str] = &["DISPLAY", "WAYLAND_SOCKET", "AT_SPI_BUS_ADDRESS"];

impl Drop for Session {
    fn drop(&mut self) {
        while let Some(c) = self.children.pop() {
            drop(c);
        }
        isolate::release_home(&self.dir);
        isolate::remove_home(&self.dir);
    }
}
