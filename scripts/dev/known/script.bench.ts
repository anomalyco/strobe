// Known-answer bench for script() targets (dev only, not shipped): harness.ts's exports cost what their names say.
// Build known.rs first (see cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/script.bench.ts --head . --check
//   strobe bench scripts/dev/known/script.bench.ts --head . --check --runs 3 --filter "heap-*"
// Truth per run: spin 300 ms CPU over 300 ms; sleep 0 CPU over 400 ms; keep-0 and keep-100 differ by 100 MiB of
// memory.app.private and of js.heap.retained (each run's memory is cleared by the reset), on Bun (`harness`) and on
// Node (`node`): on macOS a Bun host's memory.app.private is refused (its allocator reuses pages the footprint no
// longer counts: 100 MiB held read as 5.1 MiB), and js.heap.retained still reads the 100 MiB; child 300 ms CPU in a
// child that exits inside the run (not in the host's tree at the run's end: cpu.work must not read ~0 as valid);
// io 32 MiB written and read.
// heap-<shape>: 100 MiB kept in each shape a module holds buffer memory in (harness.ts `hold`): js.heap.retained
// 100 MiB on Bun and on Node, every shape (Bun's own figures repeat a buffer once an ArrayBuffer object exists;
// strobe counts it once). heap-objects: 1,000,000 small objects and no buffer, about 92 MiB on Bun and 71 MiB on
// Node: the reading strobe 0.31.0 gave, which counted the heap alone.
import { bench, script } from "@hona/strobe"

const all = ["cpu.app.total", "run.duration", "memory.app.private", "cpu.wakes", "io.read", "io.write"]
const call = (name: string, arg: number, primary = "cpu.work", extra: string[] = []) => ({
  targets: ["harness"],
  primary,
  metrics: [...all, ...extra, "cpu.work"].filter((m) => m !== primary),
  run: ({ call }: any) => call(name, arg),
  reset: ({ call }: any) => call("clear"),
})
const keep = (mb: number) => ({ ...call("keep", mb, "memory.app.private", ["js.heap.retained"]), targets: ["harness", "node"] })
const heap = (shape: string, mb = 100) => ({
  targets: ["harness", "node"],
  primary: "js.heap.retained",
  run: ({ call }: any) => call("hold", shape, mb),
  reset: ({ call }: any) => call("clear"),
})

export default bench({
  targets: { harness: script("./harness.ts"), node: script("./harness.ts", { runtime: "node" }) },
  cases: {
    spin: call("spin", 300),
    sleep: call("sleep", 400, "run.duration"),
    "keep-0": keep(0),
    "keep-100": keep(100),
    child: call("child", 300),
    io: call("io", 32, "io.write"),
    "heap-typed": heap("typed"),
    "heap-view": heap("view"),
    "heap-arraybuffer": heap("arraybuffer"),
    "heap-dataview": heap("dataview"),
    "heap-response": heap("response"),
    "heap-nodebuffer": heap("nodebuffer"),
    "heap-objects": heap("objects", 1000),
  },
})
