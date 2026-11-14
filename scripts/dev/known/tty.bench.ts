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
// Each must overrun in every run, by more than a timer tick either way: checked from known.rs's own log of when each
// key arrived and each answer was written (`log=<file>`; 10 runs of 10 keys, 0.32.0, a Windows desktop and a Linux VM).
// "Margin" is how long before the last answer was written the next key arrived (or, negative, how long after).
//   overrun-60   (Unix) 10 keys back to back on keys-60 (each answered 60 ms later): strobe sends the next key once
//                the grid has been still 50 ms, so it arrives 10 ms before the owed answer: 90 of 90 gaps, margin
//                9.8 to 10.4 ms on Linux, whose timers are exact. Not on Windows, where strobe's 50 ms wait runs to
//                about 58 ms (its timer's tick) and the margin would be 2 ms. A 50 ms answer is no case on either:
//                it overran 45 of 90 gaps on Linux (margin 0.0 to 0.8 ms) and 3 of 90 on Windows
//   overrun-120  10 keys back to back on keys-120: 54 of 54 gaps, margin 67 ms or more on Windows and 70 ms or more
//                on Linux (answers queue up, so later keys overrun by more). keys-80 back to back overran every gap
//                too, but by 13.9 to 30 ms on Windows: under a 15.6 ms tick at its low end
//   overrun-mixed  10 keys 60 ms apart on keys-mixed, answered 20 and 120 ms later in turn: each 120 ms answer comes
//                59 to 60 ms after the next key, each 20 ms answer 50 to 78 ms before it, in every run on both: no key
//                is safely paired (reading the fast ones alone would hide the slow ones)
// What 0.32.0 reads (`--check`):
//   overrun-60   Linux 10.0 ms (90 inputs, 10 silent); a 50 ms answer read 0.17 to 0.20 ms there
//   overrun-120  Windows 6.7 ms (57 inputs, 43 silent) and 55.8 ms (19, 11) in two checks, Linux 20.0 ms (59, 41)
//   overrun-mixed  Windows 59.8 ms (17 inputs, 13 silent), Linux 59.5 ms (18, 12)
//   echo-cooked  (Unix) 10 keys 100 ms apart to a line-mode program: the terminal echoes each key at once, and the
//                program answers only a whole line, so what changes the grid is the kernel's echo, not the
//                program: Linux 0.07 ms (30 inputs, tty.frames 0, tty.cells 10: the echoed keys)
//   paste        has no `paste` option on 0.32.0, which takes it without a word and types the 22 characters one
//                at a time: 22 samples a run, p50 20.8 ms (Windows) and 20.2 ms (Linux); it must be 1 sample a run
// Acceptance for the latency fix (on a Windows desktop and a Linux VM; macOS too where it runs):
//   refused     overrun-120 and overrun-mixed on both, overrun-60 and echo-cooked on Unix: AmbiguousInteraction for
//               every tty.latency.* and tty.settle.*, no latency value printed, in --check and exit 4 in a timed run
//               (`--aa --filter overrun-120 --target keys-120`, likewise each)
//   keys        tty.latency.p50 20.0 to 23.5 ms, tty.frames 10, tty.cells 400; an 8-pair A/A same (±2%)
//   spaced      --base keys --head keys-80: judge the paired tty.latency.p50 shift against +60 ms (±3 ms at the
//               two PTY edges), not a ratio from one launch. An 8-pair A/A on keys must read same (±2%).
//   paste       1 latency sample a run, tty.latency.p50 20.0 to 24.0 ms
//   ready       --base ready-200 --head ready-500: launch.ready +300 ms ± 3 ms; an 8-pair A/A on ready-500 same (±2%)
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
    ...(unix ? { "keys-60": term(["tty", "40", "60"]) } : {}),
    "keys-80": term(["tty", "40", "80"]),
    "keys-120": term(["tty", "40", "120"]),
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
    ...(unix ? { "overrun-60": { targets: ["keys-60"], primary: "tty.latency.p50", metrics: latency, resetCheck: false, run: keys(10) } } : {}),
    "overrun-120": { targets: ["keys-120"], primary: "tty.latency.p50", metrics: latency, resetCheck: false, run: keys(10) },
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
