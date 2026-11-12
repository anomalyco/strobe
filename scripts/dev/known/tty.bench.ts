// Known-answer bench for tty() targets (dev only, not shipped): known.rs's terminal modes draw what their arguments
// say. Build known.rs first (see cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/tty.bench.ts --head . --check
//   strobe bench scripts/dev/known/tty.bench.ts --base keys --head keys-80 --filter spaced
//   strobe bench scripts/dev/known/tty.bench.ts --head . --aa --filter ready --target ready-500
//   strobe bench scripts/dev/known/tty.bench.ts --base ready-200 --head ready-500 --filter ready
// Truth on Unix (Windows' tty.bytes is the console host's re-render, not the program's bytes):
//   keys      per run 10 keys, each answered 20 ms later by one synchronized frame of 62 bytes rewriting 40 cells:
//             tty.latency.p50 just over 20 ms, tty.updates 10, tty.frames 10, tty.bytes 620, tty.cells 400
//   spaced    the same keys 200 ms apart, on keys and keys-80 (each key answered 80 ms later): tty.latency.p50
//             +60 ms (--base keys --head keys-80 --filter spaced)
//   paste     two lines pasted at once (`type` with `paste: true`) into a program that turned on bracketed paste
//             (ESC [?2004h) and answers a whole paste once, 20 ms after its end: one latency sample a run,
//             tty.latency.p50 just over 20 ms
//   frames    launch until `done`: 10 frames 50 ms apart (launch.ready just over 500 ms plus the start), tty.frames 10,
//             tty.bytes 15 + 10 × 62 + 10 = 645
//   ready-200 / ready-500  launch until READY after a busy-wait of 200 or 500 ms (no timer slack): launch.ready
//             +300 ms; the A/A is on ready-500, where ±2% is ±10 ms
// Input faster than the answers has no latency to give: every case below must be refused AmbiguousInteraction (no
// tty.latency.* or tty.settle.* value, exit 4 in a timed comparison). strobe through 0.32.0 measures them instead,
// and wrongly: a key's late answer is taken for the next key's, or the terminal's own echo for the program's answer.
// What 0.32.0 reads (`--check --runs 3`, a Windows desktop and a Linux VM):
//   overrun-50   10 keys back to back on keys-50 (each answered 50 ms later, strobe's 50 ms rest before a key):
//                Windows 50.8 ms (28 inputs, 2 silent), Linux 0.17 ms (--check and a timed A/B)
//   overrun-80   10 keys back to back on keys-80: Windows 22.2 to 24.7 ms (27 inputs, 3 silent), Linux 30.0 ms
//   overrun-mixed  10 keys 60 ms apart on keys-mixed, answered 20 and 120 ms later in turn: the 20 ms answers come
//                before the next key, the 120 ms ones after it, so no key is safely paired (reading the fast ones
//                alone would hide the slow ones): Windows 59.8 ms (17 inputs, 13 silent), Linux 59.5 ms (18, 12)
//   echo-cooked  (Unix) 10 keys 100 ms apart to a line-mode program: the terminal echoes each key at once, and the
//                program answers only a whole line, so what changes the grid is the kernel's echo, not the
//                program: Linux 0.07 ms (30 inputs, tty.frames 0, tty.cells 10: the echoed keys)
//   paste        has no `paste` option on 0.32.0, which takes it without a word and types the 22 characters one
//                at a time: 22 samples a run, p50 20.8 ms (Windows) and 20.2 ms (Linux); it must be 1 sample a run
import { bench, tty } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
const io = ["cpu.work", "cpu.app.total", "cpu.wakes", "memory.app.private", "io.read", "io.write"]
const term = (args: string[]) => tty([bin, ...args], { cols: 80, rows: 24 })
const latency = ["tty.latency.p95", "tty.latency.silent", "tty.latency.busy", "tty.updates", "tty.frames", "tty.cells"]
const keys = (n: number, gap = 0) => async ({ page }: any) => {
  for (let i = 0; i < n; i++) {
    await page.press("x")
    if (gap) await page.wait(gap)
  }
}
const unix = process.platform !== "win32"

export default bench({
  targets: {
    keys: term(["tty", "40", "20"]),
    "keys-50": term(["tty", "40", "50"]),
    "keys-80": term(["tty", "40", "80"]),
    "keys-mixed": term(["tty", "40", "20,120"]),
    paste: term(["tty", "40", "20", "paste"]),
    ...(unix ? { cooked: term(["cooked", "40", "20"]) } : {}),
    frames: term(["frames", "10", "40", "50"]),
    "ready-200": term(["ready", "200", "READY"]),
    "ready-500": term(["ready", "500", "READY"]),
  },
  // Every run case (launch cases run no setup) starts once the program says it is ready.
  setup: ({ page }: any) => page.waitFor("text=ready"),
  cases: {
    keys: {
      targets: ["keys"],
      primary: "tty.latency.p50",
      metrics: ["tty.updates", "tty.frames", "tty.bytes", "tty.cells", "tty.settle.p50", "tty.frame.p50", "run.duration", ...io],
      resetCheck: false,
      run: keys(10),
    },
    spaced: { targets: ["keys", "keys-80"], primary: "tty.latency.p50", metrics: latency, resetCheck: false, run: keys(10, 200) },
    paste: {
      targets: ["paste"],
      primary: "tty.latency.p50",
      metrics: latency,
      resetCheck: false,
      run: ({ page }: any) => page.type("", "first line\nsecond line", { paste: true }),
    },
    "overrun-50": { targets: ["keys-50"], primary: "tty.latency.p50", metrics: latency, resetCheck: false, run: keys(10) },
    "overrun-80": { targets: ["keys-80"], primary: "tty.latency.p50", metrics: latency, resetCheck: false, run: keys(10) },
    "overrun-mixed": { targets: ["keys-mixed"], primary: "tty.latency.p50", metrics: latency, resetCheck: false, run: keys(10, 60) },
    ...(unix ? { "echo-cooked": { targets: ["cooked"], primary: "tty.latency.p50", metrics: latency, resetCheck: false, run: keys(10, 100) } } : {}),
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
      targets: ["ready-200", "ready-500"],
      primary: "launch.ready",
      metrics: ["tty.updates", "tty.bytes", "cpu.work"],
      launch: { until: "text=READY" },
    },
  },
})
