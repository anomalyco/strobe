// Known-answer bench for tty() targets (dev only, not shipped): known.rs's terminal modes draw what their arguments
// say. Build known.rs first (see cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/tty.bench.ts --head . --check
// Truth on Unix (Windows' tty.bytes is the console host's re-render, not the program's bytes):
//   keys      per run 10 keys, each answered 20 ms later by one synchronized frame of 62 bytes rewriting 40 cells:
//             tty.latency.p50 just over 20 ms, tty.updates 10, tty.frames 10, tty.bytes 620, tty.cells 400
//   frames    launch until `done`: 10 frames 50 ms apart (launch.ready just over 500 ms plus the start), tty.frames 10,
//             tty.bytes 15 + 10 × 62 + 10 = 645
//   ready-0 / ready-300  launch until READY: the two launch.ready differ by 300 ms
import { bench, tty } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
const io = ["cpu.work", "cpu.app.total", "cpu.wakes", "memory.app.private", "io.read", "io.write"]

export default bench({
  targets: {
    keys: tty([bin, "tty", "40", "20"], { cols: 80, rows: 24 }),
    frames: tty([bin, "frames", "10", "40", "50"], { cols: 80, rows: 24 }),
    "ready-0": tty([bin, "ready", "0", "READY"], { cols: 80, rows: 24 }),
    "ready-300": tty([bin, "ready", "300", "READY"], { cols: 80, rows: 24 }),
  },
  // Every run case (launch cases run no setup) starts once the program says it is ready.
  setup: ({ page }: any) => page.waitFor("text=ready"),
  cases: {
    keys: {
      targets: ["keys"],
      primary: "tty.latency.p50",
      metrics: ["tty.updates", "tty.frames", "tty.bytes", "tty.cells", "tty.settle.p50", "tty.frame.p50", "run.duration", ...io],
      resetCheck: false,
      run: async ({ page }: any) => {
        for (let i = 0; i < 10; i++) await page.press("x")
      },
    },
    // One key, answered by one 62-byte frame of 40 cells 20 ms later: tty.updates 1, tty.bytes 62, tty.cells 40.
    "one-key": {
      targets: ["keys"],
      primary: "tty.latency.p50",
      metrics: ["tty.updates", "tty.frames", "tty.bytes", "tty.cells", "cpu.work"],
      resetCheck: false,
      run: ({ page }: any) => page.press("x"),
    },
    // The same key waited for (until: "frame"): the frame lands inside the run.
    "one-key-until": {
      targets: ["keys"],
      primary: "tty.latency.p50",
      metrics: ["tty.updates", "tty.frames", "tty.bytes", "tty.cells", "cpu.work"],
      resetCheck: false,
      run: ({ page }: any) => page.press("x", { until: "frame" }),
    },
    // Five keys timed, then five inside ctx.untimed: only the timed five count (tty.updates 5, tty.bytes 310,
    // io.read 5 B, io.write 310 B on Unix).
    "keys-untimed": {
      targets: ["keys"],
      primary: "tty.latency.p50",
      metrics: ["tty.updates", "tty.bytes", "io.read", "io.write", "cpu.wakes"],
      resetCheck: false,
      run: async ({ page, untimed }: any) => {
        for (let i = 0; i < 5; i++) await page.press("x", { until: "frame" })
        await untimed(async () => {
          for (let i = 0; i < 5; i++) await page.press("x", { until: "frame" })
        })
      },
    },
    frames: {
      targets: ["frames"],
      primary: "launch.ready",
      metrics: ["tty.updates", "tty.frames", "tty.bytes", "cpu.work", "memory.app.private"],
      launch: { until: "text=done" },
    },
    ready: {
      targets: ["ready-0", "ready-300"],
      primary: "launch.ready",
      metrics: ["tty.updates", "tty.bytes", "cpu.work"],
      launch: { until: "text=READY" },
    },
  },
})
