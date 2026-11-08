// Known-answer bench for windowless native() targets (dev only, not shipped): known.rs's long-running modes cost
// what their arguments say. Build known.rs first (see cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/native.bench.ts --head . --check
// Truth per 1000 ms run (page.wait):
//   duty      2 threads each busy 50% of every 100 ms: cpu.work 1000 ms, cpu.app.total 100% (of one core),
//             cpu.wakes about 20/s (each thread sleeps 10 times a second)
//   wake      one thread waking 100 times a second: cpu.wakes about 100/s, cpu.work near 0
//   hold      150 MiB private written once: memory.app.private and memory.app.host 150 MiB above the idle program
//   shm       150 MiB of shared anonymous memory written: not private on Windows (a pagefile section)
//   map       150 MiB of a file mapped read-only and read: file-backed, not private
//   idle      the program sleeping: the base the memory cases are above
//   spawner   a child of 100 ms CPU every 200 ms, each ending inside the run: cpu.work about 500 ms (or refused)
//   ready-0 / ready-300  launch until it prints READY: the two launch.ready differ by 300 ms
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
    "ready-0": run(["ready", "0", "READY"]),
    "ready-300": run(["ready", "300", "READY"]),
  },
  cases: {
    duty: { targets: ["duty"], ...wait("cpu.work") },
    wake: { targets: ["wake"], ...wait("cpu.wakes") },
    spawner: { targets: ["spawner"], ...wait("cpu.work") },
    memory: { targets: ["hold", "shm", "map", "idle"], ...wait("memory.app.private") },
    ready: {
      targets: ["ready-0", "ready-300"],
      primary: "launch.ready",
      metrics: ["cpu.work", "memory.app.private"],
      launch: { until: { milestone: "up" }, milestones: { up: { match: "READY" } } },
    },
  },
})
