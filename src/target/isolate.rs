//! Launching an installed app fully isolated, with guards on the user's own state (SPEC §13.2).
//!
//! Generic: every per-user folder points into a fresh private home. What one app needs beyond that
//! (its own env vars, seed files, the user state to guard, URL schemes it registers) is an app
//! profile in strobe.json `apps`, never code here. Electron apps (`electron.rs`), apps on the OS's
//! webview (`webview.rs`) and native apps (`native.rs`) all launch through this.

use crate::host;
use crate::host::spawn::{HOME_VAR, OWNER_VAR, Owned, Stamp, Stamped, home_key as key, owner_stamp, parse_owner, stamp_of, stamped};
use crate::model::{Error, Result};
use serde_json::{Value, json};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// How strobe reaches an app's UI: the profile's `kind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    /// An Electron app: a debugging port (`--remote-debugging-port`), CDP.
    #[default]
    Electron,
    /// An app that embeds the OS's webview: the webview's own debugging port (`webview.rs`).
    Webview,
    /// No browser engine (or one strobe does not attach to): from outside and through the OS's
    /// accessibility tree (§13.7).
    Native,
}

impl AppKind {
    pub const NAMES: &[&str] = &["electron", "webview", "native"];

    pub fn name(self) -> &'static str {
        match self {
            AppKind::Electron => "electron",
            AppKind::Webview => "webview",
            AppKind::Native => "native",
        }
    }

    fn parse(name: &str) -> Option<AppKind> {
        [AppKind::Electron, AppKind::Webview, AppKind::Native].into_iter().find(|k| k.name() == name)
    }
}

impl std::fmt::Display for AppKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// One app to launch: an executable plus what its profile adds to the generic isolation.
#[derive(Clone, Debug, Default)]
pub struct App {
    pub id: String,
    pub exe: PathBuf,
    pub kind: AppKind,
    env: Vec<(String, String)>,
    unset: Vec<String>,
    dirs: Vec<String>,
    files: Vec<(String, Value)>,
    guards: Vec<GuardSpec>,
    protocols: Vec<String>,
    args: Vec<String>,
    /// Folders of the user's whose data the profile's `env` moves into the home (`moved`), so the
    /// macOS Library rule does not count them; the profile's `guard` watches them instead.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    moved: Vec<PathBuf>,
    /// The profile says `window: false`: the app opens no window (a tray app, a service).
    pub windowless: bool,
    /// The profile's `gpuCache` (`warm` or `cold`), when it names one.
    pub gpu_cache_mode: Option<String>,
    /// Where the GPU shader caches of this app's launches persist between them (`gpuCache: "warm"`):
    /// lent into each home before the launch and taken back when the home goes (`lend_gpu_caches`).
    pub gpu_cache: Option<PathBuf>,
    /// Set: something appearing in the user's Documents or Desktop while this launch runs is noted
    /// here for the experiment (a bench's later launches of a side); unset, it stops the launch.
    pub folder_notes: Option<Notes>,
    /// The profile's `motion.reduce`: what makes this app prefer reduced motion, as its owner declares it.
    motion_contract: Option<MotionContract>,
    /// The launch asks for reduced motion (`with_reduced_motion`): Electron and WebView2 launches add
    /// Chromium's switch (`motion::SWITCH`).
    pub reduce_motion: bool,
    /// Files merged over what the home holds once its seed and the profile's files are in place (JSON
    /// objects key by key, deeply; anything else replaced): the motion contract's.
    merged_files: Vec<(String, Value)>,
    /// The display a native app opens its windows on (Linux): the profile's or the target's `display`,
    /// else Wayland when this session has no X11 display (`wayland`).
    display: Option<Display>,
    /// How `spawn` starts the app (`with_launch_path`).
    launch: LaunchPath,
    /// `spawn` also watches the launch's OS launch feedback for this long (`with_feedback`).
    feedback: Option<std::time::Duration>,
}

/// How a launch starts an app: as strobe's own child, or (macOS) through Launch Services as the Dock
/// and Finder start one. Recorded on each launch block (`Spawned::path`): the two differ by about 30 ms
/// before the app runs, so two sides launched on different paths are not compared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPath {
    /// strobe spawns the executable (`posix_spawn`, `CreateProcessW`): its output is strobe's to read.
    #[default]
    Spawn,
    /// macOS: `NSWorkspace` opens the app bundle (`host::spawn::open_owned`): no output, no exit code.
    LaunchServices,
}

impl LaunchPath {
    pub fn name(self) -> &'static str {
        match self {
            LaunchPath::Spawn => "spawn",
            LaunchPath::LaunchServices => "launch_services",
        }
    }
}

/// The path a launch of `app` takes (the rule, SPEC §9.6): on macOS an app bundle Launch Services can
/// account for (its executable is the bundle's own, with a `CFBundleIdentifier`) goes through Launch
/// Services, as a person's launch does, unless the case reads milestones from the app's output
/// (`output`: Launch Services gives strobe none, so it is spawned directly, about 30 ms faster to
/// `main` than a Dock click: its `launch.bounces` and `launch.checkin` are refused). Everything else is
/// spawned.
pub fn launch_path(app: &App, output: bool) -> LaunchPath {
    #[cfg(target_os = "macos")]
    if !output && host::macos::bundle_of(&app.exe).is_ok_and(|b| b.id.is_some()) {
        return LaunchPath::LaunchServices;
    }
    let _ = (app, output);
    LaunchPath::Spawn
}

/// Where a native app shows its windows on Linux (SPEC §13.7): the X server of the session, or a
/// Wayland compositor strobe runs for the launch (`target::wayland`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    X11,
    Wayland,
}

impl Display {
    pub fn name(self) -> &'static str {
        match self {
            Display::X11 => "x11",
            Display::Wayland => "wayland",
        }
    }

    pub fn parse(name: &str) -> Result<Display> {
        match name {
            "x11" => Ok(Display::X11),
            "wayland" => Ok(Display::Wayland),
            other => Err(Error::usage(format!("display is \"wayland\" or \"x11\", not {other:?}"))),
        }
    }
}

/// An app profile's `motion.reduce` (strobe.json): the arguments, environment and files that make
/// the app prefer reduced motion in a launch.
#[derive(Clone, Debug, Default)]
struct MotionContract {
    args: Vec<String>,
    env: Vec<(String, String)>,
    files: Vec<(String, Value)>,
}

#[derive(Clone, Debug)]
struct GuardSpec {
    file: PathBuf,
    pid: Option<String>,
    url: Option<String>,
}

impl App {
    /// An executable with no profile: generic isolation only. A relative one is from the current folder
    /// (`from_dir`).
    pub fn bare(exe: &Path) -> App {
        App { id: exe.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), exe: bundle_exe(from_cwd(exe)), ..App::default() }
    }

    /// The same profile reached another way (`exe:<path> --native`, a `webview(path)` target).
    pub fn as_kind(mut self, kind: AppKind) -> App {
        self.kind = kind;
        self
    }

    /// The same profile with another build of the app, such as a variant's `exe:<path>` (a relative one
    /// from the current folder, `from_dir`).
    pub fn with_exe(mut self, exe: &Path) -> App {
        self.exe = bundle_exe(from_cwd(exe));
        self
    }

    /// Extra arguments and environment on top of the profile's (a bench target's own `args` and `env`).
    pub fn with_extra(mut self, args: &[String], env: &[(String, String)]) -> App {
        self.args.extend(args.iter().cloned());
        self.env.extend(env.iter().cloned());
        self
    }

    /// Files written into the home over the profile's (a bench target's own `files`).
    pub fn with_files(mut self, files: &serde_json::Map<String, Value>) -> App {
        for (k, v) in files {
            self.files.retain(|(f, _)| f != k);
            self.files.push((k.clone(), v.clone()));
        }
        self
    }

    /// Variables (or `PREFIX_*` families) the launch removes, after the profile's.
    pub fn with_unset(mut self, unset: &[String]) -> App {
        self.unset.extend(unset.iter().cloned());
        self
    }

    fn unsets(&self, key: &str) -> bool {
        env_unset(&self.unset, key)
    }

    /// This launch prefers reduced motion (SPEC 13.1 "Reduced motion"): the profile's declared
    /// `motion.reduce` (its args and env after the target's, its files merged over the home's), and for
    /// an app strobe reaches through Chromium (`chromium`: Electron, WebView2) Chromium's own switch. A
    /// native target's app has only its declaration: without one it is refused `NotForTarget`.
    pub fn with_reduced_motion(mut self, chromium: bool) -> Result<App> {
        match self.motion_contract.clone() {
            Some(c) => {
                self.args.extend(c.args);
                self.env.extend(c.env);
                self.merged_files.extend(c.files);
            }
            None if !chromium => return Err(no_motion_contract(&self.id).into()),
            None => {}
        }
        self.reduce_motion = chromium;
        Ok(self)
    }

    /// The profile declares `motion.reduce`.
    pub fn declares_motion(&self) -> bool {
        self.motion_contract.is_some()
    }

    /// The display its profile (or a target over it) asks for, if any.
    pub fn display(&self) -> Option<Display> {
        self.display
    }

    /// The display a target (or `--wayland`) asks for, over the profile's.
    pub fn with_display(mut self, display: Option<Display>) -> App {
        self.display = display.or(self.display);
        self
    }

    /// Whether a launch runs on a compositor strobe starts (`target::wayland`): a native app on Linux
    /// that asks for Wayland, or asks for nothing in a session with no X11 display.
    pub fn wayland(&self) -> bool {
        cfg!(target_os = "linux")
            && self.kind == AppKind::Native
            && match self.display {
                Some(d) => d == Display::Wayland,
                None => std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty()),
            }
    }

    /// How `spawn` starts the app (`launch_path` picks it). `LaunchServices` (macOS): through Launch
    /// Services (`host::spawn::open_owned`), as the Dock and Finder start it, about 30 ms slower than
    /// strobe's direct spawn before the app runs: what a person's launch pays. Its executable must be an
    /// app bundle's with a bundle id, and the launch gives strobe none of the app's output; `spawn`
    /// refuses it elsewhere.
    pub fn with_launch_path(mut self, path: LaunchPath) -> App {
        self.launch = path;
        self
    }

    /// `with_launch_path(LaunchPath::LaunchServices)`.
    pub fn through_launch_services(self) -> App {
        self.with_launch_path(LaunchPath::LaunchServices)
    }

    /// `spawn` starts the launch's OS launch-feedback watch itself, at the launch, for `within`: the
    /// X11 startup notification announced before the launch's time starts (its id in the app's
    /// environment), Windows' input idle from right after the resume, macOS's check-in from the pid
    /// and start time the launch read (`Spawned::feedback`). Only for a case that asks for that metric
    /// (or an untimed check, `look`): the macOS watch wakes `launchservicesd` every 5 ms.
    pub fn with_feedback(mut self, within: std::time::Duration) -> App {
        self.feedback = Some(within);
        self
    }
}

/// How a launch case reports `launch.bounces` (`bounces`; the owner's ruling, design addendum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bounces {
    /// macOS, a Dock launch: recorded on every launch case, declared or not.
    Dock,
    /// Off macOS, declared by the case (`primary` or `metrics`): `launch.ready` ÷ `host::DOCK_BOUNCE_MS`,
    /// with `borrowed_bounce_note()` beside the value on every surface that shows it.
    Borrowed,
    /// Not recorded: off macOS and not declared, or on macOS a launch with no Dock launch that the case
    /// did not declare it for.
    Off,
}

/// Whether and how a launch case reports `launch.bounces`: on macOS automatically for a Dock launch
/// (`dock_launch`) the Dock sees as a person's (not one `output` milestones forced to `Spawn`); off
/// macOS only when the case declares it, in a borrowed unit. A refusal only when the case declares it
/// and macOS has no Dock launch to count (each `NoFeedback` maps to a refusal by `feedback_refusal`).
/// The launch-case and windowed-target requirements (`web`, `command`, `tty`, `window: false`: `NotForTarget`)
/// are the bench's, on every OS.
pub fn bounces(app: &App, declared: bool, path: LaunchPath, output: bool) -> std::result::Result<Bounces, host::NoFeedback> {
    if !cfg!(target_os = "macos") {
        return Ok(if declared { Bounces::Borrowed } else { Bounces::Off });
    }
    let dock = dock_launch(app).and_then(|()| if output && path == LaunchPath::Spawn { Err(host::NoFeedback::OutputMilestones) } else { Ok(()) });
    match dock {
        Ok(()) => Ok(Bounces::Dock),
        Err(why) if declared => Err(why),
        Err(_) => Ok(Bounces::Off),
    }
}

/// The note `Bounces::Borrowed` puts beside the value, in plain words.
pub fn borrowed_bounce_note() -> String {
    format!("this OS has no Dock: a bounce here is a borrowed unit, 1 bounce = {} ms, the macOS Dock's launch bounce", host::DOCK_BOUNCE_MS)
}

/// The refusal a launch-feedback metric (`launch.checkin`, `launch.input_idle`, `launch.startup_notify`,
/// a declared `launch.bounces` on macOS) gets for each reason its watch gave no time.
pub fn feedback_refusal(why: host::NoFeedback) -> crate::model::ValidatorId {
    use crate::model::ValidatorId;
    use host::NoFeedback::*;
    match why {
        NotOnThisOs => ValidatorId::NotOnThisOs,
        NoDockIcon | BareExecutable | NoBundleInfo | NotMainExecutable | NoMessageQueue | OutputMilestones => ValidatorId::NotForTarget,
        Wayland | NoDisplay => ValidatorId::Headless,
        Deadline | Ended | Unreadable => ValidatorId::RunIncomplete,
    }
}

/// Whether the app's launch has a Dock launch to count bounces of (macOS): its executable is an app
/// bundle's (`host::macos::bundle_of`), and the bundle has a Dock icon. `NotOnThisOs` elsewhere, which
/// is no refusal of `launch.bounces` there: a case may declare it in a borrowed unit (`bounces`).
pub fn dock_launch(app: &App) -> std::result::Result<(), host::NoFeedback> {
    #[cfg(target_os = "macos")]
    {
        use host::macos::{DockIcon, NotBundle};
        match host::macos::bundle_of(&app.exe) {
            // Without a bundle id strobe cannot tell a Launch Services record it made (`launch_path`).
            Ok(b) if b.id.is_none() => Err(host::NoFeedback::NoBundleInfo),
            Ok(b) if b.dock == DockIcon::Shown => Ok(()),
            Ok(_) => Err(host::NoFeedback::NoDockIcon),
            Err(NotBundle::Bare) => Err(host::NoFeedback::BareExecutable),
            Err(NotBundle::NoInfo) => Err(host::NoFeedback::NoBundleInfo),
            Err(NotBundle::NotMain) => Err(host::NoFeedback::NotMainExecutable),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(host::NoFeedback::NotOnThisOs)
    }
}

/// An app's launch announced for startup notification (Linux X11, `host::x11::announce_startup`): add
/// `env` to the launch's environment (`spawn`'s `env`), and read `feedback` for the app's `remove:`.
pub struct StartupNotice {
    pub env: (String, String),
    pub feedback: host::Feedback,
}

/// Announces `app`'s launch for startup notification before `spawn` (Linux on an X11 display), silently;
/// the watch ends at `deadline`. `Wayland` when the launch runs on strobe's compositor, `NoDisplay`
/// without an X11 display, `NotOnThisOs` off Linux.
pub fn startup_notice(app: &App, deadline: std::time::Instant) -> std::result::Result<StartupNotice, host::NoFeedback> {
    #[cfg(target_os = "linux")]
    {
        if app.wayland() {
            return Err(host::NoFeedback::Wayland);
        }
        let n = host::x11::announce_startup(deadline)?;
        Ok(StartupNotice { env: n.env(), feedback: n.feedback })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, deadline);
        Err(host::NoFeedback::NotOnThisOs)
    }
}

/// `NotForTarget` for reduced motion in a native app whose profile declares none: strobe knows no
/// setting of a native app's own, and never changes the OS's (rule 10).
pub fn no_motion_contract(id: &str) -> crate::model::Refusal {
    crate::model::Refusal::new(
        crate::model::ValidatorId::NotForTarget,
        format!("reduced motion in native app {id}: a native app has no setting strobe knows to ask for it, and its app profile declares none (apps.{id}.motion.reduce)"),
    )
    .fix(format!(
        "declare in strobe.json what makes the app prefer reduced motion: \"apps\": {{ \"{id}\": {{ \"motion\": {{ \"reduce\": {{ \"args\": [...], \"env\": {{...}}, \"files\": {{...}} }} }} }} }} (a native(path) target gets a profile and native(\"{id}\")), or drop the target's motion"
    ))
}

/// A relative executable from `dir`: one that names a folder (`./app`, `build/app`), or a bare name
/// of a file in `dir`; a bare name of none there is the one on PATH (`on_path`). A launch runs in its
/// home, so a relative path kept as it is would name nothing there.
pub fn from_dir(exe: &Path, dir: &Path) -> PathBuf {
    use std::path::Component;
    if !exe.is_relative() || exe.as_os_str().is_empty() {
        return exe.to_path_buf();
    }
    // `.` and `..` resolved by name, so the path reads as the file it is.
    let mut joined = PathBuf::new();
    for c in dir.join(exe).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if matches!(joined.components().next_back(), Some(Component::Normal(_))) => {
                joined.pop();
            }
            c => joined.push(c),
        }
    }
    // An app bundle's folder (`MyApp.app`) is the app itself, as its executable is (macOS).
    if exe.components().count() > 1 || joined.is_file() || is_bundle_dir(&joined) { joined } else { on_path(exe) }
}

/// macOS: an app bundle's folder that `host::macos::bundle_of` reads; false elsewhere.
fn is_bundle_dir(p: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        p.is_dir() && host::macos::bundle_of(p).is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = p;
        false
    }
}

/// The app bundle folder an executable is in (`Foo.app`, any case of the extension, as Launch
/// Services and `host::macos::bundle_of` take it): the innermost one.
pub fn app_bundle(exe: &Path) -> Option<&Path> {
    exe.ancestors().find(|a| a.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
}

/// A bare executable name as the file on PATH it names (Windows: `.exe` added to a name without an
/// extension), so what is launched is a file strobe can read; unchanged when PATH has none.
fn on_path(exe: &Path) -> PathBuf {
    let found = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|d| {
            let p = d.join(exe);
            if p.is_file() {
                return Some(p);
            }
            Some(p.with_extension("exe")).filter(|p| cfg!(windows) && exe.extension().is_none() && p.is_file())
        })
    });
    found.unwrap_or_else(|| exe.to_path_buf())
}

/// `from_dir` from the current folder (unchanged when it cannot be read).
fn from_cwd(exe: &Path) -> PathBuf {
    std::env::current_dir().map_or_else(|_| exe.to_path_buf(), |d| from_dir(exe, &d))
}

/// macOS: an app bundle's folder (`Foo.app`) as the executable it launches (`host::macos::bundle_of`),
/// so a running copy is found by its executable and a direct spawn runs it; anything else unchanged.
fn bundle_exe(exe: PathBuf) -> PathBuf {
    #[cfg(target_os = "macos")]
    if exe.is_dir()
        && let Ok(b) = host::macos::bundle_of(&exe)
    {
        return b.executable;
    }
    exe
}

/// This OS's key in a per-OS value (`exe`, a build's `output`).
pub fn os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// A path for every OS, or one per OS (`{ "windows": …, "macos": …, "linux": … }`): this OS's.
pub fn per_os(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s),
        Value::Object(m) => m.get(os()).and_then(Value::as_str),
        _ => None,
    }
}

/// strobe.json's `build` for this OS: its `windows`, `macos` or `linux` object overlays the rest
/// (`env` merged key by key, other keys replaced), so a repository pins what its build needs on
/// one OS (a toolchain, a compiler) without every user setting it.
pub fn build_for_os(build: &Value) -> Value {
    let Some(base) = build.as_object() else { return build.clone() };
    let mut out = base.clone();
    for k in ["windows", "macos", "linux"] {
        out.remove(k);
    }
    if let Some(overlay) = base.get(os()).and_then(Value::as_object) {
        for (k, v) in overlay {
            match (k.as_str(), out.get_mut(k)) {
                ("env", Some(Value::Object(env))) if v.is_object() => env.extend(v.as_object().cloned().unwrap_or_default()),
                _ => {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }
    Value::Object(out)
}

/// The app profile `id` from `config["apps"]`, following `extends`.
pub fn app(id: &str, config: &Value) -> Result<App> {
    crate::config::check_app(config, id)?;
    let apps = config["apps"].as_object().cloned().unwrap_or_default();
    let known = || apps.keys().cloned().collect::<Vec<_>>().join(", ");
    let mut chain = vec![apps.get(id).cloned().ok_or_else(|| Error::usage(format!("no app profile \"{id}\" in strobe.json apps (known: {})", if apps.is_empty() { "none".into() } else { known() })))?];
    while let Some(parent) = chain.last().and_then(|p| p["extends"].as_str()).map(str::to_string) {
        if chain.len() > 8 {
            return Err(Error::usage(format!("app profile \"{id}\": extends loops")));
        }
        chain.push(apps.get(&parent).cloned().ok_or_else(|| Error::usage(format!("app profile \"{id}\" extends unknown \"{parent}\"")))?);
    }
    let mut merged = serde_json::Map::new();
    for p in chain.iter().rev() {
        for (k, v) in p.as_object().cloned().unwrap_or_default() {
            match (merged.get_mut(&k), v) {
                (Some(Value::Object(into)), Value::Object(from)) => into.extend(from),
                (_, v) => {
                    merged.insert(k, v);
                }
            }
        }
    }
    let p = Value::Object(merged);
    let exe = per_os(&p["exe"]).map(str::to_string).ok_or_else(|| Error::usage(format!("app profile \"{id}\" has no exe for {}", os())))?;
    let exe = expand(&exe).map_err(|e| Error::usage(format!("app profile \"{id}\" exe: {}", e.message)))?;
    // A project's profile (the one of the chain that gave the exe): a relative executable is its own build,
    // from the project's root. A user's profile has no root: a bare name is the one on PATH.
    let root = chain.iter().find(|c| per_os(&c["exe"]).is_some()).and_then(|c| c[crate::config::ROOT].as_str());
    let exe = bundle_exe(match root {
        Some(root) => from_dir(&exe, Path::new(root)),
        None if exe.is_relative() && exe.components().count() == 1 => on_path(&exe),
        None => exe,
    });
    let strings = |k: &str| p[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let mut guards = Vec::new();
    for g in p["guard"].as_array().into_iter().flatten() {
        let Some(file) = g["file"].as_str() else { continue };
        // A guard for another OS's state (`"os": "windows"`) guards nothing here: one profile serves
        // every OS.
        if g["os"].as_str().is_some_and(|o| o != os()) {
            continue;
        }
        let file = expand(file).map_err(|e| Error::usage(format!("app profile \"{id}\" guard: {}", e.message)))?;
        guards.push(GuardSpec { file, pid: g["pid"].as_str().map(str::to_string), url: g["url"].as_str().map(str::to_string) });
    }
    let moved: Vec<String> = strings("moved");
    let moved = moved.iter().map(|m| expand(m).map_err(|e| Error::usage(format!("app profile \"{id}\" moved: {}", e.message)))).collect::<Result<Vec<_>>>()?;
    let kind = p["kind"].as_str().unwrap_or("electron");
    let kind = AppKind::parse(kind).ok_or_else(|| Error::usage(format!("app profile \"{id}\": kind is {}, not {kind}", AppKind::NAMES.join(", "))))?;
    let display = p["display"].as_str().map(Display::parse).transpose().map_err(|e| Error::usage(format!("app profile \"{id}\": {}", e.message)))?;
    if display.is_some() && kind != AppKind::Native {
        return Err(Error::usage(format!("app profile \"{id}\": display is for native apps (kind \"native\"); this one is {kind}")));
    }
    // Every place the profile writes in a home stays inside it (SPEC rule 10), said by `strobe config` too.
    let keys = |v: &Value| v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    for (what, rels) in [("files", keys(&p["files"])), ("motion.reduce.files", keys(&p["motion"]["reduce"]["files"])), ("dirs", strings("dirs"))] {
        for rel in rels {
            check_home_rel(&rel, &format!("app profile \"{id}\": {what}"))?;
        }
    }
    let reduce = &p["motion"]["reduce"];
    let motion_contract = reduce.is_object().then(|| MotionContract {
        args: reduce["args"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        env: reduce["env"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default(),
        files: reduce["files"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default(),
    });
    Ok(App {
        id: id.to_string(),
        exe,
        kind,
        env: p["env"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default(),
        unset: strings("unset"),
        dirs: strings("dirs"),
        files: p["files"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default(),
        guards,
        protocols: strings("protocols"),
        args: strings("args"),
        moved,
        windowless: p["window"] == false,
        gpu_cache_mode: p["gpuCache"].as_str().map(str::to_string),
        gpu_cache: None,
        folder_notes: None,
        motion_contract,
        reduce_motion: false,
        merged_files: Vec::new(),
        display,
        launch: LaunchPath::Spawn,
        feedback: None,
    })
}

/// Fails unless the app's executable exists (a profile names it before any build does).
pub fn installed(app: &App) -> Result<()> {
    if !app.exe.is_file() {
        return Err(Error::usage(format!("{} is not installed ({} does not exist)", app.id, app.exe.display())));
    }
    Ok(())
}

/// Whether environment variable `key` is `name`: in any case on Windows, which reads names that way
/// (`%APPDATA%` is `AppData`), exactly elsewhere.
pub fn env_name_is(key: &str, name: &str) -> bool {
    if cfg!(windows) { key.eq_ignore_ascii_case(name) } else { key == name }
}

/// Whether `key` matches an `unset` pattern (`NAME`, or `PREFIX*`), cased as `env_name_is`.
pub fn env_unset(patterns: &[String], key: &str) -> bool {
    patterns.iter().any(|u| match u.strip_suffix('*') {
        Some(prefix) => key.len() >= prefix.len() && key.is_char_boundary(prefix.len()) && env_name_is(&key[..prefix.len()], prefix),
        None => env_name_is(key, u),
    })
}

/// `~`, `%VAR%` and `${VAR}` from strobe's own environment: the real user's, never a launched copy's.
/// A variable that is not set is an error: a path left holding `%VAR%` names nothing, and a guard on
/// it would protect nothing while passing every check.
fn expand(s: &str) -> Result<PathBuf> {
    let home = match s.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", host::home().display()),
        None => s.to_string(),
    };
    let mut out = String::with_capacity(home.len());
    let mut rest = home.as_str();
    loop {
        let (at, open, close) = match (rest.find('%'), rest.find("${")) {
            (Some(p), Some(b)) if b < p => (b, 2, '}'),
            (Some(p), _) => (p, 1, '%'),
            (None, Some(b)) => (b, 2, '}'),
            (None, None) => break,
        };
        out.push_str(&rest[..at]);
        let after = &rest[at + open..];
        let name = after.find(close).map(|end| &after[..end]).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || "_()".contains(c)));
        let Some(name) = name else {
            out.push_str(&rest[at..at + open]);
            rest = after;
            continue;
        };
        let value = std::env::vars_os().find(|(k, _)| k.to_str().is_some_and(|k| env_name_is(k, name))).map(|(_, v)| v.to_string_lossy().to_string());
        let Some(value) = value else {
            return Err(Error::usage(format!("{s}: environment variable {name} is not set, so the path names nothing")));
        };
        out.push_str(&value);
        rest = &after[name.len() + 1..];
    }
    out.push_str(rest);
    Ok(PathBuf::from(out))
}

/// What the profile's guarded files held before launch: the user's own state, which must not change.
#[derive(Clone, Debug, Default)]
pub struct Guard {
    checks: Vec<Check>,
    /// Seed files the home shares with other launches: none may change.
    pub shared: Vec<Shared>,
    /// The user's own Documents and Desktop (`host::user_folders`) and when each last changed, read
    /// when the launch began.
    folders: Vec<(PathBuf, Option<SystemTime>)>,
    taken: Option<SystemTime>,
    /// Where a change there is noted instead of stopping the experiment (`App::folder_notes`).
    notes: Option<Notes>,
}

/// Notes a guard leaves for its experiment (`App::folder_notes`).
pub type Notes = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

use std::time::SystemTime;

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// What the OS or a sync client writes into a folder by itself: never the copy's doing.
fn incidental(name: &str) -> bool {
    matches!(name, ".DS_Store" | ".localized" | "desktop.ini" | "Icon\r") || (name.starts_with('.') && name.ends_with(".icloud"))
}

/// What appeared in `dir` since `since` (by each entry's own time), but what the OS writes by itself.
/// None when strobe may not list it (macOS asks a person first), so it is listed only once its time
/// moved.
fn appeared(dir: &Path, since: SystemTime) -> Option<Vec<String>> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| !incidental(&e.file_name().to_string_lossy()))
        .filter(|e| e.metadata().ok().and_then(|m| m.created().ok().max(m.modified().ok())).is_some_and(|t| t >= since))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    out.sort();
    Some(out)
}

#[derive(Clone, Debug)]
struct Check {
    file: PathBuf,
    before: Option<String>,
    pid: Option<u32>,
    addr: Option<SocketAddr>,
}

impl Guard {
    pub fn take(app: &App) -> Guard {
        let checks = app
            .guards
            .iter()
            .map(|g| {
                let before = match std::fs::read_to_string(&g.file) {
                    Ok(t) => Some(t),
                    // Absent is a state a guard compares; a file strobe may not read is no state at all.
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Some(UNREADABLE.to_string()),
                    Err(_) => None,
                };
                let v: Value = before.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
                let pid = g.pid.as_deref().and_then(|k| v[k].as_u64()).map(|p| p as u32);
                let addr = g.url.as_deref().and_then(|k| v[k].as_str()).and_then(|u| u.split("://").nth(1)).and_then(|h| h.split('/').next()).and_then(|h| h.parse().ok());
                Check { file: g.file.clone(), before, pid, addr }
            })
            .collect();
        let folders = host::user_folders()
            .into_iter()
            .map(|p| {
                let when = modified(&p);
                (p, when)
            })
            .collect();
        Guard { checks, shared: Vec::new(), folders, taken: Some(SystemTime::now()), notes: app.folder_notes.clone() }
    }

    /// What appeared in the user's own Documents or Desktop since the launch began, by folder: none
    /// when a folder's time did not move or only what the OS writes by itself appeared.
    fn folder_changes(&self) -> Vec<(PathBuf, Option<Vec<String>>)> {
        let since = self.taken.unwrap_or(SystemTime::UNIX_EPOCH);
        self.folders.iter().filter(|(p, before)| modified(p) != *before).map(|(p, _)| (p.clone(), appeared(p, since))).filter(|(_, new)| new.as_ref().is_none_or(|n| !n.is_empty())).collect()
    }

    /// Fails when a guarded file changed, or its process stopped running or answering, or the app
    /// wrote to a seed file its home shares with other launches, or something appeared in the user's
    /// own Documents or Desktop (noted instead, for the experiment, past each side's first launch).
    pub fn check(&self, when: &str) -> Result<()> {
        for (p, new) in self.folder_changes() {
            let what = match &new {
                Some(n) if n.len() > 5 => format!("{} and {} more appeared in your {}", n[..5].join(", "), n.len() - 5, p.display()),
                Some(n) => format!("{} appeared in your {}", n.join(", "), p.display()),
                None => format!("your {} changed (strobe may not list it)", p.display()),
            };
            match &self.notes {
                Some(notes) => {
                    let mut n = notes.lock().unwrap_or_else(|e| e.into_inner());
                    if !n.contains(&what) {
                        n.push(what);
                    }
                }
                None => {
                    return Err(Error::msg(format!("stopped {when}: {what} while the copy ran; strobe never measures past disturbed user state")).hint(
                        "an isolation gap when the copy wrote it: add the env var, switch or file that keeps that folder in its home to its profile in strobe.json apps; if it was you or another program, run again",
                    ));
                }
            }
        }
        if let Some(s) = self.shared.iter().find(|s| stamp(&s.cache) != Some((s.len, s.modified))) {
            return Err(Error::msg(format!(
                "stopped {when}: the app changed {}, a large seed file strobe shares read-only between launches (as {}), so later launches would not start from the same seed",
                s.link.display(),
                s.cache.display()
            ))
            .hint("the next launch makes a fresh shared copy; an app that must write a seed file this large needs it below 64 MB or outside the seed"));
        }
        for c in &self.checks {
            let changed = std::fs::read_to_string(&c.file).ok() != c.before;
            let stopped = c.pid.is_some_and(|p| !alive(p));
            let silent = c.addr.is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(2)).is_err());
            if changed || stopped || silent {
                let what = if changed {
                    "changed"
                } else if stopped {
                    "lost its process"
                } else {
                    "stopped answering"
                };
                return Err(Error::msg(format!("stopped {when}: {} {what}; strobe never measures past disturbed user state", c.file.display()))
                    .hint("an isolation gap: the app reached your own state; add the env var or file it escaped through to its profile in strobe.json apps"));
            }
        }
        Ok(())
    }

    fn pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.checks.iter().filter_map(|c| c.pid)
    }

    /// A guarded file strobe may not read (macOS 27 denies reading another developer's app data
    /// without a prompt): it could change unseen, so nothing launches. A bench asks before it waits
    /// for the slot or builds.
    pub fn unreadable(&self) -> Result<()> {
        let Some(c) = self.checks.iter().find(|c| c.before.as_deref() == Some(UNREADABLE)) else { return Ok(()) };
        Err(Error::refused(
            "LiveChannel",
            format!("{} is guarded but strobe may not read it (permission denied), so a change to it would go unseen", c.file.display()),
            if cfg!(target_os = "macos") {
                "macOS denies reading another developer's app data (containers, group containers): guard a file outside them, or grant strobe Full Disk Access (System Settings › Privacy & Security › Full Disk Access)"
            } else {
                "guard a file strobe can read, or let this user read it"
            },
        ))
    }
}

/// What a guard holds for a file it may not read.
const UNREADABLE: &str = "\u{0}strobe: unreadable";

/// A launch that never showed what it waited for (`waited`: a debugging port, a window): on macOS, when
/// nothing launched for it ran past the kernel's exec (the root `pid` and every process of its set by
/// `home`, each with its reaped children: `held_cpu`), macOS held it before its first instruction, as Gatekeeper does with an app not yet opened since it
/// was downloaded or updated (the kernel logs 'Security policy would not allow process'): it waits in
/// the kernel's exec, before dyld runs. That needs a person: exit 3 with the step, never retried. Else
/// `None`: a slow launch, which a bench retries. Not held: a process gone or a zombie at the deadline,
/// one whose CPU cannot be read, a wrapper whose child ran, a tiny program that ran and then waits.
pub fn held_at_launch(pid: u32, home: Option<&Path>, exe: &Path, waited: &str) -> Option<Error> {
    let ns = held_cpu(pid, home)?;
    let bundle = app_bundle(exe);
    let step = match bundle {
        Some(app) => format!("open {} once from the Finder and allow it (if macOS still refuses, allow it in System Settings > Privacy & Security > Open Anyway), then run again", app.display()),
        None => format!("allow {} in System Settings > Privacy & Security > Open Anyway, then run again", exe.display()),
    };
    Some(
        Error::new(
            3,
            format!(
                "{waited}; it never got past starting ({} of CPU in all, almost no system calls), so macOS held it at launch (Gatekeeper asks a person after a download or an update)",
                crate::fmt::time(ns as f64 / 1e6)
            ),
        )
        .hint(step),
    )
}

/// `held_at_launch`'s bounds, measured on air (macOS 27, M4) 3–4 s after each start: a held Firefox 159
/// made 16–17 Mach system calls (55 Unix ones) with 0.04–0.16 ms of user-mode CPU (0.3–1.4 ms in all);
/// programs that ran and then waited (a C `pause()`, a Rust sleep, `/bin/sleep`, a shell running a
/// paused child) made 83–313 Mach calls with 0.38–2.5 ms of user-mode CPU. Both must hold.
#[cfg(target_os = "macos")]
const HELD_MACH_CALLS: u64 = 40;
#[cfg(target_os = "macos")]
const HELD_USER_NS: u64 = 300_000;

/// The CPU ns a launch used in all when nothing of it ran past the kernel's exec (`held_at_launch`):
/// every process of its set made few Mach calls (dyld and libSystem make dozens), and their user-mode
/// CPU with reaped children's (a wrapper's child that worked and exited counts) stays under
/// `HELD_USER_NS`. Else None.
#[cfg(target_os = "macos")]
fn held_cpu(pid: u32, home: Option<&Path>) -> Option<u64> {
    let pids = host::process_set_pids(pid, home);
    if pids.is_empty() {
        return None;
    }
    let (mut user, mut all) = (0u64, 0u64);
    for p in pids {
        // A process gone, a zombie, or one the kernel will not read: nothing says it was held.
        let r = host::macos::launch_ran(p)?;
        if r.mach_calls >= HELD_MACH_CALLS {
            return None;
        }
        user += r.user_ns;
        all += r.all_ns;
    }
    (user < HELD_USER_NS).then_some(all)
}

#[cfg(not(target_os = "macos"))]
fn held_cpu(_pid: u32, _home: Option<&Path>) -> Option<u64> {
    None
}

pub fn alive(pid: u32) -> bool {
    host::exe_of(pid).is_some()
}

/// Seed files this large are shared, not copied into each home: a copy per launch writes the whole
/// file again, an antivirus scans each new copy on every CPU while the bench measures, and deleting
/// it after costs again.
const SHARE_BYTES: u64 = 64 << 20;

/// A large seed file shared into a home: one read-only copy per seed version in strobe's own
/// `<state>/seeds`, hard-linked into each home, with its size and time when linked. The app reads it
/// like any file of its home; it cannot write through to the seed (read-only), and a write that got
/// through anyway shows in `Guard::check`.
#[derive(Clone, Debug)]
pub struct Shared {
    pub link: PathBuf,
    pub cache: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
    /// The copy's folder in use for as long as the home links it (`host::cache`): `strobe prune` leaves it.
    _lease: std::sync::Arc<host::cache::Lease>,
}

fn stamp(p: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()))
}

/// The read-only cached copy of a large seed file (made once per seed version), linked to `dst`.
/// None when it cannot be shared (another volume, no hard links): the caller copies it.
fn share(seed: &Path, dst: &Path) -> Option<Shared> {
    use sha2::Digest;
    let (len, modified) = stamp(seed)?;
    let id: String = sha2::Sha256::digest(format!("{}\n{len}\n{modified:?}", seed.display()).as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let cache = host::paths::sub("seeds").join(id).join(seed.file_name()?);
    // In use from before it is looked at or copied (a prune removing it now finishes first), until the
    // home goes, and with an experiment until it ends (`host::cache::keep`): its blocks find it warm.
    // Taking it marks when this seed version was last linked.
    // A copy that cannot be leased is not shared: the seed is copied into the home instead.
    let lease = std::sync::Arc::new(host::cache::take(cache.parent()?).ok()?);
    host::cache::keep(cache.parent()?, &lease);
    // A cached copy that changed since it was made (a write that got through) is made again.
    let fresh = stamp(&cache).is_some_and(|(l, _)| l == len) && std::fs::read_to_string(cache.with_extension("stamp")).ok() == Some(format!("{:?}", stamp(&cache)));
    if !fresh {
        if let Ok(m) = std::fs::metadata(&cache) {
            let mut p = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(&cache, p);
            let _ = std::fs::remove_file(&cache);
        }
        std::fs::create_dir_all(cache.parent()?).ok()?;
        std::fs::copy(seed, &cache).ok()?;
        let mut p = std::fs::metadata(&cache).ok()?.permissions();
        p.set_readonly(true);
        std::fs::set_permissions(&cache, p).ok()?;
        std::fs::write(cache.with_extension("stamp"), format!("{:?}", stamp(&cache))).ok()?;
    }
    std::fs::hard_link(&cache, dst).ok()?;
    let (len, modified) = stamp(&cache)?;
    Some(Shared { link: dst.to_path_buf(), cache, len, modified, _lease: lease })
}

fn copy_dir(from: &Path, to: &Path, shared: &mut Vec<Shared>) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        let dst = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &dst, shared)?;
        } else if let Some(s) = std::fs::metadata(&p).ok().filter(|m| m.len() >= SHARE_BYTES).and_then(|_| share(&p, &dst)) {
            shared.push(s);
        } else {
            std::fs::copy(&p, &dst)?;
        }
    }
    Ok(())
}

/// Unlinks a home's shared seed files: a read-only file's link may need its attribute cleared first,
/// which clears it on the cached copy too, so it is set again right after.
fn unshare(home: &Path) {
    let Ok(text) = std::fs::read_to_string(home.join(".strobe-shared")) else { return };
    for line in text.lines() {
        let Some((link, cache)) = line.split_once('\t') else { continue };
        let (link, cache) = (Path::new(link), Path::new(cache));
        // The app in this home can write the file: a line counts only for a link inside the home to a
        // copy inside strobe's seed folder, both one file (rule 10: nothing outside them is touched).
        if shared_pair(home, link, cache) {
            unlink_shared(link, cache);
        }
    }
}

/// Whether `link` (in `home`) and `cache` (in `<state>/seeds`) are names of one regular file, neither a
/// link, each inside its folder with the folders above it resolved.
fn shared_pair(home: &Path, link: &Path, cache: &Path) -> bool {
    let seeds = host::paths::state_path().join("seeds");
    if !(host::disk::inside(link, home) && host::disk::inside(cache, &seeds)) {
        return false;
    }
    match (host::disk::stat(link), host::disk::stat(cache)) {
        (Ok(l), Ok(c)) => l.kind == host::disk::Kind::File && c.kind == host::disk::Kind::File && (l.dev, l.id) == (c.dev, c.id),
        _ => false,
    }
}

/// Removes one link to a shared seed file, never the cached copy's content or its read-only mark. The
/// read-only mark is changed only on a regular file, never through a link.
fn unlink_shared(link: &Path, cache: &Path) {
    if std::fs::remove_file(link).is_ok() || std::fs::symlink_metadata(link).is_err() {
        return;
    }
    let set = |ro: bool| {
        if let Some(m) = std::fs::symlink_metadata(cache).ok().filter(|m| m.file_type().is_file()) {
            let mut p = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(ro);
            let _ = std::fs::set_permissions(cache, p);
        }
    };
    set(false);
    let _ = std::fs::remove_file(link);
    set(true);
}

/// Fails unless `rel` names a place inside a home (SPEC rule 10): a non-empty relative path of plain
/// names, with no root, drive, `..` or `.`, and no `~`, `%VAR%` or `${VAR}` left unexpanded (a home's
/// paths are never expanded). `what` says where it came from (`app profile "x": files`).
pub fn check_home_rel(rel: &str, what: &str) -> Result<()> {
    let var = |s: &str| {
        let mut parts = s.split('%');
        parts.next();
        let mut inside = parts.collect::<Vec<_>>();
        inside.pop();
        inside.iter().any(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || "_()".contains(c)))
    };
    let plain = !rel.is_empty() && Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if plain && !rel.starts_with('~') && !rel.contains("${") && !var(rel) {
        return Ok(());
    }
    Err(Error::usage(format!("{what} \"{rel}\" is not a path inside the launch's private home"))
        .hint("name it relative to the home with plain folder names (\"config/settings.json\"): no leading / or drive, no .., no ~ or environment variables; strobe writes only inside the home"))
}

/// `rel` (`check_home_rel`) inside `home`, ready to write: no folder on the way that is a link out of
/// the home (an app may have left one in a warm home), the file itself no link, and a shared seed file
/// at that place unlinked first (after `read`, which gets what it held) so a write never reaches the
/// shared copy.
fn home_path(home: &Path, rel: &str, what: &str, shared: &[Shared], read: bool) -> Result<(PathBuf, Option<String>)> {
    check_home_rel(rel, what)?;
    let path = home.join(rel);
    let mut at = home.to_path_buf();
    for c in Path::new(rel).components() {
        at.push(c);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(Error::usage(format!("{what} \"{rel}\": {} is a link, which could lead out of the launch's private home", at.display())).hint("seed real folders and files, not links"));
        }
    }
    let held = if read { std::fs::read_to_string(&path).ok() } else { None };
    if let Some(s) = shared.iter().find(|s| s.link == path) {
        unlink_shared(&s.link, &s.cache);
    }
    Ok((path, held))
}

/// A new private home folder; nothing is in it yet.
pub fn new_home() -> PathBuf {
    host::paths::run().join(format!("home-{}-{}", std::process::id(), host::now_ns()))
}

/// The file in a home or profile that names its strobe the way `host::spawn::OWNER_VAR` does.
const OWNER_FILE: &str = ".strobe-owner";

/// Homes and profiles this strobe launched into and has not released.
static HELD: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn held(dir: &str) -> bool {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|h| h == dir)
}

/// Makes a launch folder (a home or a profile) and names this strobe in its owner file at once, before
/// a seed is copied or a helper started in it: another strobe's sweep (`reap_dead_owners`) never takes
/// a folder being filled for a live strobe as a gone one's.
pub fn claim(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    own(dir)
}

/// Names this strobe in `dir`'s owner file, unless it already does: a warm home's file is never
/// truncated and rewritten under a sweep that may read it meanwhile.
fn own(dir: &Path) -> std::io::Result<()> {
    let file = dir.join(OWNER_FILE);
    if std::fs::read_to_string(&file).is_ok_and(|s| s == owner_stamp()) {
        return Ok(());
    }
    std::fs::write(file, owner_stamp())
}

/// The variables a launch for `dir` carries (`host::spawn::HOME_VAR`, `OWNER_VAR`); `dir` is held
/// until it is released, and names this strobe in its owner file.
pub fn launch_env(dir: &Path) -> [(String, String); 2] {
    let _ = own(dir);
    let k = key(dir);
    let mut h = HELD.lock().unwrap_or_else(|e| e.into_inner());
    if !h.contains(&k) {
        h.push(k);
    }
    [(HOME_VAR.into(), dir.display().to_string()), (OWNER_VAR.into(), owner_stamp().into())]
}

fn let_go(dir: &Path) {
    let k = key(dir);
    HELD.lock().unwrap_or_else(|e| e.into_inner()).retain(|h| *h != k);
}

/// Whether the strobe a stamp names runs: that pid, started at that time.
fn owner_alive(pid: u32, start: u64) -> bool {
    host::start_time(pid) == Some(start)
}

/// A stamped process is a leftover when its strobe is gone, or when it is this strobe's and the
/// folder it was launched for was released (a detached child that outlived its launch).
fn leftover(s: &Stamp) -> bool {
    if s.pid == std::process::id() { !held(&s.home) } else { !owner_alive(s.pid, s.start) }
}

/// Ends the trees of `procs` from their roots (a process whose parent is not among them), each only
/// while it is the process the scan found: its start time, its stamp, then its start time again, read
/// right before it ends (`host::kill_opened`, which reads each process's start time once more right
/// before its signal). Never a process group: a stamped process is no child of this strobe's (its
/// children are ended by their `Owned`), so its group is not strobe's to signal. Returns the ended
/// roots, named before any ended.
pub fn end_roots(procs: &[Stamped]) -> Vec<(u32, String, Stamp)> {
    let roots: Vec<(&Stamped, String)> =
        procs.iter().filter(|p| !procs.iter().any(|q| q.pid == p.parent)).map(|p| (p, host::exe_of(p.pid).map(|e| e.display().to_string()).unwrap_or_else(|| format!("process {}", p.pid)))).collect();
    roots
        .into_iter()
        .filter(|(p, _)| {
            let same = host::start_time(p.pid) == Some(p.start) && stamp_of(p.pid).as_ref() == Some(&p.stamp) && host::start_time(p.pid) == Some(p.start);
            if same {
                host::kill_opened(p.pid, p.start);
            }
            same
        })
        .map(|(p, exe)| (p.pid, exe, p.stamp.clone()))
        .collect()
}

/// The processes stamped with `dir` now.
pub fn stamped_home(dir: &Path) -> Vec<Stamped> {
    let k = key(dir);
    stamped().into_iter().filter(|s| s.stamp.home == k).collect()
}

/// Ends what dead strobes left behind, and the folders they left: every stamped process whose
/// strobe is gone (a crash, a kill, a signal before it closed its target) or that outlived a folder
/// this strobe released, then every home or profile under `run/` whose strobe is gone (its GPU
/// caches go back, shared seeds unlink, URL schemes are restored). Returns a line per process ended.
pub fn reap_dead_owners() -> Vec<String> {
    let dead: Vec<Stamped> = stamped().into_iter().filter(|s| leftover(&s.stamp)).collect();
    let mut lines: Vec<String> = end_roots(&dead)
        .into_iter()
        .map(|(pid, exe, s)| {
            if s.pid == std::process::id() {
                format!("ended {exe} (pid {pid}): this strobe launched it for a folder it already released")
            } else {
                format!("ended {exe} (pid {pid}): the strobe that launched it (pid {}) is gone and left it running", s.pid)
            }
        })
        .collect();
    for dir in launch_folders() {
        if folder_gone(&dir) == Some(true) {
            lines.extend(release_gone(&dir));
            remove_home(&dir);
        }
    }
    lines
}

/// The launch folders strobe makes: homes and browser profiles under `run/`, and Snap Firefox's
/// profiles where its sandbox lets it read them (`firefox::snap_profiles`).
pub fn launch_folders() -> Vec<PathBuf> {
    [host::paths::run(), super::firefox::snap_profiles()]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|r| r.flatten())
        // Folders only, never a link to one: a sweep never follows a link out of strobe's own folders.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| launch_pid(&n.to_string_lossy()).is_some()))
        .collect()
}

/// The pid in a launch folder's name: a launch's home or browser profile, or the folder of a Wayland
/// compositor strobe ran (`wayland`).
fn launch_pid(name: &str) -> Option<u32> {
    ["profile-", "home-", "wl-"].iter().find_map(|p| name.strip_prefix(p)).and_then(|r| r.split('-').next()).and_then(|p| p.parse::<u32>().ok())
}

/// Whether the strobe that made a launch folder is gone (its owner file's pid and start time, or for a
/// folder from before owner files, the pid in its name): None for a folder of another kind.
pub fn folder_gone(dir: &Path) -> Option<bool> {
    let named = launch_pid(&dir.file_name()?.to_string_lossy())?;
    Some(match std::fs::read_to_string(dir.join(OWNER_FILE)).ok().as_deref().and_then(parse_owner) {
        Some((pid, start)) => pid != std::process::id() && !owner_alive(pid, start),
        // No owner file: a folder from before owner files, or one made a moment ago (`claim` writes the
        // file right after the folder: one younger than a minute is left for its strobe).
        None => {
            // A time a little ahead of the clock (a coarse file system, a clock step back) is young too.
            let young = std::fs::metadata(dir).and_then(|m| m.modified()).ok().is_some_and(|t| match t.elapsed() {
                Ok(a) => a < Duration::from_secs(60),
                Err(e) => e.duration() < Duration::from_secs(60),
            });
            !young && named != std::process::id() && !host::exe_of(named).is_some_and(|exe| exe.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase().starts_with("strobe")))
        }
    })
}

/// Before a gone strobe's launch folder is removed: ends what still runs from it, restores the URL
/// schemes saved in it and unlinks its shared seed files. A line per process ended.
pub fn release_gone(dir: &Path) -> Vec<String> {
    let lines = end_roots(&stamped_home(dir)).into_iter().map(|(pid, exe, _)| format!("ended {exe} (pid {pid}): it ran from {}, a folder of a strobe that is gone", dir.display())).collect();
    restore_protocols(dir);
    unshare(dir);
    lines
}

/// What a launch folder borrows from strobe's caches while it exists: the shared seed copies linked
/// into it and the GPU cache store lent to it.
pub fn borrowed(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_to_string(dir.join(".strobe-shared")).unwrap_or_default().lines().filter_map(|l| l.split_once('\t')).map(|(_, cache)| PathBuf::from(cache)).collect();
    if let Ok(store) = std::fs::read_to_string(dir.join(".strobe-gpu-cache")) {
        out.push(PathBuf::from(store.trim()));
    }
    out
}

/// Ends what still runs from `home` (an app's detached service, anything stamped with it) and
/// restores the URL schemes saved in it, without deleting it: a warm home is launched again.
pub fn release_home(home: &Path) {
    end_roots(&stamped_home(home));
    restore_protocols(home);
    // Again: what Launch Services started late (a request still in flight at a stop) surfaces while
    // the record is removed.
    end_roots(&stamped_home(home));
    let_go(home);
}

/// While alive, a stop of strobe (Ctrl-C, SIGINT, SIGTERM) waits for a Launch Services launch under
/// way to end what it started (macOS, `macos::opening`, 2.5 s at most), ends every launch it owns (each one's tree
/// and group, its Linux cgroup or Windows job: `spawn::end_launches`) and what carries a launch's stamp
/// (a browser's crash reporter on a session of its own), waits up to 2 s for those to go and says what
/// did not (`spawn::leave`), then what still runs from `home` (stamped with it),
/// removes that home when `prepare_home` made it now, and exits 130: for a command with no stop
/// points of its own (`look`), which leaves nothing running for the next strobe to reap. A browser's
/// profile folder goes with the next strobe's `reap_dead_owners`. Dropped (before the command closes what
/// it launched), the watcher has ended: a stop that came first finished its work and exited.
pub struct EndOnStop {
    done: std::sync::Arc<std::sync::Mutex<bool>>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

pub fn end_on_stop(home: Option<&Home>) -> EndOnStop {
    use std::sync::atomic::Ordering;
    let done = std::sync::Arc::new(std::sync::Mutex::new(false));
    // The folder of the launch's Wayland compositor, ended and removed with it.
    let display = home.and_then(Home::display_dir);
    let (flag, state, home) = (host::ctrl_c(), done.clone(), home.map(|h| (h.dir.clone(), h.fresh)));
    let watcher = std::thread::spawn(move || {
        loop {
            {
                let done = state.lock().unwrap_or_else(|e| e.into_inner());
                if *done {
                    return;
                }
                // Under the lock: the command cannot start closing meanwhile.
                if flag.load(Ordering::SeqCst) {
                    // Each launch's tree and group, its cgroup or job, then what left them with its
                    // stamp (a browser's crash reporter, on its own session), waited for: what is still
                    // there 2 s on is said below, never left unsaid for speed.
                    exit_on_stop(home.as_ref(), display.as_deref());
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    EndOnStop { done, watcher: Some(watcher) }
}

impl Drop for EndOnStop {
    fn drop(&mut self) {
        *self.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
        if let Some(w) = self.watcher.take() {
            let _ = w.join();
        }
    }
}

/// A watcher and an escalated stop may coincide. Only exit after the one doing cleanup has reported.
pub(crate) fn exit_on_stop(home: Option<&(PathBuf, bool)>, display: Option<&Path>) -> ! {
    static EXIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _exit = EXIT.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(target_os = "linux")]
    super::a11y::atspi::restore_on_stop();
    host::spawn::stop_launches();
    host::reserve::restore_on_stop();
    host::power::restore_on_stop();
    if let Some((dir, fresh)) = home {
        release_home(dir);
        if *fresh {
            remove_home(dir);
        }
    }
    if let Some(dir) = display {
        release_home(dir);
        remove_home(dir);
    }
    // Also cover a launch already unlisted but whose home has not finished closing.
    for dir in launch_folders() {
        if held(&key(&dir)) && std::fs::read_to_string(dir.join(OWNER_FILE)).is_ok_and(|s| s == owner_stamp()) {
            restore_protocols(&dir);
        }
    }
    let left = host::spawn::left_alone();
    if !left.is_empty() {
        eprint!("{}", host::spawn::left_alone_text(&left));
    }
    std::process::exit(130);
}

/// Ends what a launch started, whatever state it is in: the child, its tree and its group
/// (`Owned::end`), then everything stamped with its folder; releases the folder and, with `remove`,
/// deletes it.
pub fn end_launch(child: Option<Owned>, home: Option<&Path>, remove: bool) {
    drop(child);
    if let Some(h) = home {
        release_home(h);
        if remove {
            remove_home(h);
        }
    }
}

/// Where GPU drivers keep compiled shaders, relative to a home whose per-user folders the launch
/// redirects: Windows resolves `%LOCALAPPDATA%` (and LocalLow, from `%USERPROFILE%`) under it, Mesa
/// and NVIDIA's Linux driver use `$XDG_CACHE_HOME` (older NVIDIA drivers `~/.nv`). macOS keeps Metal's
/// cache per app in the per-user cache folder the OS assigns (`getconf DARWIN_USER_CACHE_DIR`), which
/// no environment variable moves: every launch there uses the user's own, warm.
const DRIVER_CACHES: &[&str] = if cfg!(windows) {
    &[
        "AppData/Local/NVIDIA/DXCache",
        "AppData/Local/NVIDIA/GLCache",
        "AppData/Local/D3DSCache",
        "AppData/Local/AMD/DxCache",
        "AppData/Local/AMD/DxcCache",
        "AppData/Local/AMD/VkCache",
        "AppData/Local/AMD/OglCache",
        "AppData/LocalLow/Intel/ShaderCache",
    ]
} else if cfg!(target_os = "linux") {
    &[".cache/mesa_shader_cache", ".cache/mesa_shader_cache_db", ".cache/nvidia", ".cache/radv_builtin_shaders", ".nv"]
} else {
    &[]
};

/// Folders Chromium (and so an Electron app) keeps its own GPU caches in, wherever its profile is:
/// program binaries, Skia's and Dawn's compiled shaders.
const CHROMIUM_GPU_CACHES: &[&str] = &["GPUCache", "GrShaderCache", "GraphiteDawnCache", "DawnGraphiteCache", "DawnWebGPUCache", "ShaderCache"];

/// Moves `from` to `to` (same volume: strobe's state folder), retrying while a just-closed app's
/// files are still held.
fn move_dir(from: &Path, to: &Path) -> bool {
    if let Some(p) = to.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    for _ in 0..20 {
        if std::fs::rename(from, to).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Lends the GPU shader caches kept in `store` to `home` before a launch: moved in (no copy), so the
/// app's driver and Chromium find what earlier launches of the same variant compiled, as on a
/// person's machine. Never the user's own caches, and never another variant's (each has its store).
/// What the home already holds (a seed's own) wins. True when the store held anything.
pub fn lend_gpu_caches(home: &Path, store: &Path) -> bool {
    let _ = std::fs::create_dir_all(store);
    let _ = std::fs::write(home.join(".strobe-gpu-cache"), store.display().to_string());
    let held = std::fs::read_to_string(store.join("paths")).unwrap_or_default();
    let mut warm = false;
    for rel in held.lines().filter(|l| !l.is_empty()) {
        let (from, to) = (store.join("dirs").join(rel), home.join(rel));
        if from.is_dir() && !to.exists() && move_dir(&from, &to) {
            warm = true;
        }
    }
    warm
}

/// Takes a home's GPU shader caches back into the store it borrowed them from (`lend_gpu_caches`),
/// once nothing runs from it: the driver folders, and every Chromium GPU cache folder in it.
pub fn keep_gpu_caches(home: &Path) {
    let Ok(store) = std::fs::read_to_string(home.join(".strobe-gpu-cache")) else { return };
    let store = PathBuf::from(store.trim());
    // The app in this home can write that file: only a store in strobe's own GPU cache folder is one.
    if !host::disk::inside(&store, &host::paths::state_path().join("gpu-cache")) || std::fs::symlink_metadata(&store).is_ok_and(|m| m.file_type().is_symlink()) {
        return;
    }
    // A real folder, never a link the app left (it could lead out of the home).
    let real_dir = |p: &Path| host::disk::inside(p, home) && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    let mut rels: Vec<String> = DRIVER_CACHES.iter().map(|r| r.to_string()).filter(|r| real_dir(&home.join(r))).collect();
    fn chromium(dir: &Path, home: &Path, depth: usize, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(home).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if !e.file_type().is_ok_and(|t| t.is_dir()) || DRIVER_CACHES.iter().any(|d| rel == *d) {
                continue;
            }
            if CHROMIUM_GPU_CACHES.iter().any(|n| e.file_name() == *n) {
                out.push(rel);
            } else if depth > 0 {
                chromium(&p, home, depth - 1, out);
            }
        }
    }
    chromium(home, home, 6, &mut rels);
    let mut kept: Vec<String> = std::fs::read_to_string(store.join("paths")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    for rel in rels {
        let to = store.join("dirs").join(&rel);
        if to.exists() {
            let _ = std::fs::remove_dir_all(&to);
        }
        if move_dir(&home.join(&rel), &to) && !kept.contains(&rel) {
            kept.push(rel);
        }
    }
    let _ = std::fs::write(store.join("paths"), kept.join("\n"));
}

/// The GPU cache store of one executable, which recordings of it share (`record`, `record --bench`):
/// a recording after the first finds what the earlier ones compiled.
pub fn exe_gpu_store(exe: &Path) -> PathBuf {
    use sha2::Digest;
    let key: String = sha2::Sha256::digest(exe.display().to_string().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    host::paths::sub("gpu-cache").join(format!("rec-{key}"))
}

/// Bytes a GPU cache store holds (its caches, while no launch has them lent).
pub fn gpu_cache_bytes(store: &Path) -> u64 {
    fn size(p: &Path) -> u64 {
        std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| if e.path().is_dir() { size(&e.path()) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }).sum()
    }
    size(&store.join("dirs"))
}

/// Removes a home once nothing holds its files any more.
pub fn remove_home(home: &Path) {
    let_go(home);
    keep_gpu_caches(home);
    unshare(home);
    for _ in 0..20 {
        if std::fs::remove_dir_all(home).is_ok() || !home.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Unix epoch time in ms, the clock pages report in (`performance.timeOrigin`).
pub fn epoch_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1e3).unwrap_or(0.0)
}

/// A line an app wrote: when strobe read it, as epoch ms (beside a log line's own time) and on the
/// host clock in ns (beside phase marks and presents), and its text.
pub type OutputLine = (f64, u64, String);

/// Lines of a launch's stdout and stderr with their arrival time and terminal colour codes removed;
/// the pipes are always drained, so a chatty app never blocks on a full pipe.
pub fn lines(child: &mut Owned) -> std::sync::mpsc::Receiver<OutputLine> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (out, err) = child.output();
    let readers: Vec<Box<dyn std::io::Read + Send>> =
        [out.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), err.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten().collect();
    for r in readers {
        let tx = tx.clone();
        std::thread::spawn(move || {
            host::spawn::each_line(r, |line| {
                let _ = tx.send((epoch_ms(), host::now_ns(), strip_ansi(&line)));
            });
        });
    }
    rx
}

/// `text` without ANSI escape sequences (`ESC [ … letter`, `ESC ] … BEL`).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A home ready for a launch: its folders, seed and profile files are in place, and `env` is what
/// the app's environment sets (on top of strobe's own, less the profile's `unset`).
#[derive(Clone, Debug)]
pub struct Home {
    pub dir: PathBuf,
    pub port: u16,
    pub env: Vec<(String, String)>,
    /// Large seed files shared into the home rather than copied.
    pub shared: Vec<Shared>,
    /// With `App::gpu_cache`: whether the launch found GPU shader caches from earlier launches.
    pub gpu_warm: Option<bool>,
    /// `prepare_home` made it now (not a warm home launched before): a launch that ends without
    /// being closed removes it, and keeps a warm one for the caller.
    pub fresh: bool,
    /// The Wayland compositor the launch runs on (`App::wayland`), ended with the last clone of the home.
    #[cfg(target_os = "linux")]
    pub display: Option<std::sync::Arc<super::wayland::Session>>,
    /// The GPU cache store lent to the home, in use until the last clone of the home goes (and with an
    /// experiment until it ends): `strobe prune` leaves it (`host::cache`).
    pub leases: Vec<std::sync::Arc<host::cache::Lease>>,
}

impl Home {
    /// The Wayland compositor the launch runs on, when it runs on one strobe started: what it is
    /// (`sway version 1.10.1, headless, pixman`) and whether it composites in software.
    pub fn headless(&self) -> Option<(String, bool)> {
        #[cfg(target_os = "linux")]
        {
            self.display.as_ref().map(|s| (s.describe(), s.software()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// The folder of the Wayland compositor strobe runs for the launch, if it runs on one.
    pub fn display_dir(&self) -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            self.display.as_ref().map(|d| d.dir.clone())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// What reached the screen of the launch's windows: on the Wayland compositor strobe runs for it,
    /// that compositor's changes (for waits; its frames are refused `Headless`), else the OS's.
    pub fn presents(&self) -> Result<crate::probe::present::PresentCapture> {
        #[cfg(target_os = "linux")]
        if let Some(c) = self.display.as_ref().and_then(|d| d.compositor()) {
            return crate::probe::present::PresentCapture::wayland(c);
        }
        crate::probe::present::PresentCapture::start()
    }

    /// Variables the launch removes beyond the profile's: another display, on strobe's compositor.
    fn unsets(&self, key: &str) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.display.is_some() && super::wayland::UNSET.contains(&key)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = key;
            false
        }
    }

    /// The app's whole environment, for a hook that starts something the app would see as its own
    /// (a background service).
    pub fn environment(&self, app: &App) -> std::collections::BTreeMap<String, String> {
        let mut out: std::collections::BTreeMap<String, String> = std::env::vars().filter(|(k, _)| !app.unsets(k) && !self.unsets(k)).collect();
        // One entry per name: the home's `APPDATA` replaces strobe's `AppData` on Windows.
        out.retain(|k, _| !self.env.iter().any(|(e, _)| env_name_is(k, e)));
        out.extend(self.env.iter().cloned());
        out
    }

    /// `{home}` and `{port}` filled into `s`.
    pub fn fill(&self, s: &str) -> String {
        s.replace("{home}", &self.dir.display().to_string()).replace("{port}", &self.port.to_string())
    }
}

/// Prepares `home` (a new one when `None`) for `app`. A home launched before (a warm profile) keeps
/// its state, and keeps its `{port}`: something started for it (a service) is still found there.
pub fn prepare_home(app: &App, seed: Option<&Path>, home: Option<PathBuf>) -> Result<Home> {
    let home = home.unwrap_or_else(new_home);
    let fresh = !home.join(".strobe-home").exists();
    claim(&home)?;
    let appdata = home.join("AppData").join("Roaming");
    let local = home.join("AppData").join("Local");
    let temp = local.join("Temp");
    let (data, state, config, cache) = (home.join(".local").join("share"), home.join(".local").join("state"), home.join(".config"), home.join(".cache"));
    for d in [&appdata, &temp, &data, &state, &config, &cache] {
        std::fs::create_dir_all(d)?;
    }
    // Windows resolves known folders under the redirected profile; Electron's app.getPath("documents")
    // throws when the folder is missing (an isolated desktop app failed to start on exactly that).
    for d in ["Documents", "Desktop", "Downloads", "Pictures", "Music", "Videos"] {
        std::fs::create_dir_all(home.join(d))?;
    }
    // macOS: what Cocoa resolves into the home (`CFFIXED_USER_HOME`, below) exists there as in a
    // person's home, so an app that only looks a folder up (never creates it) finds one.
    if cfg!(target_os = "macos") {
        for d in ["Movies", "Library/Application Support", "Library/Caches", "Library/Preferences", "Library/Logs", "Library/Saved Application State"] {
            std::fs::create_dir_all(home.join(d))?;
        }
    }
    for d in &app.dirs {
        std::fs::create_dir_all(home_path(&home, d, &format!("app {}: dirs", app.id), &[], false)?.0)?;
    }
    let mut shared = Vec::new();
    if let Some(s) = seed.filter(|_| fresh) {
        copy_dir(s, &home, &mut shared)?;
        let list: String = shared.iter().map(|s| format!("{}\t{}\n", s.link.display(), s.cache.display())).collect();
        std::fs::write(home.join(".strobe-shared"), list)?;
    } else if let Ok(text) = std::fs::read_to_string(home.join(".strobe-shared")) {
        // A warm home keeps its links: watch them for this launch too.
        // The app can write that file: only a link in the home to a copy in strobe's seed folder counts.
        for (link, cache) in text.lines().filter_map(|l| l.split_once('\t')) {
            let (link, cache) = (Path::new(link), Path::new(cache));
            if !shared_pair(&home, link, cache) {
                continue;
            }
            let Some(dir) = cache.parent() else { continue };
            let Ok(lease) = host::cache::take(dir) else { continue };
            let lease = std::sync::Arc::new(lease);
            host::cache::keep(dir, &lease);
            if let Some((len, modified)) = stamp(cache) {
                shared.push(Shared { link: link.into(), cache: cache.into(), len, modified, _lease: lease });
            }
        }
    }
    std::fs::write(home.join(".strobe-home"), "")?;
    // A seed copied from a used profile may hold a stale port file.
    for stale in find(&home, "DevToolsActivePort", 6) {
        let _ = std::fs::remove_file(stale);
    }
    let kept = std::fs::read_to_string(home.join(".strobe-port")).ok().and_then(|p| p.trim().parse::<u16>().ok());
    let port = match kept {
        Some(p) => p,
        None => std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port(),
    };
    std::fs::write(home.join(".strobe-port"), port.to_string())?;
    let fill = |s: &str| s.replace("{home}", &home.display().to_string()).replace("{port}", &port.to_string());
    for (rel, content) in &app.files {
        let (path, _) = home_path(&home, rel, &format!("app {}: files", app.id), &shared, false)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = match content {
            Value::String(s) => fill(s),
            v => fill_json(v, &fill, port).to_string(),
        };
        std::fs::write(path, text)?;
    }
    // Over the seed's and the profile's files: a JSON object merged into the JSON object a file holds
    // (the app's other settings stay; comments and trailing commas are read, not kept), text written as
    // is. A file there that holds no JSON object fails: replacing it would drop the app's settings.
    for (rel, content) in &app.merged_files {
        let what = format!("app {}: motion.reduce.files", app.id);
        let (path, held) = home_path(&home, rel, &what, &shared, true)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let filled = match content {
            Value::String(s) => Value::String(fill(s)),
            v => fill_json(v, &fill, port),
        };
        let text = match (&filled, held) {
            (Value::String(s), _) => s.clone(),
            (Value::Object(_), Some(held)) => match serde_json::from_str::<Value>(&crate::jsonc::strip(&held)) {
                Ok(mut held) if held.is_object() => {
                    merge_json(&mut held, &filled);
                    serde_json::to_string_pretty(&held)?
                }
                _ => {
                    return Err(Error::usage(format!("{what} \"{rel}\": the file the home holds there ({}) is not a JSON object to merge into", path.display()))
                        .hint("seed that file as a JSON object (comments allowed), or give the whole file as text in motion.reduce.files"));
                }
            },
            (v, _) => serde_json::to_string_pretty(v)?,
        };
        std::fs::write(path, text)?;
    }
    save_protocols(&home, &app.protocols);
    #[cfg(target_os = "macos")]
    macos_library::save(&home, app);
    let text = |p: &Path| p.display().to_string();
    let mut env: Vec<(String, String)> = [
        ("USERPROFILE", text(&home)),
        ("HOME", text(&home)),
        ("APPDATA", text(&appdata)),
        ("LOCALAPPDATA", text(&local)),
        ("TEMP", text(&temp)),
        ("TMP", text(&temp)),
        ("XDG_DATA_HOME", text(&data)),
        ("XDG_STATE_HOME", text(&state)),
        ("XDG_CONFIG_HOME", text(&config)),
        ("XDG_CACHE_HOME", text(&cache)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    // macOS: Cocoa resolves the home (`NSHomeDirectory`) and the user's folders (`NSSearchPath…`:
    // Documents, Desktop, Downloads, Application Support, Caches, Logs) from the account, not `HOME`;
    // CoreFoundation's fixed-home variable moves all of them into the home. Preferences still go
    // through `cfprefsd` to the user's own Library (the Library rule below).
    if cfg!(target_os = "macos") {
        env.push(("CFFIXED_USER_HOME".into(), text(&home)));
    }
    // A native app on Wayland: its session (buses with this home as theirs, the compositor) starts now,
    // before any timed span, on strobe's CPUs.
    #[cfg(target_os = "linux")]
    let display = if app.wayland() {
        match super::wayland::Session::start(&env, app.display.is_none()) {
            Ok(s) => Some(std::sync::Arc::new(s)),
            Err(e) => {
                if fresh {
                    remove_home(&home);
                }
                return Err(e);
            }
        }
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    if let Some(d) = &display {
        env.extend(d.env());
    }
    env.extend(app.env.iter().map(|(k, v)| (k.clone(), fill(v))));
    // Last, so no profile replaces them: what tells strobe's launches (and what they start) apart.
    env.extend(launch_env(&home));
    let gpu_lease = app
        .gpu_cache
        .as_deref()
        .map(|store| -> Result<_> {
            let lease = std::sync::Arc::new(host::cache::take(store)?);
            host::cache::keep(store, &lease);
            Ok(lease)
        })
        .transpose()?;
    let gpu_warm = app.gpu_cache.as_deref().map(|store| lend_gpu_caches(&home, store));
    Ok(Home {
        leases: gpu_lease.into_iter().collect(),
        dir: home,
        port,
        env,
        shared,
        gpu_warm,
        fresh,
        #[cfg(target_os = "linux")]
        display,
    })
}

/// A spawned app: the child with its group, when it was spawned (epoch ms and the host clock: its
/// release, `Owned::released_ns`, or just before a Launch Services request), its output lines when
/// asked for, how it was started, and its OS
/// launch-feedback watch when the app asked for one (`App::with_feedback`).
pub struct Spawned {
    pub child: Owned,
    pub pid: u32,
    pub epoch_ms: f64,
    pub at_ns: u64,
    pub output: Option<std::sync::mpsc::Receiver<crate::target::isolate::OutputLine>>,
    pub path: LaunchPath,
    /// Its time less `at_ns` is the metric (`host::ms_between(at_ns, t)`).
    pub feedback: Option<host::Feedback>,
}

/// Starts `app` in `home`: strobe's environment less the profile's `unset`, then the home's, then
/// `env`; the profile's `args`, then `args`; pinned to `cpus` when given (children inherit it); in a
/// group of its own (`host::spawn_owned`), or through Launch Services (`App::with_launch_path`).
pub fn spawn(app: &App, home: &Home, args: &[String], env: &[(String, String)], output: bool, cpus: Option<u64>) -> Result<Spawned> {
    installed(app)?;
    if app.launch == LaunchPath::LaunchServices {
        #[cfg(target_os = "macos")]
        return open(app, home, args, env, output);
        #[cfg(not(target_os = "macos"))]
        return Err(crate::model::Refusal::new(crate::model::ValidatorId::NotOnThisOs, "Launch Services is macOS's: this OS launches an app by spawning it").into());
    }
    // Announced before the launch's time starts: its id goes into the app's environment.
    let notice = app.feedback.filter(|_| cfg!(target_os = "linux")).map(|within| startup_notice(app, std::time::Instant::now() + within));
    let mut cmd = Command::new(&app.exe);
    for (k, _) in std::env::vars() {
        if app.unsets(&k) || home.unsets(&k) {
            cmd.env_remove(&k);
        }
    }
    for (k, v) in home.env.iter().chain(env) {
        cmd.env(k, v);
    }
    if let Some(Ok(n)) = &notice {
        cmd.env(&n.env.0, &n.env.1);
    }
    for a in app.args.iter().chain(args) {
        cmd.arg(home.fill(a));
    }
    let piped = || if output { Stdio::piped() } else { Stdio::null() };
    cmd.stdin(Stdio::null()).stdout(piped()).stderr(piped());
    // macOS: an app bundle Launch Services has no record of gets one as its app checks in; asked before
    // the launch's time starts, and removed when the home is released (`macos_library::forget`).
    #[cfg(target_os = "macos")]
    if let Ok(b) = host::macos::bundle_of(&app.exe) {
        macos_library::note_unrecorded(&home.dir, &b);
    }
    let (epoch, at_ns) = (epoch_ms(), host::now_ns());
    let mut child = host::spawn_owned(&mut cmd, cpus, 0, Some(&home.dir)).map_err(|e| Error::msg(format!("launch {}: {e}", app.exe.display())))?;
    // The launch's time from the child's release (`Owned::released_ns`), with the OS creating the
    // process (Windows), as a person's launch: strobe's job, cgroup and CPU mask are before it.
    let from = child.released_ns().saturating_sub(child.creation_ns()).max(at_ns);
    let (epoch, at_ns) = (epoch + (from - at_ns) as f64 / 1e6, from);
    let feedback = app.feedback.map(|within| watch_feedback(&child, notice, within));
    let output = output.then(|| lines(&mut child));
    Ok(Spawned { pid: child.pid(), child, epoch_ms: epoch, at_ns, output, path: LaunchPath::Spawn, feedback })
}

/// The OS launch-feedback watch of a launch just made, for `within` from now: Windows' input idle on
/// the child, macOS's check-in by the pid and start time the launch read, Linux's startup
/// notification announced before it (`notice`; on Wayland or without a display, why not).
fn watch_feedback(child: &Owned, notice: Option<std::result::Result<StartupNotice, host::NoFeedback>>, within: std::time::Duration) -> host::Feedback {
    let deadline = std::time::Instant::now() + within;
    if cfg!(windows) {
        return child.input_idle(deadline);
    }
    if cfg!(target_os = "macos") {
        return host::checkin(child.pid(), child.start(), deadline);
    }
    match notice {
        Some(Ok(n)) => n.feedback,
        Some(Err(why)) => host::Feedback::none(why),
        None => host::Feedback::none(host::NoFeedback::NotOnThisOs),
    }
}

/// `spawn` through Launch Services (macOS, `App::through_launch_services`): the same environment and
/// arguments a direct spawn gives (strobe's environment less the profile's `unset`, then the home's,
/// then `env`), the app made active as `host::raise_windows` would (nobody used the Mac for a minute).
/// Its time starts just before the request (`macos::Opened::at_ns`), after all of strobe's own
/// preparation, as a person's click does. A launch that fails ends what it may have started late
/// (stamped with the home) and the Launch Services record it may have made.
#[cfg(target_os = "macos")]
fn open(app: &App, home: &Home, args: &[String], env: &[(String, String)], output: bool) -> Result<Spawned> {
    use crate::model::{Refusal, ValidatorId};
    use host::macos::{NotBundle, OpenFailed};
    if output {
        return Err(Error::usage(
            "a launch through Launch Services gives strobe none of the app's output (launchd takes its stdout and stderr): read the app's milestones from a log file, or launch it directly",
        ));
    }
    let refuse = |why: &str| -> Error {
        Refusal::new(ValidatorId::NotForTarget, format!("Launch Services launches app bundles: {why}")).fix("give the .app bundle (or its main executable) as the app's exe").into()
    };
    let bundle = host::macos::bundle_of(&app.exe).map_err(|e| {
        refuse(match e {
            NotBundle::Bare => "the executable is not an app bundle's (an .app folder, or the executable in its Contents/MacOS)",
            NotBundle::NoInfo => "the app bundle has no readable Contents/Info.plist",
            NotBundle::NotMain => "the executable is not the one its app bundle launches (CFBundleExecutable)",
        })
    })?;
    if bundle.id.is_none() {
        return Err(refuse("the app bundle has no CFBundleIdentifier, so strobe cannot tell a Launch Services record its launch made from one already there"));
    }
    let mut vars = home.environment(app);
    for (k, v) in env {
        vars.retain(|name, _| !env_name_is(name, k));
        vars.insert(k.clone(), v.clone());
    }
    let vars: Vec<(String, String)> = vars.into_iter().collect();
    let args: Vec<String> = app.args.iter().chain(args).map(|a| home.fill(a)).collect();
    let activate = host::macos::user_idle_s() >= host::macos::IDLE_BEFORE_ACTIVATE_S;
    macos_library::note_unrecorded(&home.dir, &bundle);
    let (child, opened) = match host::spawn::open_owned(&bundle, &args, &vars, &home.dir, activate) {
        Ok(x) => x,
        Err(e) => {
            end_roots(&stamped_home(&home.dir));
            macos_library::forget(&home.dir);
            end_roots(&stamped_home(&home.dir));
            return Err(match e {
                OpenFailed::NoAppKit => Error::msg("AppKit could not be loaded, so Launch Services cannot be asked to launch the app"),
                OpenFailed::BadPath => Error::msg("the app bundle's path makes no file URL"),
                OpenFailed::Refused { domain, code, .. } => Error::msg(format!("Launch Services refused to launch the app ({domain} error {code})")),
                OpenFailed::NoProcess => Error::unreachable("Launch Services reported no running process of the app within 30 s (or the app exited before checking in)"),
                OpenFailed::Stopped => Error::new(130, "stopped while Launch Services launched the app"),
                OpenFailed::Foreign { pid, why } => {
                    Refusal::new(ValidatorId::LiveChannel, format!("Launch Services answered the launch with process {pid}, which is not this launch's ({why}): strobe touched nothing of it"))
                        .fix("quit the app (a copy of it may already run, or it hands a launch to a running copy), then run again")
                        .into()
                }
            });
        }
    };
    let feedback = app.feedback.map(|within| host::checkin(child.pid(), child.start(), std::time::Instant::now() + within));
    Ok(Spawned { pid: child.pid(), child, epoch_ms: opened.epoch_ms, at_ns: opened.at_ns, output: None, path: LaunchPath::LaunchServices, feedback })
}

/// A JSON seed file with placeholders filled; a string that is exactly `{port}` becomes a number.
fn fill_json(v: &Value, fill: &dyn Fn(&str) -> String, port: u16) -> Value {
    match v {
        Value::String(s) if s == "{port}" => json!(port),
        Value::String(s) => Value::String(fill(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| fill_json(x, fill, port)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), fill_json(x, fill, port))).collect()),
        other => other.clone(),
    }
}

/// `over` merged into `into`: objects key by key at every depth, any other value replaced.
fn merge_json(into: &mut Value, over: &Value) {
    match (into, over) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(k) {
                    Some(held) => merge_json(held, v),
                    None => {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (into, over) => *into = over.clone(),
    }
}

/// Files named `name` under `dir`, at most `depth` levels down.
pub fn find(dir: &Path, name: &str, depth: usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .flat_map(|e| {
            let p = e.path();
            if p.is_dir() {
                return if depth > 0 { find(&p, name, depth - 1) } else { Vec::new() };
            }
            if e.file_name() == name { vec![p] } else { Vec::new() }
        })
        .collect()
}

/// A script runtime (`node`, `bun`, `python3.12`, …) rather than an app of its own.
pub fn interpreter(exe: &Path) -> bool {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    ["python", "pythonw", "py", "node", "bun", "deno", "ruby", "perl", "sh", "bash", "java", "javaw", "pwsh", "powershell"]
        .iter()
        .any(|i| stem == *i || stem.strip_prefix(i).is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.')))
}

/// Refuses to launch an app the user is using: its executable is running, or a guarded process
/// (such as the app's own background service) runs from its install folder or a folder named like
/// it. Isolation should make it safe, but the user's own app is never the thing to experiment on.
pub fn live(app: &App, guard: &Guard) -> Result<()> {
    guard.unreadable()?;
    let mut running = host::running(&app.exe);
    if !running.is_empty() {
        // An interpreter runs every script of its kind: a running one says nothing about the app.
        if interpreter(&app.exe) {
            return Err(Error::refused(
                "LiveChannel",
                format!("{} is running, and it is an interpreter: strobe cannot tell your app's copy from any other script it runs", app.exe.display()),
                "give the app an executable of its own: copy the runtime binary under its own name for this bench (works on every OS), or launch the script itself (a shebang line and chmod +x on Linux and macOS, or a small launcher)",
            ));
        }
        // A copy strobe launched (stamped) is not the user's: a leftover is ended; one that a
        // running strobe (this one included) still holds is named.
        // Each with its start time, read before and after its stamp: the stamp is that process's.
        let stamps: Vec<(u32, Option<u64>, Option<Stamp>)> = running
            .iter()
            .map(|&p| {
                let start = host::start_time(p);
                let s = stamp_of(p);
                (p, start.filter(|&t| host::start_time(p) == Some(t)), s)
            })
            .collect();
        let left: Vec<Stamped> = stamps.iter().filter_map(|(p, t, s)| Some(Stamped { pid: *p, parent: 0, start: (*t)?, stamp: s.clone().filter(leftover)? })).collect();
        if !end_roots(&left).is_empty() {
            for _ in 0..50 {
                running = host::running(&app.exe);
                if !running.iter().any(|p| left.iter().any(|l| l.pid == *p)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        if let Some((pid, _, Some(s))) = stamps.iter().find(|(p, _, s)| running.contains(p) && s.as_ref().is_some_and(|s| !leftover(s))) {
            return Err(if s.pid == std::process::id() {
                Error::refused(
                    "LiveChannel",
                    format!("{} is running (pid {pid}) from {}, a home this strobe launched it into and has not released", app.exe.display(), s.home),
                    "a launch did not close: end that process",
                )
            } else {
                Error::refused(
                    "LiveChannel",
                    format!("{} is running in the isolated home of another strobe that is measuring it", app.exe.display()),
                    "wait for that strobe to finish, or stop it; strobe ends such a copy by itself once that strobe is gone",
                )
            });
        }
        if !running.is_empty() {
            return Err(Error::refused(
                "LiveChannel",
                format!("{} is running: that is your app (no strobe launched it)", app.exe.display()),
                "measure another build or channel, or close the running app",
            ));
        }
    }
    // The install folder: a macOS app's bundle, else the executable's folder.
    let install = app_bundle(&app.exe).or(app.exe.parent());
    let sep = std::path::MAIN_SEPARATOR;
    let dir = install.map(|d| d.to_string_lossy().to_lowercase()).unwrap_or_default();
    let folder = install.and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    for pid in guard.pids() {
        let Some(exe) = host::exe_of(pid) else { continue };
        let path = exe.to_string_lossy().to_lowercase();
        if (!folder.is_empty() && path.contains(&format!("{sep}{folder}{sep}"))) || (!dir.is_empty() && path.starts_with(&dir)) {
            return Err(Error::refused("LiveChannel", format!("a guarded process of your live {} runs from this app", app.id), "measure another build or channel"));
        }
    }
    #[cfg(target_os = "macos")]
    macos_library::check(app)?;
    Ok(())
}

/// `live`'s Library rule alone (macOS: an app whose data Cocoa keeps in the user's ~/Library whatever
/// HOME says is refused), for a launch that skips the running-copy check: a terminal program the user
/// may also run elsewhere (its own copy never shares the isolated home).
pub fn library_check(app: &App) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_library::check(app)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

/// A launch until its target takes it (`done`): dropped before (a failed start, an early return), it
/// ends what the launch started (`end_launch`) and releases its folder, removing it when `fresh`.
pub struct Launching {
    pub child: Option<Owned>,
    pub home: Option<PathBuf>,
    /// The launch made the folder: it goes with a failed launch.
    pub fresh: bool,
    done: bool,
}

impl Launching {
    pub fn new(child: Owned, home: Option<PathBuf>, fresh: bool) -> Launching {
        Launching { child: Some(child), home, fresh, done: false }
    }

    /// The launch started: its target owns the child and the folder from here.
    pub fn done(mut self) -> (Option<Owned>, Option<PathBuf>) {
        self.done = true;
        (self.child.take(), self.home.take())
    }
}

impl Drop for Launching {
    fn drop(&mut self) {
        if !self.done {
            end_launch(self.child.take(), self.home.take().as_deref(), self.fresh);
        }
    }
}

/// URL schemes an app registers on start (`protocols` in its profile) live in the per-user registry,
/// which no environment variable redirects: strobe saves each key into the private home first.
fn save_protocols(home: &Path, schemes: &[String]) {
    #[cfg(windows)]
    for scheme in schemes {
        let key = format!(r"HKCU\Software\Classes\{scheme}");
        let saved = Command::new("reg")
            .args(["export", &key])
            .arg(home.join(format!(".strobe-protocol-{scheme}.reg")))
            .arg("/y")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !saved {
            let _ = std::fs::write(home.join(format!(".strobe-protocol-{scheme}.absent")), "");
        }
    }
    let _ = (home, schemes);
}

/// A `reg export` file's text: UTF-16LE after its byte-order mark, else UTF-8.
#[cfg(windows)]
fn reg_text(b: &[u8]) -> String {
    match b {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>()),
        _ => String::from_utf8_lossy(b).to_string(),
    }
}

/// Puts every saved scheme back as it was before the launch (deleting one that did not exist).
pub fn restore_protocols(home: &Path) {
    // Normal teardown can already be restoring this home when the second-stop worker starts.
    #[cfg(any(windows, target_os = "macos"))]
    static RESTORING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[cfg(any(windows, target_os = "macos"))]
    let _restoring = RESTORING.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(target_os = "macos")]
    {
        macos_library::restore(home);
        macos_library::forget(home);
    }
    #[cfg(windows)]
    for e in std::fs::read_dir(home).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(".strobe-protocol-") else { continue };
        let (scheme, reg) = match (rest.strip_suffix(".reg"), rest.strip_suffix(".absent")) {
            (Some(s), _) => (s.to_string(), true),
            (_, Some(s)) => (s.to_string(), false),
            _ => continue,
        };
        // The app in the home can write these files: only a URL scheme's name, and a saved key that holds
        // that scheme's key alone, are put back (rule 10).
        // Not a link, not a class Windows or the shell owns (whose user-level override a delete would drop),
        // and a URL scheme's key (`URL Protocol`): the saved one, or for one that did not exist, the one there now.
        const BUILT_IN: [&str; 24] = [
            "http",
            "https",
            "ftp",
            "file",
            "mailto",
            "tel",
            "ldap",
            "news",
            "nntp",
            "exefile",
            "lnkfile",
            "batfile",
            "cmdfile",
            "comfile",
            "clsid",
            "directory",
            "folder",
            "drive",
            "allfilesystemobjects",
            "appid",
            "interface",
            "typelib",
            "wow6432node",
            "applications",
        ];
        let lower = scheme.to_ascii_lowercase();
        let plain = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
            && !BUILT_IN.contains(&lower.as_str())
            && !lower.starts_with("ms-")
            && !lower.starts_with("microsoft")
            && e.file_type().is_ok_and(|t| t.is_file());
        let own = |line: &str| {
            let l = line.trim_start_matches('\u{feff}').trim_matches('\0').trim().to_ascii_lowercase();
            let k = format!(r"hkey_current_user\software\classes\{lower}");
            !l.starts_with('[') || l == format!("[{k}]") || l.starts_with(&format!("[{k}\\")) || l == format!("[-{k}]")
        };
        let key = format!(r"HKCU\Software\Classes\{scheme}");
        let text = if reg { std::fs::read(e.path()).map(|b| reg_text(&b)).unwrap_or_default() } else { String::new() };
        let url = if reg {
            text.lines().any(|l| l.trim().to_ascii_lowercase().starts_with("\"url protocol\"="))
        } else {
            Command::new("reg").args(["query", &key, "/v", "URL Protocol"]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        };
        if !plain || !url || (reg && !text.lines().all(own)) {
            continue;
        }
        let _ = Command::new("reg").args(["delete", &key, "/f"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if reg {
            let _ = Command::new("reg").arg("import").arg(e.path()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
    let _ = home;
}

/// macOS keeps an app's data where `HOME` does not reach: Cocoa resolves the home from the user's
/// account (`NSHomeDirectory`, Application Support, caches, saved window state), which only
/// `CFFIXED_USER_HOME` (set by `prepare_home`) moves, and `cfprefsd` writes its preferences for the
/// user whatever the process's home. A launched copy may still write into the user's own `~/Library`
/// folders for its bundle id (a bare executable: for its name, through a daemon). So strobe launches
/// such an app only while none of those folders exist (a build with its own bundle id, or an app this
/// user never ran), records which it may create, and removes them (and the preferences domain) when
/// the launch is released. A sandboxed app's container is in the user's Library too: refused.
#[cfg(target_os = "macos")]
mod macos_library {
    use super::*;

    /// The bundle id (an app bundle's `CFBundleIdentifier`, or a bare executable's own Info.plist
    /// linked into it), else the executable's name, which is what Cocoa uses for a bare executable's
    /// preferences; and a bundle's name.
    fn ids(app: &App) -> (String, Option<String>) {
        let bundle = app_bundle(&app.exe);
        let id = match bundle {
            // The bundle's own Info.plist (XML or binary), as Launch Services reads it.
            Some(b) => host::macos::bundle_of(b)
                .ok()
                .and_then(|b| b.id)
                .or_else(|| host::output(Command::new("plutil").args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"]).arg(b.join("Contents/Info.plist")))),
            None => embedded_id(&app.exe),
        };
        let name = bundle.and_then(|b| b.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| app.exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let bundle_name = (bundle.is_some() && id.is_some()).then(|| name.clone());
        (id.unwrap_or(name), bundle_name)
    }

    /// The `CFBundleIdentifier` of the Info.plist a bare executable carries in its `__TEXT,__info_plist`
    /// section (`-sectcreate`), which CoreFoundation takes as the process's bundle id; any slice of a
    /// universal binary.
    fn embedded_id(exe: &Path) -> Option<String> {
        use object::read::macho::FatArch;
        use object::{Object, ObjectSection};
        let data = std::fs::read(exe).ok()?;
        let from = |bytes: &[u8]| -> Option<String> {
            let obj = object::File::parse(bytes).ok()?;
            let plist = obj.section_by_name("__info_plist")?.data().ok()?;
            let text = String::from_utf8_lossy(plist);
            let after = &text[text.find("<key>CFBundleIdentifier</key>")? + "<key>CFBundleIdentifier</key>".len()..];
            let value = after.trim_start().strip_prefix("<string>")?;
            Some(value[..value.find("</string>")?].trim().to_string()).filter(|s| !s.is_empty())
        };
        match object::FileKind::parse(&*data).ok()? {
            object::FileKind::MachOFat32 => object::read::macho::MachOFatFile32::parse(&*data).ok()?.arches().iter().find_map(|a| from(a.data(&*data).ok()?)),
            object::FileKind::MachOFat64 => object::read::macho::MachOFatFile64::parse(&*data).ok()?.arches().iter().find_map(|a| from(a.data(&*data).ok()?)),
            _ => from(&data),
        }
    }

    /// An executable outside any app bundle (a command-line program, a bare build).
    fn bare(app: &App) -> bool {
        app_bundle(&app.exe).is_none()
    }

    /// The Library folders Cocoa and its daemons key by an id: the folder and the suffix after the id.
    /// The first five for any executable, the rest for a bundle's.
    const BY_ID: [(&str, &str); 8] = [
        ("Preferences", ".plist"),
        ("HTTPStorages", ""),
        ("WebKit", ""),
        ("Cookies", ".binarycookies"),
        ("Containers", ""),
        ("Application Support", ""),
        ("Caches", ""),
        ("Saved Application State", ".savedState"),
    ];

    fn places(app: &App) -> (String, Option<String>, Vec<PathBuf>) {
        let (id, name) = ids(app);
        let lib = host::home().join("Library");
        // The folders a process looks up itself (its caches, data and saved state) follow `HOME` and
        // `CFFIXED_USER_HOME` into the home. A bundle's helpers and services that launchd starts under
        // its id never get that environment, so a bundle's are counted; a bare executable has no
        // bundle id to key them by: what is left in the user's Library for it is what a daemon
        // writes under its name (cfprefsd's preferences, network and web storage).
        let n = if bare(app) { 5 } else { BY_ID.len() };
        let mut p: Vec<PathBuf> = BY_ID[..n].iter().map(|(dir, ext)| lib.join(dir).join(format!("{id}{ext}"))).collect();
        if let Some(n) = &name {
            p.push(lib.join("Application Support").join(n));
        }
        // A folder the profile's env moves into the home (an app that reads its data folder from an
        // environment variable) is the app's own choice, not Cocoa's: its guards watch it.
        p.retain(|place| !app.moved.iter().any(|m| m == place));
        (id, name, p)
    }

    /// A bundle id or an executable's name that is safe as one name in the Library and as an argument
    /// to `defaults`: no `/`, no leading `-` or `.`, no `..`.
    fn plain(s: &str) -> bool {
        !s.is_empty() && s.len() <= 255 && !s.starts_with(['-', '.']) && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || "._- +".contains(c))
    }

    fn sandboxed(app: &App) -> bool {
        let bundle = app_bundle(&app.exe).map(Path::to_path_buf).unwrap_or_else(|| app.exe.clone());
        host::output(Command::new("codesign").args(["-d", "--entitlements", "-", "--xml"]).arg(&bundle).stderr(Stdio::null())).is_some_and(|x| x.contains("com.apple.security.app-sandbox"))
    }

    pub fn check(app: &App) -> Result<()> {
        let (id, _, places) = places(app);
        if sandboxed(app) {
            return Err(Error::refused(
                "LiveChannel",
                format!("{id} is sandboxed: its container lives in your ~/Library, which no launch can redirect"),
                "measure a build without the App Sandbox entitlement",
            ));
        }
        // Only a folder known to be absent counts as absent: one macOS will not let strobe look at
        // (another developer's app data, macOS 27) may exist, and `restore` would remove it.
        if let Some(p) = places.iter().find(|p| !matches!(p.try_exists(), Ok(false))) {
            return Err(Error::refused(
                "LiveChannel",
                format!(
                    "{} {}: macOS keeps {id}'s data in your ~/Library whatever HOME says, so a launched copy would use and change it",
                    p.display(),
                    if p.try_exists().is_err() { "may exist (macOS would not let strobe look)" } else { "exists" }
                ),
                // A bare executable without an Info.plist of its own goes by its file name.
                if bare(app) && app.exe.file_name().is_some_and(|n| n.to_string_lossy() == id) {
                    "measure a copy of the program under another file name (macOS keys a bare executable's data by its name)"
                } else {
                    "measure a build with its own bundle id (or one this account never ran)"
                },
            ));
        }
        Ok(())
    }

    /// Where a launch for `home` records the app bundle whose Launch Services record it may make: beside
    /// the home, never in it (the launched app may write its home, and this names what strobe removes).
    fn unrecorded_file(home: &Path) -> Option<PathBuf> {
        Some(home.parent()?.join(format!(".ls-{}", home.file_name()?.to_string_lossy())))
    }

    /// Before a launch's time starts: when Launch Services has no record of `bundle` (`ls_registered`
    /// is `Some(false)`), notes it for `forget`, which removes the record the launch makes (a launch
    /// through Launch Services, and an app bundle's executable spawned directly, which is recorded as it
    /// checks in). A bundle Launch Services knows already, or one strobe cannot tell (no bundle id), is
    /// not noted: strobe removes only a record it made.
    pub fn note_unrecorded(home: &Path, bundle: &host::macos::Bundle) {
        let Some(file) = unrecorded_file(home) else { return };
        if host::macos::ls_registered(bundle) == Some(false) {
            let _ = std::fs::write(&file, bundle.path.display().to_string());
        }
    }

    /// Removes the Launch Services record a launch for `home` made (`note_unrecorded`), and the note.
    /// Every end of a launch comes here: `release_home` (a close, a failed launch, a stop's
    /// `end_on_stop`) and, after strobe was killed, the next strobe's `reap_dead_owners`. Only an app
    /// bundle outside the system's and the person's Applications folders is unregistered, and only:
    /// - once the launch's processes (stamped with the home, by pid and start time) are gone from the
    ///   process list, not only their environment, which goes first (2 s at most);
    /// - while no copy of the app that is not the launch's runs (one a person opened from Finder: the
    ///   record is theirs too, so it stays, and `spawn::left_alone` says so).
    ///
    /// Then checked: Launch Services may record the bundle again after the first removal (its own
    /// bookkeeping of the app's end, or a request still in flight at a stop: seen on macOS 27), so
    /// strobe asks again (`ls_registered`, a fresh process: never its own stale view) every 0.25 s for
    /// 1.5 s (3 s with a request in flight) and removes a record that came back, three times at most, each
    /// time only while no copy of the app strobe did not launch runs (checked again before each removal:
    /// a person may open it meanwhile). The launch's processes are matched by pid and start time, and a
    /// bundle path that links into an installed app's folder is that installed app.
    ///
    /// The note goes only once that is done: a stop and a failed launch may end the same home at once
    /// from two threads (`end_on_stop`'s and the launch's), and the one the process exits under must
    /// still find it.
    pub fn forget(home: &Path) {
        let Some(file) = unrecorded_file(home) else { return };
        let Ok(text) = std::fs::read_to_string(&file) else { return };
        let path = PathBuf::from(text.lines().next().unwrap_or("").trim());
        // Its real path: a link to an installed app is that installed app.
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let installed = ["/Applications", "/System", "/Library"]
            .iter()
            .map(PathBuf::from)
            .chain([host::home().join("Applications")])
            .any(|d| path.starts_with(&d) || real.starts_with(std::fs::canonicalize(&d).unwrap_or(d)));
        if let Some(b) = host::macos::bundle_of(&path).ok().filter(|b| !installed && b.path == path) {
            unrecord(home, &b);
        }
        let _ = std::fs::remove_file(&file);
    }

    fn unrecord(home: &Path, bundle: &host::macos::Bundle) {
        use std::time::Instant;
        let k = key(home);
        let launch: Vec<(u32, u64)> = stamped().into_iter().filter(|s| s.stamp.home == k).map(|s| (s.pid, s.start)).collect();
        // A copy of the app that is neither one of the launch's processes (by pid and start time: a pid
        // one of them left may be a person's copy's now) nor stamped with its home.
        let others = || -> Vec<u32> {
            host::running(&bundle.executable).into_iter().filter(|p| !launch.iter().any(|l| l.0 == *p && host::start_time(*p) == Some(l.1)) && stamp_of(*p).is_none_or(|s| s.home != k)).collect()
        };
        let keep = |left: Vec<u32>| {
            for p in left {
                host::spawn::leave(Some(p), "a copy of the app strobe did not launch", "it runs, so Launch Services' record of the app bundle (perhaps the person's) stays");
            }
        };
        let until = Instant::now() + Duration::from_secs(2);
        while (launch.iter().any(|&(p, s)| host::start_time(p) == Some(s)) || !others().is_empty()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = others();
        if !left.is_empty() {
            return keep(left);
        }
        host::macos::unregister(&bundle.path);
        let mut removed = 1;
        let mut since = Instant::now();
        loop {
            let window = Duration::from_millis(if host::macos::request_pending() { 3000 } else { 1500 });
            if since.elapsed() >= window || removed >= 3 {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
            if host::macos::ls_registered(bundle) == Some(true) {
                // Before every removal, not only the first: a person may have opened the app meanwhile.
                let left = others();
                if !left.is_empty() {
                    return keep(left);
                }
                host::macos::unregister(&bundle.path);
                removed += 1;
                since = Instant::now();
            }
        }
    }

    /// Records what the launch may create, for `restore`.
    pub fn save(home: &Path, app: &App) {
        let (id, name, places) = places(app);
        let list: Vec<String> = places.iter().filter(|p| matches!(p.try_exists(), Ok(false))).map(|p| p.display().to_string()).collect();
        let _ = std::fs::write(home.join(".strobe-macos-library.json"), json!({ "id": id, "name": name, "created": list }).to_string());
    }

    /// Removes what the launch created in the user's Library, and its preferences domain. The launched
    /// app can write its home, so the record is trusted only this far (rule 10): a plain file; an id
    /// and a bundle name that are plain names; a place only when it is exactly one of the id's (or the
    /// name's) Library folders, not a link, and made after the home was (an older one is the user's).
    /// It goes through `disk::remove`, which never follows a link.
    pub fn restore(home: &Path) {
        let file = home.join(".strobe-macos-library.json");
        if !std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file()) {
            return;
        }
        let Some(v) = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return };
        let Some(id) = v["id"].as_str().filter(|s| plain(s)) else { return };
        let name = v["name"].as_str().filter(|s| plain(s));
        let Ok(since) = std::fs::symlink_metadata(home).and_then(|m| m.created()) else { return };
        let lib = host::home().join("Library");
        let mut allowed: Vec<PathBuf> = BY_ID.iter().map(|(dir, ext)| lib.join(dir).join(format!("{id}{ext}"))).collect();
        allowed.extend(name.map(|n| lib.join("Application Support").join(n)));
        let created: Vec<PathBuf> = v["created"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).filter(|p| allowed.contains(p)).collect();
        // Absent (not written yet), or a plain file or folder this launch made.
        let ours = |p: &Path| match std::fs::symlink_metadata(p) {
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            Ok(m) => !m.file_type().is_symlink() && m.created().is_ok_and(|c| c >= since),
        };
        let prefs = lib.join("Preferences").join(format!("{id}.plist"));
        // Only a domain whose file this launch made: `defaults` also reaches a domain kept elsewhere (an
        // app's container) by its id alone.
        if created.contains(&prefs) && std::fs::symlink_metadata(&prefs).is_ok() && ours(&prefs) {
            let _ = Command::new("defaults").args(["delete", id]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        for p in created {
            if std::fs::symlink_metadata(&p).is_ok() && ours(&p) {
                let _ = host::disk::remove(&p, &lib);
            }
        }
    }
}
