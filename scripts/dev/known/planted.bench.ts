// Planted differences for paired known-answer runs (dev only, not shipped): each case has two targets that differ
// only in a planted cost, so an A/B of them (`--base <a> --head <b>`) must recover that cost inside its interval,
// and an A/A of one (`--aa --target <a>`) must read same. Build known.rs first (see cli.bench.ts), then from the
// repo root, for example:
//   strobe bench scripts/dev/known/planted.bench.ts --head . --check --headless --filter spin,page,launch,wakes,counters,memory
//   strobe bench scripts/dev/known/planted.bench.ts --base script-a --head script-b --filter spin
//   strobe bench scripts/dev/known/planted.bench.ts --base page-a --head page-b --filter page --headless
//   strobe bench scripts/dev/known/planted.bench.ts --head . --aa --filter launch --target launch-200 --headless
//   strobe bench scripts/dev/known/planted.bench.ts --base burst-1x --head burst-rate --filter counters --primary energy.cpu
//   strobe bench scripts/dev/known/planted.bench.ts --base frame-2 --head frame-25 --filter frames      (headed)
//   strobe bench scripts/dev/known/planted.bench.ts --base click-0 --head click-50 --filter click       (headed)
// The truth, per run (or launch):
//   spin      script-a spins 150 ms of the host's main thread, script-b 300 ms: cpu.work and cpu.main +150 ms
//   page      page-a spins 100 ms of the page's main thread, page-b 200 ms: main.work and cpu.work +100 ms
//   launch    #ready appears after a task that busy-waits 0 ms (launch-0) or 200 ms (launch-200): launch.ready
//             +200 ms (a 200 ms timer instead read +205.5 ms on air: a browser's timer fires late by its slack). An
//             A/A of launch-0 (12 ms) cannot decide ±2%: run it on launch-200
//   frames    every animation frame spins 2 ms (frame-2) or 25 ms (frame-25), on a 60 Hz display, headed only:
//             frame-2 presents every vsync (frames.interval.p50 16.7 ms, frames.dropped.pct 0). Chromium starts a
//             late main frame at once rather than at the next vsync, so frame-25 makes 40 frames a second, not 30:
//             frames.dropped.pct 1 - 40/60 = 33.3% (a little more for each frame's own style and paint), and
//             its intervals half 16.7 and half 33.3 ms (no answer for their median)
//   click     20 clicks a run, each answered in the next animation frame, which spins 0 ms (click-0) or 50 ms
//             (click-50) before it changes the button's colour: interaction.p50 +50 ms (3 vsyncs at 60 Hz, so
//             the frame it shows in moves by exactly 3); headed only. A spin in the click handler instead read
//             +42.8 ms [+38.7 … +47.3] on air, likely because Chromium then starts the late frame at once rather
//             than at the next vsync (as frame-25 shows), so that plant has no exact answer
//   wakes     one thread waking 50 (tick50) or 100 (tick100) times a second on a fixed schedule, no work:
//             cpu.wakes 50/s and 100/s. cpu.idle_wakes (macOS) has no planted answer: the OS's own count read
//             0/s at 100 Hz and 8-36/s at 50 Hz on air (whether the package was idle at each wake is its call)
//   counters  bursts of a fixed count of loop iterations, 50 a second (`known burst`): burst-1x 1,000,000 a burst,
//             burst-2x 2,000,000, burst-0 none (the wakes' own cost, o), burst-rate 1,000,000 100 times a second.
//             cpu.instructions: burst-2x over burst-1x is (2c + o) / (c + o), c = burst-1x less burst-0; burst-rate
//             over burst-1x is 2x exactly (the same bursts and wakes twice as often), and so is energy.cpu
//             within the clock's own drift (a count by time is no answer here: a short burst runs at a lower clock)
//   memory    a windowless program holding 50 MiB (hold-50) or 100 MiB (hold-100) written once: memory.app.private
//             +50 MiB
import { bench, native, script, url } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
const page = (query: string) => url(new URL(`../pages/planted.html?${query}`, import.meta.url).href)
// Headed: a viewport that fits a laptop's screen beside its menu bar and Dock.
const shown = (query: string) => url(new URL(`../pages/planted.html?${query}`, import.meta.url).href, { viewport: { width: 800, height: 600 } })
const tick = (hz: number) => native(bin, { args: ["tick", String(hz), "0", "600"], window: false })
const burst = (hz: number, kiters: number) => native(bin, { args: ["burst", String(hz), String(kiters), "600"], window: false })
const hold = (mb: number) => native(bin, { args: ["hold", String(mb), "600"], window: false })

export default bench({
  targets: {
    "script-a": script("./harness.ts"),
    "script-b": script("./harness.ts"),
    "page-a": page("p=a"),
    "page-b": page("p=b"),
    "launch-0": page("delay=0"),
    "launch-200": page("delay=200"),
    "frame-2": shown("frame=2"),
    "frame-25": shown("frame=25"),
    "click-0": shown("click=0"),
    "click-50": shown("click=50"),
    tick50: tick(50),
    tick100: tick(100),
    "burst-0": burst(50, 0),
    "burst-1x": burst(50, 1000),
    "burst-2x": burst(50, 2000),
    "burst-rate": burst(100, 1000),
    "hold-50": hold(50),
    "hold-100": hold(100),
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
      targets: ["frame-2", "frame-25"],
      primary: "frames.dropped.pct",
      metrics: ["frames.interval.p50", "frames.dropped", "frames.presented.p95"],
      run: ({ page }: any) => page.wait(1000),
    },
    click: {
      targets: ["click-0", "click-50"],
      primary: "interaction.p50",
      metrics: ["interaction.p95"],
      async run({ page }: any) {
        for (let i = 0; i < 20; i++) {
          await page.click("#btn")
          await page.wait(120)
        }
      },
    },
    wakes: {
      targets: ["tick50", "tick100"],
      primary: "cpu.wakes",
      metrics: ["cpu.idle_wakes", "cpu.work"],
      run: ({ page }: any) => page.wait(1000),
    },
    counters: {
      targets: ["burst-0", "burst-1x", "burst-2x", "burst-rate"],
      primary: "cpu.instructions",
      metrics: ["energy.cpu", "cpu.idle_wakes", "cpu.wakes", "cpu.work"],
      run: ({ page }: any) => page.wait(1000),
    },
    memory: {
      targets: ["hold-50", "hold-100"],
      primary: "memory.app.private",
      metrics: ["cpu.work"],
      run: ({ page }: any) => page.wait(1000),
    },
  },
})
