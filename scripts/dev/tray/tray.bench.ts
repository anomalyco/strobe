// Windowed and windowless apps in one file, on every OS (dev only, not shipped). Profiles in strobe.json beside it:
// `tray` (a headless Chromium with `window: false`, whose only window is hidden), `desk` (the same Chromium with a
// window), `liar` (desk's window, declared `window: false`), `stub` (electron/main.js: a BrowserWindow kept hidden,
// `show: false`, declared `window: false`; STROBE_DEV_SHOW=1 shows it).
// Set STROBE_DEV_TRAY_EXE to a Chromium executable (Windows: chrome.exe of strobe's own browser, `strobe status`
// names it; Linux: /usr/bin/chromium or that one; macOS: a copy with its own bundle id, since strobe refuses to launch
// one whose preferences exist in your ~/Library: copy the .app, set CFBundleIdentifier, `codesign --force --deep -s -`),
// and STROBE_DEV_ELECTRON to an Electron executable (optional: the Electron targets are left out without it; on macOS
// a re-signed copy too). The windowed targets need a display (left out on Linux without one). Build known.rs first
// (see known/cli.bench.ts). From the repo root, the whole file, then one exe target run as a profile named on the
// command line:
//   strobe bench scripts/dev/tray/tray.bench.ts --head . --check
//   strobe bench scripts/dev/tray/tray.bench.ts --head app:tray --check --target plain
// Truth, per case and target:
//   idle · tray, idle · nat: run (cpu.work). idle · tray fails "<exe> shows no window" on Windows and Linux at the
//     bench's window fit (bench.rs, before W8-1 lands); on macOS it runs.
//   frames · tray: frames.* and interaction.* no value or refused (nothing it draws reaches a display).
//   launch · tray, stub: launch.ready about 1.2–1.3 s (the browser's start, the page served 800 ms late, its 200 ms
//     task), with no window read or recorded. · desk: the same, with its window.
//   hidden-page · stub-hid (shown, then hidden at once: its page reports visibilityState hidden): launch.ready and
//     launch.hidden measured, never refused WindowHidden. · stub (never shown): its page may report visible, and then
//     launch.hidden never matches (RunIncomplete).
//   run-hidden · stub-hid: a run case of that page; the bench's own blocks still refuse it WindowHidden (bench.rs,
//     before W8-1 lands), where it should run.
//   launch-window · tray: refused NotForTarget ("cannot end on one"); · desk: launch.ready at its window.
//   launch-painted · tray: launch.ready measured, launch.painted refused NotForTarget; · desk: launch.painted read
//     (or NoPresentSource without the screen's pixels, SoftwareGpu on a software adapter: Xvfb). tray's refusal is
//     SoftwareGpu there too, until bench.rs keeps a windowless app's NotForTarget (W8-1).
//   painted-primary · tray: refused NotForTarget before it launches (a primary of its pixels).
//   liar · liar, stub-shown: declare window: false and show a window: refused VariantMismatch.
//   plain (a bare exe, no profile; windowed): launch-window measured; with --head app:tray, refused NotForTarget.
// No case stops the file: every other series still runs.
import { bench, exe, native } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, "..", "known", process.platform === "win32" ? "known.exe" : "known")
// The page is served 800 ms late, so it is still loading when strobe attaches (a file: page has already shown #ready
// by then on macOS, which refuses the launch RunIncomplete: STROBE-ISSUES).
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: async () => (await Bun.sleep(800), new Response(Bun.file(join(import.meta.dir, "..", "pages", "planted.html")), { headers: { "content-type": "text/html" } })),
})
const page = `http://127.0.0.1:${server.port}/planted.html?delay=200`
const chrome = process.env.STROBE_DEV_TRAY_EXE ?? ""
const electron = process.env.STROBE_DEV_ELECTRON
// Chromium exits at once when it runs as root without --no-sandbox (a Linux VM's usual user).
const root = process.getuid?.() === 0 ? ["--no-sandbox"] : []
const display = process.platform !== "linux" || !!process.env.DISPLAY || !!process.env.WAYLAND_DISPLAY
const app = (profile: string) => exe(chrome, { app: profile, args: [page, ...root] })
const stub = (env: Record<string, string> = {}) => exe(electron ?? "", { app: "stub", args: [join(import.meta.dir, "electron"), page, ...root], env })

const desk = display ? ["desk"] : []
const liars = [...(display ? ["liar"] : []), ...(electron ? ["stub-shown"] : [])]
export default bench({
  targets: {
    tray: app("tray"),
    ...(display ? { desk: app("desk"), liar: app("liar"), plain: exe(chrome, { args: ["--no-first-run", "--no-default-browser-check", page, ...root] }) } : {}),
    ...(electron ? { stub: stub(), "stub-hid": stub({ STROBE_DEV_HIDE: "1" }), "stub-shown": stub({ STROBE_DEV_SHOW: "1" }) } : {}),
    nat: native(bin, { args: ["tick", "50", "0", "600"], window: false }),
  },
  cases: {
    idle: { targets: ["tray", "nat"], primary: "cpu.work", run: ({ page }: any) => page.wait(500) },
    frames: { targets: ["tray"], primary: "cpu.work", metrics: ["frames.dropped.pct", "interaction.p50"], run: ({ page }: any) => page.wait(500) },
    launch: { targets: ["tray", ...desk, ...(electron ? ["stub"] : [])], primary: "launch.ready", launch: { until: "#ready" } },
    "launch-window": { targets: ["tray", ...desk, ...(display ? ["plain"] : [])], primary: "launch.ready", launch: { until: { window: "visible" } } },
    "launch-painted": { targets: ["tray", ...desk], primary: "launch.ready", metrics: ["launch.painted"], launch: { until: "#ready" } },
    "painted-primary": { targets: ["tray"], primary: "launch.painted", launch: { until: "#ready" } },
    // Which stub's page says visibilityState hidden: launch.hidden is its line (a missing one: RunIncomplete).
    // run-hidden: a run case of that hidden page (bench.rs's blocks, W8-1).
    ...(electron
      ? {
          "hidden-page": { targets: ["stub", "stub-hid"], primary: "launch.ready", metrics: ["launch.hidden"], launch: { until: "#ready", milestones: { hidden: { match: "page hidden" } } } },
          "run-hidden": { targets: ["stub-hid"], primary: "cpu.work", run: ({ page }: any) => page.wait(500) },
        }
      : {}),
    ...(liars.length ? { liar: { targets: liars, primary: "launch.ready", launch: { until: "#ready" } } } : {}),
  },
})
