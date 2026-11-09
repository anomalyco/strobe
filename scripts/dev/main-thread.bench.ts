// Known-answer bench for cpu.main (dev only, not shipped): which thread is busy, from main-thread-harness.ts.
// Run from the repo root (noisy with --check):
//   strobe bench scripts/dev/main-thread.bench.ts --head . --check
// Truth per 300 ms run:
//   main-spin    the main thread busy: cpu.work ~300 ms, cpu.main ~300 ms (cpu.main.busy ~100%)
//   main-spin-30 the same for 30 ms: cpu.work ~30 ms, never below cpu.main (the host idles after the run, so a
//                cycles-to-ms rate taken over the end's process listing would read it short)
//   worker-spin  a worker thread busy:  cpu.work ~300 ms, cpu.main < 10 ms
//   child-spin   a child process busy:  cpu.work ~300 ms (or refused where the child's CPU left with it), cpu.main < 10 ms
//   timer-wait   a timer: cpu.work < 5 ms, cpu.main < 5 ms
// The app-* cases run the same thing without end in a windowless native() program, each run a 300 ms wait.
// A late timeline tick (dev hook): STROBE_DEV_TIMELINE_STALL_MS=2600 strobe record pid:<app-main-spin's pid> --for 6s,
// then `strobe query <rec> threads --timeline`: the main thread reads `#` in every second, no blank column and no spike.
import { bench, native, script } from "@hona/strobe"
import { copyFileSync, existsSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const harness = join(import.meta.dir, "main-thread-harness.ts")
// The runtime under a name of its own: strobe refuses an interpreter as a native app (any script of it would count).
const exe = join(tmpdir(), process.platform === "win32" ? "main-thread-app.exe" : "main-thread-app")
if (!existsSync(exe)) copyFileSync(process.execPath, exe)
const app = (mode: string) => native(exe, { args: [harness, mode], window: false })
const metrics = ["cpu.main", "cpu.main.busy", "run.duration"]
const call = (name: string, ms = 300) => ({ targets: ["harness"], primary: "cpu.work", metrics, run: ({ call }: any) => call(name, ms) })
const wait = (target: string) => ({ targets: [target], primary: "cpu.work", metrics, run: ({ page }: any) => page.wait(300) })

export default bench({
  // A program's runtime compiles on its main thread for a second or so after it starts: --check's one cold run
  // would count that.
  setup: ({ target, page }) => (target.startsWith("app-") ? page.wait(2000) : undefined),
  targets: {
    harness: script("./main-thread-harness.ts"),
    "app-main-spin": app("main-spin"),
    "app-worker-spin": app("worker-spin"),
    "app-child-spin": app("child-spin"),
    "app-timer-wait": app("timer-wait"),
  },
  cases: {
    "main-spin": call("mainSpin"),
    "main-spin-30": call("mainSpin", 30),
    "worker-spin": call("workerSpin"),
    "child-spin": call("childSpin"),
    "timer-wait": call("timerWait"),
    "app-main-spin": wait("app-main-spin"),
    "app-worker-spin": wait("app-worker-spin"),
    "app-child-spin": wait("app-child-spin"),
    "app-timer-wait": wait("app-timer-wait"),
  },
})
