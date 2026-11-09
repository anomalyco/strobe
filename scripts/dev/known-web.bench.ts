// Known-answer bench for pages (not shipped): every case does work whose true cost is known by
// construction, so each number strobe prints can be checked against it. Run in each engine, headless:
//   strobe bench scripts/dev/known-web.bench.ts --head . --check --headless [--engine firefox] [--why always]
//   strobe bench scripts/dev/known-web.bench.ts --head . --blocks 2 --headless [--engine firefox]
//   strobe bench scripts/dev/known-web.bench.ts --head . --probes memory --repeat 6 --headless --filter leak
// The truth, per run:
//   spin100       one 100 ms task: main.work ≈ 100 ms, main.tasks.long 1, cpu.renderer ≈ 100 ms of the run's wall time
//   spin70        one 70 ms task: main.tasks.long 1
//   click50       a click whose listener spins 50 ms: main.script ≈ 50 ms (spin100's script is strobe's evaluate)
//   tasks3x30     three 30 ms tasks: main.work ≈ 90 ms, main.tasks.long 0
//   phases        phase.a 2 × 10 ms (one run enters it twice), phase.b 40 ms, count.ctx 7, count.page 5
//   layout        25 forced layouts (layout.count 25, style.count 25), nothing else dirty
//   net           3 uncached requests of 1,000, 10,000 and 100,000 body bytes: net.requests 3, and net.bytes as the
//                 server wrote them (count.wire, headers included: the server is raw TCP, so it knows every byte)
//   hot           hotA spins 3 × 30 ms, hotB 3 × 10 ms: hotspots A:B 3:1 (total time)
//   dirty         2,000 cells' widths written and never read: headless, HeadlessFrame names the layout left
//                 to the frame after the run; dirty-click does the same through a click (laid out in the run)
//   idle          a 200 ms wait in strobe: main.work ≈ 0, run.duration ≈ 200 ms
//   frames        30 animation frames: every frame metric refused Headless
//   frameclock    count.frame_us: the page's own time for two animation frames, which strobe waits for after a run
//   leak          1,000,000 doubles kept per run in 20 arrays of 50,000 (20 × 400,016 bytes of elements, plus the
//                 arrays): leaks ≈ +7.63 MiB/run
//   heap          50 MiB of ArrayBuffer (every page written) kept by the block's first run (a warmup), nothing after:
//                 memory.app.private holds it, js.heap.used does not (V8 keeps array buffers' bytes off its heap)
import { bench, url } from "@hona/strobe"
import { serve, type Known } from "./known-server"

let server: Known | null = null
// Bytes and requests the server answered since it started: the network's truth.
const wire = () => server?.wire ?? { bytes: 0, requests: 0 }

const port = Number(process.env.KNOWN_PORT ?? 4823)
const target = url(`http://127.0.0.1:${port}/`, {
  prepare() {
    server ??= serve(port)
  },
  teardown() {
    server?.stop()
    server = null
  },
})

// CPU time (ms) of the browser this strobe drives, from the OS, and its processes' ids; null off Windows or
// when none is found. Only its tree: the processes whose command line names this strobe's profile
// (`run\profile-<strobe pid>-…`, strobe being the runner's parent) and their descendants, never a person's
// browser running beside it. A process that ends takes its CPU out of the next read: compare two reads
// only when every process of the first is in the second.
function osCpu(): { ms: number; pids: number[] } | null {
  if (process.platform !== "win32") return null
  // The profile reaches PowerShell in its environment, not its command line, which it would match itself.
  const profile = `\\strobe\\run\\profile-${process.ppid}-`.toLowerCase()
  const ps = [
    "$all = @(Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,CommandLine,CreationDate,KernelModeTime,UserModeTime)",
    "$set = @{}; foreach ($p in $all) { if ($p.ProcessId -ne $PID -and $p.CommandLine -and $p.CommandLine.ToLower().Contains($env:STROBE_PROFILE)) { $set[$p.ProcessId] = $p } }",
    "do { $n = $set.Count; foreach ($p in $all) { $up = $set[$p.ParentProcessId]; if (-not $set.ContainsKey($p.ProcessId) -and $up -and $p.CreationDate -ge $up.CreationDate) { $set[$p.ProcessId] = $p } } } while ($set.Count -ne $n)",
    "if ($set.Count) { [string]($set.Values | ForEach-Object { $_.KernelModeTime + $_.UserModeTime } | Measure-Object -Sum).Sum + ';' + (($set.Keys | Sort-Object) -join ',') }",
  ].join("; ")
  const out = Bun.spawnSync(["powershell", "-NoProfile", "-Command", ps], { env: { ...process.env, STROBE_PROFILE: profile } }).stdout.toString().trim()
  if (!out) return null
  const [sum, pids] = out.split(";")
  return { ms: Number(sum) / 1e4, pids: (pids ?? "").split(",").filter(Boolean).map(Number) }
}

const main = ["main.work", "main.busy", "main.script", "main.tasks.long", "run.duration", "cpu.renderer", "cpu.app.total", "cpu.browser", "cpu.gpu"]

export default bench({
  targets: { known: target },
  cases: {
    spin100: { primary: "cpu.work", metrics: main, run: ({ page }) => page.eval(() => (window as any).known.spin(100)) },
    // spin100 with 100 ms of rest after it, and before it: CPU at a run's edges.
    spinrest: {
      primary: "cpu.work",
      metrics: main,
      async run({ page }) {
        await page.eval(() => (window as any).known.spin(100))
        await page.wait(100)
      },
    },
    restspin: {
      primary: "cpu.work",
      metrics: main,
      async run({ page }) {
        await page.wait(100)
        await page.eval(() => (window as any).known.spin(100))
      },
    },
    spin70: { primary: "cpu.work", metrics: main, run: ({ page }) => page.eval(() => (window as any).known.spin(70)) },
    // The same work from the page's own handler: a click whose listener spins 50 ms (main.script ≈ 50 ms).
    click50: { primary: "cpu.work", metrics: main, run: ({ page }) => page.click("#spin") },
    tasks3x30: { primary: "cpu.work", metrics: main, run: ({ page }) => page.eval(() => (window as any).known.tasks(3, 30)) },
    phases: {
      primary: "phase.b",
      metrics: ["phase.a", "count.ctx", "count.page", "run.duration"],
      async run({ page, phase, count }) {
        await phase("a", () => page.eval(() => (window as any).known.spin(10)))
        await phase("b", () => page.eval(() => (window as any).known.spin(40)))
        await phase("a", () => page.eval(() => (window as any).known.spin(10)))
        count("ctx", 3)
        count("ctx", 4)
        await page.eval(() => (globalThis as any).__strobeCount("page", 5))
      },
    },
    layout: {
      primary: "cpu.work",
      metrics: ["layout.count", "style.count", "main.layout", "main.style", "main.work"],
      run: ({ page }) => page.eval(() => (window as any).known.layouts(25)),
    },
    net: {
      primary: "net.requests",
      metrics: ["net.bytes", "net.bytes{/blob/100000}", "net.requests{/blob/*}", "count.wire", "count.requests"],
      async run({ page, count }) {
        const before = { ...wire() }
        await page.eval((sizes: number[]) => (window as any).known.fetchAll(sizes), [1000, 10_000, 100_000])
        count("wire", wire().bytes - before.bytes)
        count("requests", wire().requests - before.requests)
      },
    },
    // How long two of the page's animation frames take (count.frame_us): strobe ends each run on the page's next frame.
    frameclock: {
      primary: "cpu.work",
      metrics: ["run.duration", "count.frame_us"],
      async run({ page, count }) {
        count("frame_us", Math.round(1000 * (await page.eval(() => (window as any).known.twoFrames()))))
      },
    },
    // What page.eval hands back (count.ret 42, count.promised 7, count.frame_ms > 0).
    evalret: {
      primary: "cpu.work",
      metrics: ["count.ret", "count.promised", "count.frame_ms"],
      async run({ page, count }) {
        count("ret", Number(await page.eval(() => 42)))
        count("promised", Number(await page.eval(() => Promise.resolve(7))))
        count("frame_ms", Number(await page.eval(() => (window as any).known.twoFrames())) > 0 ? 1 : 0)
      },
    },
    // A 2 s spin, with the browser's CPU read by the OS around it (Windows: the kernel and user time of
    // strobe's browser tree, 15.6 ms ticks): count.os_cpu_ms is the truth for cpu.work. A process of the
    // tree that ended in the run (a utility process) took its CPU out of the second read, which can no
    // longer say the truth: count.os_cpu_ended counts those instead, and no count.os_cpu_ms is given. One
    // born and ended inside the run is in neither read: compare only runs the recording's ended_runs does
    // not list (`strobe query <rec> summary`, `ended` lines), as strobe's cpu.work counts it.
    cpu2s: {
      primary: "cpu.work",
      metrics: ["count.os_cpu_ms", "count.os_cpu_ended", "cpu.renderer", "cpu.app.total", "run.duration"],
      async run({ page, count }) {
        const before = osCpu()
        await page.eval(() => (window as any).known.spin(2000))
        const after = osCpu()
        if (before == null || after == null) return
        const ended = before.pids.filter((p) => !after.pids.includes(p)).length
        count("os_cpu_ended", ended)
        if (ended === 0) count("os_cpu_ms", Math.round(after.ms - before.ms))
      },
    },
    // Writes 2,000 cells' styles and returns without reading them: a headless run's frame after it lays
    // them out, past the run (HeadlessFrame names it). Through a click, the input's own path lays them
    // out inside the run: no warning.
    dirty: { primary: "cpu.work", metrics: ["main.work", "layout.count", "main.layout"], run: ({ page }) => page.eval(() => (window as any).known.dirty(2000)) },
    "dirty-click": { primary: "cpu.work", metrics: ["main.work", "layout.count", "main.layout"], run: ({ page }) => page.click("#dirty") },
    hot: { primary: "cpu.work", metrics: ["main.work"], run: ({ page }) => page.eval(() => (window as any).known.hot(30, 10, 3)) },
    idle: { primary: "cpu.work", metrics: main, run: ({ page }) => page.wait(200) },
    frames: {
      primary: "cpu.work",
      metrics: ["frames.presented.p95", "frames.interval.p95", "frames.dropped.pct", "frames.dropped", "main.work"],
      run: ({ page }) => page.eval(() => (window as any).known.frames(30)),
    },
    leak: {
      primary: "cpu.work",
      metrics: ["js.heap.used", "memory.app.private"],
      run: ({ page }) => page.eval(() => (window as any).known.keepDoubles(1_000_000)),
    },
    heap: {
      primary: "cpu.work",
      metrics: ["js.heap.used", "memory.app.private", "dom.nodes"],
      run: ({ page }) => page.eval(() => (window as any).known.keepBufferOnce(50 * 1024 * 1024)),
    },
  },
})
