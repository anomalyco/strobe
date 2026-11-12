// A native app with a window it keeps hidden, and one that declares none but shows one (dev only, macOS: window.swift
// beside this file; build it first: swiftc -O -o scripts/dev/tray/window scripts/dev/tray/window.swift, then set
// STROBE_DEV_WINDOW to that binary's absolute path for the `hidden-app` profile in strobe.json).
//   strobe bench scripts/dev/tray/window.bench.ts --head . --check
// Truth: idle · hidden and launch · hidden run (cpu.work; launch.ready at its `ready` line, milestone `up`); launch-window · hidden is
// refused NotForTarget (it cannot end on a window); idle · liar and launch · liar are refused VariantMismatch
// (window: false, yet a window on screen); idle · shown runs as any windowed app; launch-window · over (the
// `hidden-app` profile's window: false, overridden by the target's window: true) is timed to its window.
// No case stops the file.
import { bench, native } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.STROBE_DEV_WINDOW ?? join(import.meta.dir, "window")
const ready = { milestones: { up: { match: "ready" } }, until: { milestone: "up" } }

export default bench({
  targets: {
    hidden: native(bin, { args: ["hidden"], window: false }),
    liar: native(bin, { args: ["shown"], window: false }),
    shown: native(bin, { args: ["shown"] }),
    over: native("hidden-app", { args: ["shown"], window: true } as any),
  },
  cases: {
    idle: { targets: ["hidden", "liar", "shown"], primary: "cpu.work", run: ({ page }: any) => page.wait(500) },
    launch: { targets: ["hidden", "liar"], primary: "launch.ready", launch: ready },
    "launch-window": { targets: ["hidden", "over"], primary: "launch.ready", launch: { until: { window: "visible" } } },
  },
})
