// Planted differences for paired known-answer runs (dev only, not shipped): each case has two targets that differ
// only in a planted cost, so an A/B of them (`--base <a> --head <b>`) must recover that cost inside its interval,
// and an A/A of one (`--aa --target <a>`) must read same (±2%). Build known.rs first (see cli.bench.ts).
//
// Judging a run (labels-design §3.1). Tolerances: CPU and instructions ≤ 2%, a wall span ±1.5 ms per edge, exact
// counters exact, wakes ±10%, memory ≤ 2%, an energy ratio ±10%. A planted A/B passes when its interval covers the
// truth. strobe's intervals are of the ratio head / base - 1, so a fixed cost b0 a run carries besides the plant (a
// `ctx.call`, strobe's evaluate, a process's launch, the idle program's memory) moves the truth of a plant from A to
// B to (B + b0) / (A + b0) - 1; read b0 from the zero side (burst-0, idle) or the check, and give the absolute
// difference too. An interval of one value (an exact quantity) passes when it lies within the tolerance.
//
// Commands, from the repo root. Every OS:
//   strobe bench scripts/dev/known/planted.bench.ts --head . --check --headless --filter spin,page,launch,wakes,memory,pulse
//   strobe bench scripts/dev/known/planted.bench.ts --base script-a --head script-b --filter spin
//   strobe bench scripts/dev/known/planted.bench.ts --base page-a --head page-b --filter page --headless
//   strobe bench scripts/dev/known/planted.bench.ts --head . --aa --filter launch --target launch-200 --headless
//   strobe bench scripts/dev/known/planted.bench.ts --base launch-0 --head launch-200 --filter launch --headless
//   strobe bench scripts/dev/known/planted.bench.ts --base pulse-100 --head pulse-200 --filter pulse
//   strobe bench scripts/dev/known/planted.bench.ts --base hold-50 --head hold-100 --filter memory
// Headed (a display, the window in front):
//   strobe bench scripts/dev/known/planted.bench.ts --base frame-light --head frame-heavy --filter frames
//   strobe bench scripts/dev/known/planted.bench.ts --head . --aa --filter click --target click-150
//   strobe bench scripts/dev/known/planted.bench.ts --base click-0 --head click-150 --filter click
// Instruction counters (macOS, and Linux with perf events; Windows has none: NoCounter):
//   strobe bench scripts/dev/known/planted.bench.ts --base burst-1x --head burst-2x --filter counters
//   strobe bench scripts/dev/known/planted.bench.ts --base burst-1x --head burst-rate --filter counters
// Energy (Apple silicon only; elsewhere NoCounter):
//   strobe bench scripts/dev/known/planted.bench.ts --base burst-1x --head burst-rate --filter counters --primary energy.cpu
//
// The truth, per run (or launch):
//   spin      script-a spins 150 ms of the host's main thread, script-b 300 ms: cpu.work and cpu.main +150 ms (b0:
//             the `ctx.call`, about 0.7 ms of run.duration)
//   page      page-a spins 100 ms of the page's main thread, page-b 200 ms: main.work and cpu.work +100 ms (b0:
//             strobe's evaluate, about 0.2 ms of main.work and 0.6 ms of run.duration; the page's clock is
//             coarsened to 0.1 ms)
//   launch    #ready appears after a task that busy-waits 0 ms (launch-0) or 200 ms (launch-200): launch.ready
//             +200 ms (a 200 ms timer instead read +205.5 ms on a Mac: a browser's timer fires late by its slack).
//             An A/A of launch-0 (about 12 ms) cannot decide ±2%: run it on launch-200
//   frames    the page times the display's vsync V, then every animation frame spins 0.1 V (frame-light) or 1.5 V
//             (frame-heavy), headed only: frame-light presents every vsync (frames.interval.p50 V,
//             frames.dropped.pct 0); Chromium starts a late main frame at once rather than at the next vsync, so
//             frame-heavy presents 2 frames in every 3 vsyncs at any refresh rate: frames.interval.p95 2 V exactly
//             (light V: a planted +100%; its intervals are half V and half 2 V, so their median has no answer).
//             frames.dropped.pct is 1 - V / W for a frame of W, 33.3% at W = 1.5 V; each frame's own style and
//             paint add a little (33.6% on a 60 Hz Mac: W = 1.508 V), so it is within 1 pp above 33.3%
//   click     15 clicks a run, each answered in the next animation frame, which spins 0, 50 or 150 ms before the
//             button changes colour (click-0, click-50, click-150): interaction.p50 moves by the spin rounded up to
//             whole vsyncs, ceil(ms / V) × V (+50 and +150 ms exactly at 60, 120 and 240 Hz; +55.6 and +152.8 ms
//             at 144 Hz); headed only. A spin in the click handler itself read +42.8 ms [+38.7 … +47.3] for 50 ms
//             on a 60 Hz Mac (Chromium starts that late frame at once): no exact answer, so the spin is in a frame.
//             Vsync phase jitter is a few ms: an A/A decides ±2% on click-150 (about 184 ms), not on click-0
//   wakes     one thread waking 50 (tick50) or 100 (tick100) times a second on a fixed schedule, no work:
//             cpu.wakes 50/s and 100/s. cpu.idle_wakes (macOS) has no planted answer: it counts the whole chip's
//             exits from idle, which the other cores' work decides (0/s at 100 Hz, 8-36/s at 50 Hz on a Mac)
//   counters  bursts of a fixed count of loop iterations on a fixed schedule (`known burst`): burst-1x 250,000 a
//             burst 200 times a second, burst-2x 500,000, burst-0 none (the wakes' own cost, o), burst-rate
//             250,000 400 times a second. A run is 2 s, and a run's edges catch a whole burst or none: ±1 wake,
//             ±1 / (hz × 2 s), 0.25% at 200 Hz and 0.13% at 400 Hz. cpu.instructions: burst-2x over burst-1x is
//             (2c + o) / (c + o), c = burst-1x less burst-0; burst-rate over burst-1x is 2x within those bounds,
//             and so is energy.cpu within the clock's own drift (a count by time is no answer for instructions or
//             energy: a short burst runs at a lower clock)
//   memory    a windowless program holding 50 MiB (hold-50) or 100 MiB (hold-100) written once: memory.app.private
//             +50 MiB (b0: the idle program, about 1.2 MiB; the page tables of 50 MiB more add about 30 KiB)
//   pulse     a windowless program that prints BEGIN, busy-waits 100 ms (pulse-100) or 200 ms (pulse-200) of wall
//             time, prints END and sleeps 300 ms; a run waits for BEGIN and END: count.busy_us, the time between
//             when strobe read the two lines (`waitForLine`'s `at`, its host clock), 100,000 and 200,000 µs
//             (±1.5 ms per edge), +100 ms. phase.busy around the END wait is coarser: the runner polls a line wait
//             every 10 ms (later on macOS), so each phase edge is late by up to a poll. A `page.wait` is no wall
//             answer on macOS (a sleep wakes 1.5-10 ms late there); this busy-wait is
import { bench, native, script, url } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
const page = (query: string) => url(new URL(`../pages/planted.html?${query}`, import.meta.url).href)
// Headed: a viewport that fits a laptop's screen beside its menu bar and Dock.
const shown = (query: string) => url(new URL(`../pages/planted.html?${query}`, import.meta.url).href, { viewport: { width: 800, height: 600 } })
const tick = (hz: number) => native(bin, { args: ["tick", String(hz), "0", "600"], window: false })
const burst = (hz: number, kiters: number) => native(bin, { args: ["burst", String(hz), String(kiters), "600"], window: false })
const hold = (mb: number) => native(bin, { args: ["hold", String(mb), "600"], window: false })
const pulse = (ms: number) => native(bin, { args: ["pulse", String(ms), "300", "600"], window: false, output: true })
const click = (ms: number) => shown(`click=${ms}`)

export default bench({
  targets: {
    "script-a": script("./harness.ts"),
    "script-b": script("./harness.ts"),
    "page-a": page("p=a"),
    "page-b": page("p=b"),
    "launch-0": page("delay=0"),
    "launch-200": page("delay=200"),
    "frame-light": shown("vsyncs=0.1"),
    "frame-heavy": shown("vsyncs=1.5"),
    "click-0": click(0),
    "click-50": click(50),
    "click-150": click(150),
    tick50: tick(50),
    tick100: tick(100),
    "burst-0": burst(200, 0),
    "burst-1x": burst(200, 250),
    "burst-2x": burst(200, 500),
    "burst-rate": burst(400, 250),
    "hold-50": hold(50),
    "hold-100": hold(100),
    "pulse-100": pulse(100),
    "pulse-200": pulse(200),
  },
  cases: {
    spin: {
      targets: ["script-a", "script-b"],
      primary: "cpu.work",
      metrics: ["cpu.main", "run.duration"],
      run: ({ call, target }: any) => call("spin", target === "script-b" ? 300 : 150),
    },
    page: {
      targets: ["page-a", "page-b"],
      primary: "cpu.work",
      metrics: ["main.work", "main.script", "run.duration", "cpu.renderer"],
      run: ({ page, target }: any) => page.eval((ms: number) => (window as any).known.spin(ms), target === "page-b" ? 200 : 100),
    },
    launch: {
      targets: ["launch-0", "launch-200"],
      primary: "launch.ready",
      launch: { until: "#ready" },
    },
    frames: {
      targets: ["frame-light", "frame-heavy"],
      primary: "frames.interval.p95",
      metrics: ["frames.dropped.pct", "frames.interval.p50", "frames.dropped"],
      // 2 s: a block of 5 runs has the 200 frame gaps a p95 needs at 40 frames a second.
      run: ({ page }: any) => page.wait(2000),
    },
    click: {
      targets: ["click-0", "click-50", "click-150"],
      primary: "interaction.p50",
      metrics: ["interaction.p95"],
      async run({ page }: any) {
        for (let i = 0; i < 15; i++) {
          await page.click("#btn")
          await page.wait(250)
        }
      },
    },
    wakes: {
      targets: ["tick50", "tick100"],
      primary: "cpu.wakes",
      metrics: ["cpu.work"],
      run: ({ page }: any) => page.wait(1000),
    },
    counters: {
      targets: ["burst-0", "burst-1x", "burst-2x", "burst-rate"],
      primary: "cpu.instructions",
      metrics: ["cpu.wakes", "cpu.work"],
      run: ({ page }: any) => page.wait(2000),
    },
    memory: {
      targets: ["hold-50", "hold-100"],
      primary: "memory.app.private",
      metrics: ["cpu.work"],
      run: ({ page }: any) => page.wait(1000),
    },
    pulse: {
      targets: ["pulse-100", "pulse-200"],
      primary: "count.busy_us",
      metrics: ["phase.busy", "run.duration"],
      async run({ waitForLine, phase, count }: any) {
        const begin = await waitForLine("BEGIN")
        const end = await phase("busy", () => waitForLine("END"))
        // When strobe read each line (ns, its host clock): the program's busy-wait between them.
        count("busy_us", Math.round((end.at - begin.at) / 1000))
      },
    },
  },
})
