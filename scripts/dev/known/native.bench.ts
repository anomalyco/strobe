// Known-answer bench for windowless native() targets (dev only, not shipped): known.rs's long-running modes cost
// what their arguments say. Build known.rs first (see cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/native.bench.ts --head . --check
// Truth per 1000 ms run (page.wait):
//   duty      2 threads each busy 50% of every 100 ms: cpu.work 1000 ms, cpu.app.total 100% (of one core),
//             cpu.wakes about 20/s (each thread sleeps 10 times a second)
//   wake      one thread waking 100 times a second: cpu.wakes about 100/s, cpu.work near 0 (its sleep of 10 ms
//             wakes late on macOS: about 78/s there, so no answer; `tick` keeps a fixed schedule)
//   hold      150 MiB private written once: memory.app.private and memory.app.host 150 MiB above the idle program
//   shm       150 MiB of shared anonymous memory written: not private on Windows (a pagefile section) or Linux
//             (shmem); macOS's footprint charges it once to the process that made it: 150 MiB above idle there
//   (a timed run's wall time is page.wait's sleep, which wakes 1.5-10 ms late on macOS: planted.bench.ts
//   `pulse` is the wall answer for a windowless app; tick-4 vs tick-8 plants CPU time, not instructions or energy
//   on Apple silicon: planted.bench.ts `counters` plants those)
//   map       150 MiB of a file mapped read-only and read: file-backed, not private
//   idle      the program sleeping: the base the memory cases are above
//   spawner   a child of 100 ms CPU every 200 ms, each ending inside the run: cpu.work about 500 ms (or refused)
//   exits     the program spins 200 ms and exits, inside its first run: refused (the app's processes are gone
//             before the run ends), never a cpu.work or memory read of what is left
//   ready-0 / ready-300  launch until it prints READY: the two launch.ready differ by 300 ms
//   tick-0 / tick-4 / tick-8  planted work: one thread waking 50 times a second, spinning 0, 4 or 8 ms of its CPU
//             each wake: cpu.work o, 200 + o and 400 + o ms, where o is the wakes' own cost (tick-0 reads it), and
//             cpu.wakes about 50/s on all three. tick-8 over tick-4 is a planted 2x: exactly (400 + o) / (200 + o)
import { bench, native } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
const run = (args: string[]) => native(bin, { args, window: false })
const all = ["cpu.work", "cpu.app.total", "cpu.wakes", "memory.app.private", "memory.app.host", "io.read", "io.write"]
const wait = (primary: string) => ({ primary, metrics: all.filter((m) => m !== primary), run: ({ page }: any) => page.wait(1000) })

export default bench({
  targets: {
    duty: run(["duty", "50", "2", "600"]),
    wake: run(["wake", "100", "600"]),
    hold: run(["hold", "150", "600"]),
    shm: run(["shm", "150", "600000"]),
    map: run(["mapfile", "150", "600000"]),
    idle: run(["sleep", "600000"]),
    spawner: run(["spawner", "100", "100", "600"]),
    exits: run(["spin", "200"]),
    "ready-0": run(["ready", "0", "READY"]),
    "ready-300": run(["ready", "300", "READY"]),
    "tick-0": run(["tick", "50", "0", "600"]),
    "tick-4": run(["tick", "50", "4", "600"]),
    "tick-8": run(["tick", "50", "8", "600"]),
  },
  cases: {
    duty: { targets: ["duty"], ...wait("cpu.work") },
    wake: { targets: ["wake"], ...wait("cpu.wakes") },
    spawner: { targets: ["spawner"], ...wait("cpu.work") },
    exits: { targets: ["exits"], ...wait("cpu.work") },
    tick: { targets: ["tick-0", "tick-4", "tick-8"], ...wait("cpu.work") },
    memory: { targets: ["hold", "shm", "map", "idle"], ...wait("memory.app.private") },
    ready: {
      targets: ["ready-0", "ready-300"],
      primary: "launch.ready",
      metrics: ["cpu.work", "memory.app.private"],
      launch: { until: { milestone: "up" }, milestones: { up: { match: "READY" } } },
    },
  },
})
