// Known-answer bench for cli() targets (dev only, not shipped): every case runs known.rs, whose costs are known by
// construction, and declares every metric a cli() run gives, so each printed number can be checked against the
// truth (its truth_* lines come back as metric.truth_*). Build the fixture first, then run from the repo root:
//   rustc -O -o scripts/dev/known/known scripts/dev/known/known.rs        (known.exe on Windows)
//   strobe bench scripts/dev/known/cli.bench.ts --head . --check
//   strobe bench scripts/dev/known/cli.bench.ts --head . --blocks 2 --filter spin-2
// Truth per run: spin-1 300 ms CPU; spin-2 600 ms CPU over ~300 ms wall (cpu.app.total 200% of one core); sleep
// 0 CPU over 500 ms; tree 3 × 400 ms in children; grand 2 × 400 ms in grandchildren; breakaway 400 ms in a child
// that asks to leave the job it starts in (Windows: CREATE_BREAKAWAY_FROM_JOB), breakaway-2 the same in a
// grandchild that asked again from that child; breakaway-left leaves that grandchild running (refused
// RunIncomplete, and strobe ends it: no known process left after); twice 2 commands of 150 ms;
// timer 100 wakes a second; alloc 200 MiB touched (alloc-small 4 MiB, under strobe's own resident set); io 40 MiB written, 24 MiB read; orphan leaves a process
// running (refused RunIncomplete).
// Paired runs (`strobe run --base "<known> …" --head "<known> …"`): a spin is CPU work, counted by the thread's CPU
// clock (Linux, macOS) or cycle count (Windows x64), so other work on the same CPUs cannot make it do less; it waits
// out the CPU it is denied, so its wall time is no plant when the CPUs are shared: plant wall time with `sleep 150`
// vs `sleep 300`. (Before the cycle count, a Windows spin was wall time: three children sharing two CPUs with a
// spinner did 313-391 ms of CPU for 1200 planted; now 1214.7-1215.7 ms, 1200 plus their launches.) Each process's own launch and exit (sleep's cpu.work and
// run.duration less its 500 ms) is the fixed cost b0 of a planted ratio; page tables add 100 KiB per 50 MiB of
// alloc on Windows (4 KiB pages, 8 bytes each), which memory.peak counts. The tree's parent spends 20-30 ms of its
// own CPU launching three children on Windows, but 180-240 ms in 3 of 14 runs there: the plant is the children's.
import { bench, cli } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
const all = ["cpu.app.total", "run.duration", "cli.launch", "cpu.wakes", "memory.peak", "io.read", "io.write"]
const one = (line: string, primary = "cpu.work", extra: string[] = []) => ({
  primary,
  metrics: [...all, "cpu.work", ...extra].filter((m) => m !== primary),
  resetCheck: false,
  run: ({ exec }: any) => exec(`"${bin}" ${line}`),
})

export default bench({
  targets: { known: cli() },
  cases: {
    "spin-1": one("spin 300", "cpu.work", ["metric.truth_cpu_ms"]),
    "spin-2": one("spin 300 2", "cpu.work", ["metric.truth_cpu_ms"]),
    sleep: one("sleep 500", "run.duration"),
    tree: one("tree 3 400", "cpu.work", ["metric.truth_cpu_ms"]),
    grand: one("tree 2 400 grand", "cpu.work", ["metric.truth_cpu_ms"]),
    breakaway: one("breakaway 400", "cpu.work", ["metric.truth_cpu_ms"]),
    "breakaway-2": one("breakaway 400 2", "cpu.work", ["metric.truth_cpu_ms"]),
    "breakaway-left": one("breakaway 200 2 leave", "cpu.work"),
    twice: {
      primary: "cpu.work",
      metrics: [...all, "metric.truth_cpu_ms"],
      resetCheck: false,
      run: async ({ exec }: any) => {
        await exec(`"${bin}" spin 150`)
        await exec(`"${bin}" spin 150`)
      },
    },
    // A small command then a big one: the run's peak is the big one's (40 MiB + its base), whatever
    // the small one's reads on Linux.
    "peak-mixed": {
      primary: "memory.peak",
      resetCheck: false,
      run: async ({ exec }: any) => {
        await exec(`"${bin}" alloc 1 50`)
        await exec(`"${bin}" alloc 40 50`)
      },
    },
    timer: one("timer 100 1000", "cpu.wakes", ["metric.truth_wakes_per_s"]),
    alloc: one("alloc 200 300", "memory.peak"),
    "alloc-small": one("alloc 4 300", "memory.peak"),
    io: one("io 40 24", "io.write", ["metric.truth_write_bytes", "metric.truth_read_bytes"]),
    orphan: one("orphan 200", "cpu.work"),
  },
})
