//! Verified and preview paths (SPEC §21.1): which ways of measuring have been checked against a
//! known answer on a real machine. A path is a runtime (what a target runs on), a capability (the
//! instrument a metric is read with: `MetricDef::capability`) and a host (OS and CPU architecture).
//! A path is `verified` when, on a physical machine of that host, a fixture in `scripts/dev/` recovered
//! its planted truth, an A/A of 8 pairs or more stayed inside ±2%, a planted difference fell inside the
//! paired interval, and the path's boundary refusal was seen; every other path strobe runs is
//! `preview`. A path strobe refuses on an OS has no label. The label switches nothing off and changes
//! no measurement, verdict or exit code: it says how far the instrument has been checked.
//!
//! `SUPPORT_MATRIX` is data: each row's cells keep their evidence or what is missing beside them, in
//! one form `scripts/dev/support-matrix.ts` prints and `scripts/doc-check.ts` can compare with SPEC.

use crate::model::{self, WebEngine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// How far a path has been checked. `Preview < Verified`, so a path made of several is the least of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Support {
    /// strobe runs it and its runtime checks hold; no known-answer fixture has checked it on this host.
    Preview,
    /// Checked on this host against a known answer (the four parts in the module's doc).
    Verified,
}

impl Support {
    pub fn name(self) -> &'static str {
        match self {
            Support::Preview => "preview",
            Support::Verified => "verified",
        }
    }
}

impl fmt::Display for Support {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An operating system: a column of the matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

impl Os {
    pub const ALL: [Os; 3] = [Os::Windows, Os::Macos, Os::Linux];

    pub fn label(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Macos => "macOS",
            Os::Linux => "Linux",
        }
    }
}

/// The machine strobe runs on: its OS and CPU architecture. Each OS has one primary architecture,
/// the one its column's evidence was taken on; the other is always `preview` where the OS runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Host {
    WindowsX64,
    WindowsArm64,
    MacosArm64,
    MacosX64,
    LinuxX64,
    LinuxArm64,
}

impl Host {
    pub const ALL: [Host; 6] = [Host::WindowsX64, Host::WindowsArm64, Host::MacosArm64, Host::MacosX64, Host::LinuxX64, Host::LinuxArm64];

    /// This build's host.
    pub fn this() -> Host {
        let arm = cfg!(target_arch = "aarch64");
        if cfg!(windows) {
            if arm { Host::WindowsArm64 } else { Host::WindowsX64 }
        } else if cfg!(target_os = "macos") {
            if arm { Host::MacosArm64 } else { Host::MacosX64 }
        } else if arm {
            Host::LinuxArm64
        } else {
            Host::LinuxX64
        }
    }

    pub fn os(self) -> Os {
        match self {
            Host::WindowsX64 | Host::WindowsArm64 => Os::Windows,
            Host::MacosArm64 | Host::MacosX64 => Os::Macos,
            Host::LinuxX64 | Host::LinuxArm64 => Os::Linux,
        }
    }

    /// The architecture the OS's column was checked on: x64 Windows and Linux, Apple silicon.
    pub fn primary(self) -> bool {
        matches!(self, Host::WindowsX64 | Host::MacosArm64 | Host::LinuxX64)
    }

    /// `windows-x64`, as result.json writes it.
    pub fn name(self) -> &'static str {
        match self {
            Host::WindowsX64 => "windows-x64",
            Host::WindowsArm64 => "windows-arm64",
            Host::MacosArm64 => "macos-arm64",
            Host::MacosX64 => "macos-x64",
            Host::LinuxX64 => "linux-x64",
            Host::LinuxArm64 => "linux-arm64",
        }
    }

    /// `Windows x64`, as text says it.
    pub fn label(self) -> &'static str {
        match self {
            Host::WindowsX64 => "Windows x64",
            Host::WindowsArm64 => "Windows arm64",
            Host::MacosArm64 => "macOS arm64",
            Host::MacosX64 => "macOS x64",
            Host::LinuxX64 => "Linux x64",
            Host::LinuxArm64 => "Linux arm64",
        }
    }

    pub fn parse(s: &str) -> Option<Host> {
        Host::ALL.into_iter().find(|h| h.name() == s)
    }
}

/// What a target runs on (SPEC §9.6, §13): its launcher and protocol, which every instrument reads through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// `web()`, `url()` in Chromium (the default engine), headed, headless or emulating a device.
    ChromiumPage,
    /// `web()`, `url()` with `engine: "firefox"` (WebDriver BiDi).
    FirefoxPage,
    /// `web()`, `url()` with `engine: "webkit"` (Safari on macOS; WebKitGTK or WPE on Linux).
    WebkitPage,
    /// A page target under `--mobile`: Chrome on an Android phone.
    AndroidChrome,
    /// `record wir:`: Safari on an iPhone over Web Inspector (no bench target).
    IosSafari,
    /// `app()`, `exe()`, `command()`: an app on Chromium (an isolated home, CDP, its process tree).
    ChromiumApp,
    /// `webview()`: an app on the OS's webview.
    WebviewApp,
    /// `native()` with a window, driven through the OS's accessibility tree.
    Native,
    /// `native()` with `window: false` (on the target or its app profile).
    NativeWindowless,
    /// `script()`: a module in a Bun or Node host.
    Script,
    /// `cli()` and `strobe run --base … --head …`: a command run to its exit.
    Cli,
    /// `tty()`: a program in a pseudo-terminal, read as a grid.
    Tty,
}

impl Runtime {
    pub const ALL: [Runtime; 12] = [
        Runtime::ChromiumPage,
        Runtime::FirefoxPage,
        Runtime::WebkitPage,
        Runtime::AndroidChrome,
        Runtime::IosSafari,
        Runtime::ChromiumApp,
        Runtime::WebviewApp,
        Runtime::Native,
        Runtime::NativeWindowless,
        Runtime::Script,
        Runtime::Cli,
        Runtime::Tty,
    ];

    /// `chromium_page`, as result.json writes it.
    pub fn name(self) -> &'static str {
        match self {
            Runtime::ChromiumPage => "chromium_page",
            Runtime::FirefoxPage => "firefox_page",
            Runtime::WebkitPage => "webkit_page",
            Runtime::AndroidChrome => "android_chrome",
            Runtime::IosSafari => "ios_safari",
            Runtime::ChromiumApp => "chromium_app",
            Runtime::WebviewApp => "webview_app",
            Runtime::Native => "native",
            Runtime::NativeWindowless => "native_windowless",
            Runtime::Script => "script",
            Runtime::Cli => "cli",
            Runtime::Tty => "tty",
        }
    }

    /// `chromium page`, as text says it.
    pub fn label(self) -> &'static str {
        match self {
            Runtime::ChromiumPage => "chromium page",
            Runtime::FirefoxPage => "firefox page",
            Runtime::WebkitPage => "webkit page",
            Runtime::AndroidChrome => "chrome on a phone",
            Runtime::IosSafari => "safari on an iphone",
            Runtime::ChromiumApp => "chromium app",
            Runtime::WebviewApp => "webview app",
            Runtime::Native => "native",
            Runtime::NativeWindowless => "windowless native",
            Runtime::Script => "script",
            Runtime::Cli => "cli",
            Runtime::Tty => "tty",
        }
    }

    pub fn parse(s: &str) -> Option<Runtime> {
        Runtime::ALL.into_iter().find(|r| r.name() == s)
    }

    /// The runtime of a bench target as the runner describes it (`describe["targets"][name]`, after
    /// `--engine` set every page's `engine`). `windowless`: the target opens no window (`window: false`
    /// on it or its app profile); `phone`: the bench runs its pages on a phone (`--mobile`). None for
    /// a kind strobe does not know.
    pub fn of_target(target: &Value, windowless: bool, phone: bool) -> Option<Runtime> {
        Some(match target["kind"].as_str()? {
            "web" | "url" if phone => Runtime::AndroidChrome,
            "web" | "url" => match target["engine"].as_str().and_then(|e| WebEngine::parse(e).ok()).unwrap_or_default() {
                WebEngine::Chromium => Runtime::ChromiumPage,
                WebEngine::Firefox => Runtime::FirefoxPage,
                WebEngine::Webkit => Runtime::WebkitPage,
            },
            "app" | "exe" | "command" => Runtime::ChromiumApp,
            "webview" => Runtime::WebviewApp,
            "native" if windowless => Runtime::NativeWindowless,
            "native" => Runtime::Native,
            "script" => Runtime::Script,
            "cli" => Runtime::Cli,
            "tty" => Runtime::Tty,
            _ => return None,
        })
    }
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// The instrument a metric is read with: one family of code from the OS or engine to the number. Every
/// catalog metric names one (`MetricDef::capability`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// strobe's own clock and counts: run and phase wall time, `count.*`, `metric.*`, `app.*`, its
    /// input pacing and its own looks at the target.
    WallAndCounts,
    /// What the engine reports of itself: Chromium's trace and Performance domain (main thread, layout,
    /// style, GC, shader compiles, spans), network events, the JS heap and DOM, leak snapshots.
    PageEngine,
    /// The OS's accounting of the target's processes: CPU time from cycle counters or rusage, the main
    /// and UI thread's CPU, context switches, bytes through read and write calls.
    ProcessCpu,
    /// The OS's private memory and peaks of the target's processes.
    ProcessMemory,
    /// The CPU's own counters per process: instructions retired and core energy.
    HardwareCounters,
    /// Wakes of the CPU package from idle (macOS): a whole-chip count the OS attributes per process.
    PackageIdle,
    /// Frames on screen: presented frames, their intervals and drops, a page's canvas frames.
    Frames,
    /// Input to presentation: discrete inputs' and scrolls' latency.
    InputLatency,
    /// The OS's GPU use and memory per process.
    Gpu,
    /// strobe's reads of the window's pixels, and launches timed by them.
    Pixels,
    /// A launch timed from the spawn to a selector or a milestone.
    Launch,
    /// The launch feedback the OS itself gives (a Dock check-in, an input-idle wait, a startup
    /// notification): a signal of the OS, not of the app's own ready state.
    LaunchSignal,
    /// A terminal program's grid: latencies, settles, updates and bytes.
    TtyGrid,
    /// The HTTP probe (`ctx.httpProbe`): latency of a server strobe sends requests to.
    HttpProbe,
}

impl Capability {
    pub const ALL: [Capability; 14] = [
        Capability::WallAndCounts,
        Capability::PageEngine,
        Capability::ProcessCpu,
        Capability::ProcessMemory,
        Capability::HardwareCounters,
        Capability::PackageIdle,
        Capability::Frames,
        Capability::InputLatency,
        Capability::Gpu,
        Capability::Pixels,
        Capability::Launch,
        Capability::LaunchSignal,
        Capability::TtyGrid,
        Capability::HttpProbe,
    ];

    /// `process_cpu`, as result.json writes it.
    pub fn name(self) -> &'static str {
        match self {
            Capability::WallAndCounts => "wall_and_counts",
            Capability::PageEngine => "page_engine",
            Capability::ProcessCpu => "process_cpu",
            Capability::ProcessMemory => "process_memory",
            Capability::HardwareCounters => "hardware_counters",
            Capability::PackageIdle => "package_idle",
            Capability::Frames => "frames",
            Capability::InputLatency => "input_latency",
            Capability::Gpu => "gpu",
            Capability::Pixels => "pixels",
            Capability::Launch => "launch",
            Capability::LaunchSignal => "launch_signal",
            Capability::TtyGrid => "tty_grid",
            Capability::HttpProbe => "http_probe",
        }
    }

    /// `process CPU`, as text says it.
    pub fn label(self) -> &'static str {
        match self {
            Capability::WallAndCounts => "wall time and counts",
            Capability::PageEngine => "page engine",
            Capability::ProcessCpu => "process CPU",
            Capability::ProcessMemory => "process memory",
            Capability::HardwareCounters => "hardware counters",
            Capability::PackageIdle => "package idle wakes",
            Capability::Frames => "frames",
            Capability::InputLatency => "input latency",
            Capability::Gpu => "GPU",
            Capability::Pixels => "pixels",
            Capability::Launch => "launch",
            Capability::LaunchSignal => "OS launch signal",
            Capability::TtyGrid => "terminal grid",
            Capability::HttpProbe => "HTTP probe",
        }
    }

    pub fn parse(s: &str) -> Option<Capability> {
        Capability::ALL.into_iter().find(|c| c.name() == s)
    }

    /// A metric's capability, families by their base (`model::metric`); None for a name strobe does not know.
    pub fn of(metric: &str) -> Option<Capability> {
        model::metric(metric).map(|d| d.capability)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What made a cell `verified`: the four parts, as SPEC §21.1 cites them. Machine classes are generic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Evidence {
    /// The day of the last part (`YYYY-MM-DD`).
    pub date: &'static str,
    /// The strobe version or main commit the runs used.
    pub version: &'static str,
    /// The machine, by class (`Windows 11 x64 desktop`, `Apple silicon laptop (macOS 27.0.1)`).
    pub machine: &'static str,
    /// The known-answer fixture, a path from the repository's root under `scripts/dev/`.
    pub fixture: &'static str,
    /// Part 1: what it read against its truth.
    pub known: &'static str,
    /// Part 2: the A/A, its pairs and interval.
    pub aa: &'static str,
    /// Part 3: the planted difference against its interval.
    pub planted: &'static str,
    /// Part 3: the path's boundary refusal seen on the host.
    pub boundary: &'static str,
}

/// One OS's cell of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Verified(&'static Evidence),
    /// What is missing before it could be verified.
    Preview(&'static str),
    /// strobe refuses this path on the OS, whatever the target does: why.
    Refused(&'static str),
}

/// A runtime and a capability on each OS's primary architecture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub runtime: Runtime,
    pub capability: Capability,
    pub windows: Cell,
    pub macos: Cell,
    pub linux: Cell,
}

impl Row {
    pub fn on(&self, os: Os) -> &Cell {
        match os {
            Os::Windows => &self.windows,
            Os::Macos => &self.macos,
            Os::Linux => &self.linux,
        }
    }
}

/// A path's condition beyond its runtime and capability (OS input to the target), per OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifier {
    pub windows: Cell,
    pub macos: Cell,
    pub linux: Cell,
}

impl Modifier {
    pub fn on(&self, os: Os) -> &Cell {
        match os {
            Os::Windows => &self.windows,
            Os::Macos => &self.macos,
            Os::Linux => &self.linux,
        }
    }
}

use Cell::{Preview, Refused};

// What is missing, said once for the cells it holds for. The evidence these name is SPEC's (§7.5, §8.6,
// §8.9, §9.10, §10.3, and §24 as `git show 3ed0413:SPEC.md` keeps it), mac's is the promotion runs'.
const NO_PAIRED: &str = "no paired A/A (8 pairs or more) or planted A/B of it is recorded on this OS";
const NO_DISPLAY: &str = "only a Linux x64 VM without a display has run it; frames, input, GPU and pixels need a physical desktop with a GPU and a monitor";
const GPU_TRUTH: &str = "no fixture gives GPU use or GPU memory a known answer (a fixed load repeats within 10%, which is no truth)";
const PHONE: &str = "only Android emulators have run it, and an emulator refuses timed runs: a physical phone on a cable has not";
const IPHONE: &str = "record only; its span, TLS and clock fixes are checked on synthetic input, never on a live phone";
const APP: &str = "no known-answer fixture exists for app(), exe() or command() targets: their isolation and process tree are checked, not a planted cost";
const WEBVIEW: &str = "the OS's webview ran in --check only; no known-answer fixture exists for webview() targets";
const NATIVE_WINDOWED: &str = "no planted known-answer run of a windowed native app (UI thread CPU, OS input, frames) is recorded";
const LAUNCH_SIGNAL: &str = "no fixture gives the OS's launch feedback a known answer";
const LAUNCH: &str = "launch.ready of known/native.bench.ts ready-0 and ready-300 ran in --check only; no paired A/A or planted A/B";
const PAGE_LAUNCH: &str = "no planted page launch has run on this OS; the planted launch case is in scripts/dev/known/planted.bench.ts, which has not landed";
const PIXELS: &str = "canvas.bench.ts ran in --check only, and a one-key timed block costs over the 2% budget";
const TTY: &str = "known/tty.bench.ts passes --check with its known frames, bytes and 20 ms answers; no paired A/A or planted A/B";
const HTTP: &str = "http-probe.bench.ts recovers its stall cohorts, but its timed A/A had 6 clean pairs and no planted A/B ran";
const FIREFOX: &str = "Firefox ran headless --check and leak runs (a 100 KB/run plant read +100 KB/run); no paired A/A or planted A/B of a run metric";
const FIREFOX_FRAMES: &str = "no headed Firefox has run against a known answer; its OS presents are checked on no display";
const SCRIPT_ENGINE: &str = "no known-answer fixture checks js.heap.retained";
const SCRIPT_MEMORY: &str = "known/script.bench.ts keep-0 and keep-100 ran in --check only; no paired A/A or planted A/B";
const CLI_WALL: &str = "no known answer for a command's run.duration is recorded, nor a paired A/B of one";
const PAGE_WALL: &str = "counts read exact in known-web.bench.ts; with the display off run.duration is refused Throttled, and no paired A/A or planted A/B of a page's wall time is recorded";
const PAGE_ENGINE: &str =
    "known-web.bench.ts recovers main.work (100.4 to 100.9 ms for 100), layout.count 25, net.bytes byte for byte and a +7.6 MB/run leak; no paired A/A or planted A/B of a page metric is recorded";
const PAGE_MEMORY: &str = "a heap plant was read in --check and --blocks 2 only; no paired A/A or planted A/B";
const NATIVE_WALL: &str = "native page.wait sleeps, so a windowless run has no exact wall truth; no planted wall A/B";
const NATIVE_MEMORY: &str = "known/native.bench.ts hold reads its 150 MiB and a planted leak was found, but no paired A/A or planted A/B of memory.app.private is recorded";
const COUNTERS_UNRUN: &str = "no committed fixture plants instructions on this target; none has run here";

const WIN_CLI_CPU: &str = "known/cli.bench.ts tree read 1218 ms for 1200 and an A/A without load read same (chaos/chaos.bench.ts); no planted A/B of a command is recorded";
const WIN_CLI_MEMORY: &str = "known/cli.bench.ts alloc reads its MiB in --check; no paired A/A or planted A/B of memory.peak is recorded";
const WIN_SCRIPT_CPU: &str = "derive/script.bench.ts counts a child that ends in the run, and an A/A without load read same (chaos/chaos.bench.ts); no planted A/B";
const WIN_PAGE_CPU: &str = "a 100 ms spin read cpu.work 100.1 to 100.2 ms and a headless A/A without load read same (chaos/chaos.bench.ts); no planted A/B";
const WIN_NATIVE_CPU: &str = "known/native.bench.ts spawner read 502 ms for 500 (0.22.0); its A/A (+0.2% [−1.1 … +1.5], 8 pairs) and planted 2× (+99% [+95 … +104]) ran on 0.11.0 with an app outside scripts/dev, before cpu.work's run edges changed (758dc27): run both again on a committed fixture";
const WIN_FRAMES: &str = "one check of a page's trace against the OS's presents on a Windows 11 x64 laptop (1198 frames, p50 4.17 ms both); no quiet A/A or planted A/B";
const WIN_INPUT: &str = "headed input ran in --check only on a Windows 11 x64 laptop; no A/A or planted A/B of a latency";
const WIN_COUNTERS: &str = "Windows gives an unprivileged process no per-process instruction or energy count (NoCounter)";
const NO_IDLE: &str = "only macOS counts package idle wakes per process (NoCounter)";
const NO_WEBKIT: &str = "Windows has no WebKit engine strobe drives (EngineUnsupported)";
const NO_WEBVIEW: &str = "webview() targets drive the OS's webview only on Windows (EngineUnsupported)";

const LINUX_CLI_CPU: &str = "100 A/As of one command said same 99 times, and a 20 M against 22 M loop read cpu.work +9.2% [+8.1 … +11] for +10%; no known-answer cpu.work of known/cli.bench.ts nor a boundary refusal is recorded";
const LINUX_CLI_MEMORY: &str = "memory.peak is refused under strobe's own resident set (the floor said); no known answer above it or paired A/B is recorded";
const LINUX_SCRIPT_CPU: &str = "derive/script.bench.ts read 198 to 204 ms for a 200 ms child, but a planted 2× loop read cpu.work +104% [+103 … +105], which misses +100% (a VM's CPU time)";
const LINUX_SCRIPT_COUNTERS: &str =
    "a planted 1×/2× loop read cpu.instructions +100% [+100 … +100] (A/A spread 0.022%), but the loop is no committed fixture and ran before the counter blocks' fixes of 2026-11-08";
const LINUX_PAGE_CPU: &str = "a busy-wait's cpu.work reads 20% under its wall time in a VM (steal), so no known answer holds here";
const LINUX_NATIVE_CPU: &str = "known tick read 405 ms for 400.9; its A/A (+0.2% [−0.3 … +0.7], 8 pairs) and planted 2× (+97% [+96 … +98] for +96.8%) ran on 0.11.0 with an app outside scripts/dev, before cpu.work's run edges changed (758dc27): run both again on a committed fixture";
const LINUX_COUNTERS: &str = "a native app's instructions were read with no known answer; no committed fixture plants them";

const MAC_UNRUN: &str = "not run against a known answer on macOS";
const MAC_SIGN: &str = "passed all four parts on an Apple silicon laptop (2026-11-12, 964e825); verified once its evidence line is in SPEC §21.1";
const MAC_PLANTED: &str = "passed all four parts on an Apple silicon laptop (2026-11-12); its planted A/B uses scripts/dev/known/planted.bench.ts, which lands with cleanup/promo-mac";
const MAC_CLI_CPU: &str = "passed all four parts on an Apple silicon laptop (2026-11-12); io.read needs the uncached io mode of known.rs, which lands with cleanup/promo-mac";
const MAC_HEADLESS: &str = "passed all four parts on an Apple silicon laptop only with cleanup/promo-mac's front_page fix: main refuses two headless pages compared WindowHidden";
const MAC_FRAMES: &str = "frames passed all four parts on an Apple silicon laptop's 60 Hz display (2026-11-12); its planted case lands with cleanup/promo-mac";
const MAC_INPUT: &str = "a click-latency A/A was underpowered at 26 pairs (−1.4% … +2.8%); needs an A/A on a longer frame spin";
const MAC_NATIVE_MEMORY: &str =
    "passed on an Apple silicon laptop; waits for cleanup/promo-mac's mapfile fix and planted hold cases, and the docs' line that the footprint counts shared anonymous memory its creator made";
const MAC_COUNTERS: &str = "instructions and energy passed on an Apple silicon laptop, but the planted burst-rate case reads +102% for 2× (one wake more a second): fix the fixture and run it again";
const MAC_SCRIPT_COUNTERS: &str = "a planted 1×/2× loop read instructions +100% [+99 … +100] and energy +99% [+93 … +106], but the loop is no committed fixture";
const MAC_SCRIPT_MEMORY: &str = "a Bun host's footprint misses memory it reuses after a GC (keep-100 read +0.19 MB over keep-0 for +100 MiB)";
const MAC_NATIVE_WALL: &str = "page.wait sleeps, which wakes 1.5 to 10 ms late on macOS: no exact wall truth until a fixture busy-waits";
const MAC_IDLE: &str = "the OS counts package idle wakes for the whole chip (0/s for a 100 Hz timer, 8 to 36/s for 50 Hz): no isolated known answer exists";
const MAC_FIREFOX: &str = "macOS holds a downloaded Firefox at launch until a person opens it once; not run against a known answer";
const MAC_SAFARI: &str = "Safari ran in --check only";
const LINUX_WEBKIT: &str = "WPE ran headless --check; WebKitGTK under a virtual display refuses SoftwareGpu";

/// Every path strobe can run, with its support on each OS's primary architecture. A runtime and
/// capability with no row is not measured for that kind of target (strobe refuses `NotForTarget`).
pub const SUPPORT_MATRIX: &[Row] = &[
    Row { runtime: Runtime::ChromiumPage, capability: Capability::WallAndCounts, windows: Preview(PAGE_WALL), macos: Preview(MAC_HEADLESS), linux: Preview(PAGE_WALL) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::PageEngine, windows: Preview(PAGE_ENGINE), macos: Preview(MAC_HEADLESS), linux: Preview(PAGE_ENGINE) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::ProcessCpu, windows: Preview(WIN_PAGE_CPU), macos: Preview(MAC_HEADLESS), linux: Preview(LINUX_PAGE_CPU) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::ProcessMemory, windows: Preview(PAGE_MEMORY), macos: Preview(MAC_UNRUN), linux: Preview(PAGE_MEMORY) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::HardwareCounters, windows: Refused(WIN_COUNTERS), macos: Preview(MAC_UNRUN), linux: Preview(COUNTERS_UNRUN) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::PackageIdle, windows: Refused(NO_IDLE), macos: Preview(MAC_IDLE), linux: Refused(NO_IDLE) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::Frames, windows: Preview(WIN_FRAMES), macos: Preview(MAC_FRAMES), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::InputLatency, windows: Preview(WIN_INPUT), macos: Preview(MAC_INPUT), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::Gpu, windows: Preview(GPU_TRUTH), macos: Preview(GPU_TRUTH), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::Pixels, windows: Preview(PIXELS), macos: Preview(MAC_UNRUN), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::Launch, windows: Preview(PAGE_LAUNCH), macos: Preview(MAC_PLANTED), linux: Preview(PAGE_LAUNCH) },
    Row { runtime: Runtime::ChromiumPage, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_UNRUN), linux: Preview(HTTP) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::WallAndCounts, windows: Preview(FIREFOX), macos: Preview(MAC_FIREFOX), linux: Preview(FIREFOX) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::PageEngine, windows: Preview(FIREFOX), macos: Preview(MAC_FIREFOX), linux: Preview(FIREFOX) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::ProcessCpu, windows: Preview(FIREFOX), macos: Preview(MAC_FIREFOX), linux: Preview(LINUX_PAGE_CPU) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::ProcessMemory, windows: Preview(FIREFOX), macos: Preview(MAC_FIREFOX), linux: Preview(FIREFOX) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::HardwareCounters, windows: Refused(WIN_COUNTERS), macos: Preview(MAC_FIREFOX), linux: Preview(COUNTERS_UNRUN) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::PackageIdle, windows: Refused(NO_IDLE), macos: Preview(MAC_IDLE), linux: Refused(NO_IDLE) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::Frames, windows: Preview(FIREFOX_FRAMES), macos: Preview(MAC_FIREFOX), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::InputLatency, windows: Preview(FIREFOX_FRAMES), macos: Preview(MAC_FIREFOX), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::Gpu, windows: Preview(GPU_TRUTH), macos: Preview(GPU_TRUTH), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::Pixels, windows: Preview(PIXELS), macos: Preview(MAC_FIREFOX), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::Launch, windows: Preview(PAGE_LAUNCH), macos: Preview(MAC_FIREFOX), linux: Preview(PAGE_LAUNCH) },
    Row { runtime: Runtime::FirefoxPage, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_FIREFOX), linux: Preview(HTTP) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::WallAndCounts, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(LINUX_WEBKIT) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::PageEngine, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(LINUX_WEBKIT) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::ProcessCpu, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(LINUX_WEBKIT) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::ProcessMemory, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(LINUX_WEBKIT) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::HardwareCounters, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(COUNTERS_UNRUN) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::PackageIdle, windows: Refused(NO_WEBKIT), macos: Preview(MAC_IDLE), linux: Refused(NO_IDLE) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::Frames, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::InputLatency, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::Gpu, windows: Refused(NO_WEBKIT), macos: Preview(GPU_TRUTH), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::Pixels, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::WebkitPage, capability: Capability::HttpProbe, windows: Refused(NO_WEBKIT), macos: Preview(MAC_SAFARI), linux: Preview(HTTP) },
    Row { runtime: Runtime::AndroidChrome, capability: Capability::WallAndCounts, windows: Preview(PHONE), macos: Preview(PHONE), linux: Preview(PHONE) },
    Row { runtime: Runtime::AndroidChrome, capability: Capability::PageEngine, windows: Preview(PHONE), macos: Preview(PHONE), linux: Preview(PHONE) },
    Row { runtime: Runtime::AndroidChrome, capability: Capability::ProcessCpu, windows: Preview(PHONE), macos: Preview(PHONE), linux: Preview(PHONE) },
    Row { runtime: Runtime::AndroidChrome, capability: Capability::ProcessMemory, windows: Preview(PHONE), macos: Preview(PHONE), linux: Preview(PHONE) },
    Row { runtime: Runtime::AndroidChrome, capability: Capability::Frames, windows: Preview(PHONE), macos: Preview(PHONE), linux: Preview(PHONE) },
    Row { runtime: Runtime::AndroidChrome, capability: Capability::InputLatency, windows: Preview(PHONE), macos: Preview(PHONE), linux: Preview(PHONE) },
    Row { runtime: Runtime::AndroidChrome, capability: Capability::HttpProbe, windows: Preview(PHONE), macos: Preview(PHONE), linux: Preview(PHONE) },
    Row { runtime: Runtime::IosSafari, capability: Capability::WallAndCounts, windows: Preview(IPHONE), macos: Preview(IPHONE), linux: Preview(IPHONE) },
    Row { runtime: Runtime::IosSafari, capability: Capability::PageEngine, windows: Preview(IPHONE), macos: Preview(IPHONE), linux: Preview(IPHONE) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::WallAndCounts, windows: Preview(APP), macos: Preview(APP), linux: Preview(APP) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::PageEngine, windows: Preview(APP), macos: Preview(APP), linux: Preview(APP) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::ProcessCpu, windows: Preview(APP), macos: Preview(APP), linux: Preview(APP) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::ProcessMemory, windows: Preview(APP), macos: Preview(APP), linux: Preview(APP) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::HardwareCounters, windows: Refused(WIN_COUNTERS), macos: Preview(APP), linux: Preview(APP) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::PackageIdle, windows: Refused(NO_IDLE), macos: Preview(MAC_IDLE), linux: Refused(NO_IDLE) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::Frames, windows: Preview(APP), macos: Preview(APP), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::InputLatency, windows: Preview(APP), macos: Preview(APP), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::Gpu, windows: Preview(GPU_TRUTH), macos: Preview(GPU_TRUTH), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::Pixels, windows: Preview(APP), macos: Preview(APP), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::Launch, windows: Preview(APP), macos: Preview(APP), linux: Preview(APP) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::LaunchSignal, windows: Preview(LAUNCH_SIGNAL), macos: Preview(LAUNCH_SIGNAL), linux: Preview(LAUNCH_SIGNAL) },
    Row { runtime: Runtime::ChromiumApp, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_UNRUN), linux: Preview(HTTP) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::WallAndCounts, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::PageEngine, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::ProcessCpu, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::ProcessMemory, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::HardwareCounters, windows: Refused(WIN_COUNTERS), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::PackageIdle, windows: Refused(NO_IDLE), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::Frames, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::InputLatency, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::Gpu, windows: Preview(GPU_TRUTH), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::Pixels, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::Launch, windows: Preview(WEBVIEW), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::LaunchSignal, windows: Preview(LAUNCH_SIGNAL), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::WebviewApp, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Refused(NO_WEBVIEW), linux: Refused(NO_WEBVIEW) },
    Row { runtime: Runtime::Native, capability: Capability::WallAndCounts, windows: Preview(NATIVE_WINDOWED), macos: Preview(NATIVE_WINDOWED), linux: Preview(NATIVE_WINDOWED) },
    Row { runtime: Runtime::Native, capability: Capability::ProcessCpu, windows: Preview(NATIVE_WINDOWED), macos: Preview(NATIVE_WINDOWED), linux: Preview(NATIVE_WINDOWED) },
    Row { runtime: Runtime::Native, capability: Capability::ProcessMemory, windows: Preview(NATIVE_WINDOWED), macos: Preview(NATIVE_WINDOWED), linux: Preview(NATIVE_WINDOWED) },
    Row { runtime: Runtime::Native, capability: Capability::HardwareCounters, windows: Refused(WIN_COUNTERS), macos: Preview(NATIVE_WINDOWED), linux: Preview(NATIVE_WINDOWED) },
    Row { runtime: Runtime::Native, capability: Capability::PackageIdle, windows: Refused(NO_IDLE), macos: Preview(MAC_IDLE), linux: Refused(NO_IDLE) },
    Row { runtime: Runtime::Native, capability: Capability::Frames, windows: Preview(NATIVE_WINDOWED), macos: Preview(NATIVE_WINDOWED), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::Native, capability: Capability::InputLatency, windows: Preview(NATIVE_WINDOWED), macos: Preview(NATIVE_WINDOWED), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::Native, capability: Capability::Gpu, windows: Preview(GPU_TRUTH), macos: Preview(GPU_TRUTH), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::Native, capability: Capability::Pixels, windows: Preview(PIXELS), macos: Preview(NATIVE_WINDOWED), linux: Preview(NO_DISPLAY) },
    Row { runtime: Runtime::Native, capability: Capability::Launch, windows: Preview(NATIVE_WINDOWED), macos: Preview(NATIVE_WINDOWED), linux: Preview(NATIVE_WINDOWED) },
    Row { runtime: Runtime::Native, capability: Capability::LaunchSignal, windows: Preview(LAUNCH_SIGNAL), macos: Preview(LAUNCH_SIGNAL), linux: Preview(LAUNCH_SIGNAL) },
    Row { runtime: Runtime::Native, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_UNRUN), linux: Preview(HTTP) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::WallAndCounts, windows: Preview(NATIVE_WALL), macos: Preview(MAC_NATIVE_WALL), linux: Preview(NATIVE_WALL) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::ProcessCpu, windows: Preview(WIN_NATIVE_CPU), macos: Preview(MAC_PLANTED), linux: Preview(LINUX_NATIVE_CPU) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::ProcessMemory, windows: Preview(NATIVE_MEMORY), macos: Preview(MAC_NATIVE_MEMORY), linux: Preview(NATIVE_MEMORY) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::HardwareCounters, windows: Refused(WIN_COUNTERS), macos: Preview(MAC_COUNTERS), linux: Preview(LINUX_COUNTERS) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::PackageIdle, windows: Refused(NO_IDLE), macos: Preview(MAC_IDLE), linux: Refused(NO_IDLE) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::Gpu, windows: Preview(GPU_TRUTH), macos: Preview(GPU_TRUTH), linux: Preview(GPU_TRUTH) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::Launch, windows: Preview(LAUNCH), macos: Preview(MAC_NATIVE_WALL), linux: Preview(LAUNCH) },
    Row { runtime: Runtime::NativeWindowless, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_UNRUN), linux: Preview(HTTP) },
    Row { runtime: Runtime::Script, capability: Capability::WallAndCounts, windows: Preview(NO_PAIRED), macos: Preview(MAC_PLANTED), linux: Preview(NO_PAIRED) },
    Row { runtime: Runtime::Script, capability: Capability::PageEngine, windows: Preview(SCRIPT_ENGINE), macos: Preview(SCRIPT_ENGINE), linux: Preview(SCRIPT_ENGINE) },
    Row { runtime: Runtime::Script, capability: Capability::ProcessCpu, windows: Preview(WIN_SCRIPT_CPU), macos: Preview(MAC_PLANTED), linux: Preview(LINUX_SCRIPT_CPU) },
    Row { runtime: Runtime::Script, capability: Capability::ProcessMemory, windows: Preview(SCRIPT_MEMORY), macos: Preview(MAC_SCRIPT_MEMORY), linux: Preview(SCRIPT_MEMORY) },
    Row { runtime: Runtime::Script, capability: Capability::HardwareCounters, windows: Refused(WIN_COUNTERS), macos: Preview(MAC_SCRIPT_COUNTERS), linux: Preview(LINUX_SCRIPT_COUNTERS) },
    Row { runtime: Runtime::Script, capability: Capability::PackageIdle, windows: Refused(NO_IDLE), macos: Preview(MAC_IDLE), linux: Refused(NO_IDLE) },
    Row { runtime: Runtime::Script, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_UNRUN), linux: Preview(HTTP) },
    Row { runtime: Runtime::Cli, capability: Capability::WallAndCounts, windows: Preview(CLI_WALL), macos: Preview(MAC_SIGN), linux: Preview(CLI_WALL) },
    Row { runtime: Runtime::Cli, capability: Capability::ProcessCpu, windows: Preview(WIN_CLI_CPU), macos: Preview(MAC_CLI_CPU), linux: Preview(LINUX_CLI_CPU) },
    Row { runtime: Runtime::Cli, capability: Capability::ProcessMemory, windows: Preview(WIN_CLI_MEMORY), macos: Preview(MAC_SIGN), linux: Preview(LINUX_CLI_MEMORY) },
    Row { runtime: Runtime::Cli, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_UNRUN), linux: Preview(HTTP) },
    Row { runtime: Runtime::Tty, capability: Capability::WallAndCounts, windows: Preview(TTY), macos: Preview(MAC_UNRUN), linux: Preview(TTY) },
    Row { runtime: Runtime::Tty, capability: Capability::ProcessCpu, windows: Preview(TTY), macos: Preview(MAC_UNRUN), linux: Preview(TTY) },
    Row { runtime: Runtime::Tty, capability: Capability::ProcessMemory, windows: Preview(TTY), macos: Preview(MAC_UNRUN), linux: Preview(TTY) },
    Row { runtime: Runtime::Tty, capability: Capability::TtyGrid, windows: Preview(TTY), macos: Preview(MAC_UNRUN), linux: Preview(TTY) },
    Row { runtime: Runtime::Tty, capability: Capability::Launch, windows: Preview(TTY), macos: Preview(MAC_UNRUN), linux: Preview(TTY) },
    Row { runtime: Runtime::Tty, capability: Capability::HttpProbe, windows: Preview(HTTP), macos: Preview(MAC_UNRUN), linux: Preview(HTTP) },
];

/// OS input (`fairness.input == "os"`: a native side or a page from outside driven through the OS):
/// what each side measured also depends on where the OS delivered strobe's input.
pub const OS_INPUT: Modifier = Modifier {
    windows: Preview("OS input reached headed pages and native apps in --check runs; no planted latency was recovered through it"),
    macos: Preview("OS input to headed Chromium arrived 9 of 9 times on an Apple silicon laptop; no planted latency was recovered through it"),
    linux: Preview("XTest input has run only under a virtual display; a real display's has not"),
};

/// The row of a runtime and capability; None: strobe measures no metric of that capability for that
/// kind of target.
pub fn row(runtime: Runtime, capability: Capability) -> Option<&'static Row> {
    SUPPORT_MATRIX.iter().find(|r| r.runtime == runtime && r.capability == capability)
}

/// A cell's support on a host: a refused cell has none; a `verified` one is `preview` off the OS's
/// primary architecture, where no evidence was taken.
fn tier(cell: &Cell, host: Host) -> Option<Support> {
    match cell {
        Cell::Refused(_) => None,
        Cell::Verified(_) if host.primary() => Some(Support::Verified),
        Cell::Verified(_) | Cell::Preview(_) => Some(Support::Preview),
    }
}

/// A runtime and capability's support on a host. None: strobe refuses the path there (on that OS, or
/// for that kind of target), and a refusal has no label.
pub fn support(runtime: Runtime, capability: Capability, host: Host) -> Option<Support> {
    tier(row(runtime, capability)?.on(host.os()), host)
}

/// OS input's support on a host.
pub fn os_input(host: Host) -> Option<Support> {
    tier(OS_INPUT.on(host.os()), host)
}

/// What a series runs on: one target, or an implementation comparison's two (SPEC §9.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    One(Runtime),
    Two { base: Runtime, head: Runtime },
}

/// What else a series' metric passed through (SPEC §21.1): strobe read the window's pixels in a run or
/// ended a launch on them (`driver.pixel_reads` over 0, `launch.until` on a window's paint or pixels),
/// or drove a side through the OS's input (`fairness.input == "os"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Through {
    pub pixels: bool,
    pub os_input: bool,
}

/// A path's support as result.json writes it (`series[].support`): its tier and what decided it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSupport {
    pub tier: Support,
    /// The runtime the tier is of: the only one, or in an implementation comparison the side whose
    /// path is the less checked (base when both are alike).
    pub runtime: Runtime,
    pub capability: Capability,
    pub host: Host,
    pub os_input: bool,
    pub pixels: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_runtime: Option<Runtime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_runtime: Option<Runtime>,
}

/// One side's tier: its runtime and capability, and its pixels when the series read them.
fn side(runtime: Runtime, capability: Capability, host: Host, through: Through) -> Option<Support> {
    let own = support(runtime, capability, host)?;
    if through.pixels { Some(own.min(support(runtime, Capability::Pixels, host)?)) } else { Some(own) }
}

/// The support of a metric's path: each side's runtime with the capability (and pixels when the series
/// read them), the least of the sides, and OS input's when a side was driven through it. None when any
/// part is refused there: a refused path has no label.
pub fn resolve(sides: Sides, capability: Capability, host: Host, through: Through) -> Option<PathSupport> {
    let (runtime, tier, base_runtime, head_runtime) = match sides {
        Sides::One(r) => (r, side(r, capability, host, through)?, None, None),
        Sides::Two { base, head } => {
            let (b, h) = (side(base, capability, host, through)?, side(head, capability, host, through)?);
            (if h < b { head } else { base }, b.min(h), Some(base), Some(head))
        }
    };
    let tier = if through.os_input { tier.min(os_input(host)?) } else { tier };
    Some(PathSupport { tier, runtime, capability, host, os_input: through.os_input, pixels: through.pixels, base_runtime, head_runtime })
}

/// A metric's path, by its name (`model::metric`); None for a name strobe does not know or a refused path.
pub fn resolve_metric(sides: Sides, metric: &str, host: Host, through: Through) -> Option<PathSupport> {
    resolve(sides, Capability::of(metric)?, host, through)
}
