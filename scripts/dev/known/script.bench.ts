// Known-answer bench for script() targets (dev only, not shipped): harness.ts's exports cost what their names say.
// Build known.rs first (see cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/script.bench.ts --head . --check
// Truth per run: spin 300 ms CPU over 300 ms; sleep 0 CPU over 400 ms; keep-0 and keep-100 differ by 100 MiB of
// memory.app.private (each run's memory is cleared by the reset); child 300 ms CPU in a child that exits inside
// the run (not in the host's tree at the run's end: cpu.work must not read ~0 as valid); io 32 MiB written and read.
import { bench, script } from "@hona/strobe"

const all = ["cpu.app.total", "run.duration", "memory.app.private", "cpu.wakes", "io.read", "io.write"]
const call = (name: string, arg: number, primary = "cpu.work") => ({
  primary,
  metrics: [...all, "cpu.work"].filter((m) => m !== primary),
  run: ({ call }: any) => call(name, arg),
  reset: ({ call }: any) => call("clear"),
})

export default bench({
  targets: { harness: script("./harness.ts") },
  cases: {
    spin: call("spin", 300),
    sleep: call("sleep", 400, "run.duration"),
    "keep-0": call("keep", 0, "memory.app.private"),
    "keep-100": call("keep", 100, "memory.app.private"),
    child: call("child", 300),
    io: call("io", 32, "io.write"),
  },
})
