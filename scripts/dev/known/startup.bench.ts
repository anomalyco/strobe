// Known-answer bench for startup that runs into the runs (dev only, not shipped): known.rs's `startup` mode keeps a
// thread named `loader` busy for its first 4 s, then prints READY and idles. Build known.rs first (see
// cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/startup.bench.ts --head . --check
//   strobe bench scripts/dev/known/startup.bench.ts --head . --blocks 1 --filter plateau
// Truth:
//   spill     setup waits 1 s, the run 5 s: loader busy from the launch to 4 s, so run 1's first half is all its work and
//             its second half 0.5 s of it: --check warns NotSteady naming loader (never a refusal)
//   ready     the same app, setup waits for READY (since the launch): the run starts after the load, nothing to warn
//   steady    two threads busy 50% of every 100 ms (duty) and one busy throughout: as busy in both halves, no warning
//   plateau   a timed block of 1 s runs: runs 1-3 are the load (about a core each), later ones idle; the steady pair the
//             load makes is left (Warmup's plateau guard) and the measured runs are the idle ones (cpu.work ≈ 0)
//   script    a script host that starts a 4 s spin in a child at its import: the --check's run 1 warns NotSteady too
//   front     no setup, a run that spins 1.5 s then sleeps 1.5 s: its own work, front-loaded; nothing ran before it,
//             so no warning (a few ms of the host's start in the moment before run 1 are no startup)
//   page      headless pages/startup.html: ?mode=load spins 90% of the main thread for 4 s from the load (setup 1 s,
//             run 5 s): NotSteady from the OS sampler on the pinned CPUs; ?mode=steady spins 50% throughout: none
import { bench, native, script, url } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
const page = (mode: string) => new URL(`../pages/startup.html?mode=${mode}`, import.meta.url).href
const app = (args: string[]) => native(bin, { args, window: false, output: true })

export default bench({
  targets: {
    startup: app(["startup", "4000", "600"]),
    duty: app(["duty", "50", "2", "600"]),
    busy: app(["duty", "100", "1", "600"]),
    loading: script("./startup-host.ts"),
    harness: script("./harness.ts"),
    "page-load": url(page("load"), { headless: true }),
    "page-steady": url(page("steady"), { headless: true }),
  },
  setup: async ({ page, case: c, waitForLine }: any) => {
    if (c === "ready") await waitForLine("READY", { since: "launch", timeout: 15_000 })
    else if (c === "script") await Bun.sleep(1000)
    else if (c !== "plateau" && c !== "front") await page.wait(1000)
  },
  cases: {
    spill: { targets: ["startup"], primary: "cpu.work", run: ({ page }: any) => page.wait(5000) },
    ready: { targets: ["startup"], primary: "cpu.work", run: ({ page }: any) => page.wait(5000) },
    steady: { targets: ["duty", "busy"], primary: "cpu.work", run: ({ page }: any) => page.wait(5000) },
    plateau: { targets: ["startup"], primary: "cpu.work", run: ({ page }: any) => page.wait(1000) },
    // run.duration: the child ends inside the run, which may refuse the host tree's cpu.work.
    script: { targets: ["loading"], primary: "run.duration", run: ({ call }: any) => call("wait", 5000) },
    // No setup, and the run's own work first: 1.5 s of spin, then 1.5 s of sleep. Nothing ran before it.
    front: {
      targets: ["harness"],
      primary: "cpu.work",
      run: async ({ call }: any) => {
        await call("spin", 1500)
        await call("sleep", 1500)
      },
    },
    page: { targets: ["page-load", "page-steady"], primary: "cpu.work", run: ({ page }: any) => page.wait(5000) },
  },
})
