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
// Truth: idle and launch run (cpu.work; launch.ready about 200 ms past the page's start, planted.html?delay=200);
// launch-window and launch-painted refuse before anything launches ("the app shows no window"); frames and
// interaction metrics refuse NotForTarget (nothing it draws reaches a display).
import { bench, exe, native } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, "..", "known", process.platform === "win32" ? "known.exe" : "known")
const page = new URL("../pages/planted.html?delay=200", import.meta.url).href
const chrome = process.env.STROBE_DEV_TRAY_EXE ?? ""

export default bench({
  targets: {
    tray: exe(chrome, { app: "tray", args: [page] } as any),
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
