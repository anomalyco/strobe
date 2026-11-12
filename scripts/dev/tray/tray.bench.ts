// A tray-style app on every OS (dev only, not shipped): a headless Chromium launched as an app with `window: false`
// (profile `tray` in strobe.json beside this file), whose only window is hidden, beside a windowless native target.
// Point STROBE_DEV_TRAY_EXE at a Chromium executable (Windows: chrome.exe of strobe's own browser, `strobe status`
// names it; Linux: /usr/bin/chromium or that one; macOS: a copy with its own bundle id, since strobe refuses to launch
// one whose preferences exist in your ~/Library: copy the .app, set CFBundleIdentifier, `codesign --force --deep -s -`).
// Build known.rs first (see known/cli.bench.ts). From the repo root:
//   strobe bench scripts/dev/tray/tray.bench.ts --check --base tray --head nat --filter idle
//   strobe bench scripts/dev/tray/tray.bench.ts --head . --check --filter launch
//   strobe bench scripts/dev/tray/tray.bench.ts --head . --check --filter launch-window     (a usage error)
//   strobe bench scripts/dev/tray/tray.bench.ts --head . --check --filter launch-painted    (a usage error)
//   strobe bench scripts/dev/tray/tray.bench.ts --head . --check --target tray --filter frames
// Truth: idle and launch run (cpu.work; launch.ready 1.2–1.3 s: the browser's start, the 800 ms the page is served
// late, then its 200 ms task);
// launch-window and launch-painted refuse before anything launches ("the app shows no window"). On Windows and Linux,
// idle · tray and frames fail "<exe> shows no window" at the bench's window fit before any run; on macOS they run and
// frames.* reads "no value". Once admission skips the fit, they should run everywhere and frames.* refuse NotForTarget.
import { bench, exe, native } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, "..", "known", process.platform === "win32" ? "known.exe" : "known")
// The page is served 800 ms late, so it is still loading when strobe attaches (a file: page has already shown #ready
// by then on macOS, which refuses the launch RunIncomplete).
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: async () => (await Bun.sleep(800), new Response(Bun.file(join(import.meta.dir, "..", "pages", "planted.html")), { headers: { "content-type": "text/html" } })),
})
const page = `http://127.0.0.1:${server.port}/planted.html?delay=200`
const chrome = process.env.STROBE_DEV_TRAY_EXE ?? ""

export default bench({
  targets: {
    // Chromium exits at once when it runs as root without --no-sandbox (a Linux VM's usual user).
    tray: exe(chrome, { app: "tray", args: [page, ...(process.getuid?.() === 0 ? ["--no-sandbox"] : [])] } as any),
    nat: native(bin, { args: ["tick", "50", "0", "600"], window: false }),
  },
  cases: {
    idle: { primary: "cpu.work", run: ({ page }: any) => page.wait(500) },
    frames: { targets: ["tray"], primary: "cpu.work", metrics: ["frames.dropped.pct", "interaction.p50"], run: ({ page }: any) => page.wait(500) },
    launch: { targets: ["tray"], primary: "launch.ready", launch: { until: "#ready" } },
    "launch-window": { targets: ["tray"], primary: "launch.ready", launch: { until: { window: "visible" } } },
    "launch-painted": { targets: ["tray"], primary: "launch.ready", metrics: ["launch.painted"], launch: { until: "#ready" } },
  },
})
