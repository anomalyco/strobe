// Known answers for who loads a script() target (dev only, not shipped): server-host.ts serves HTTP on the loopback.
// Run from the repo root:
//   strobe bench scripts/dev/known/client.bench.ts --head . --check
// Truth:
//   load    the bench file's run() loads the server with fetch for 1 s and makes no ctx.call: the runner's CPU is the
//           client's, not strobe relaying calls. Pinned (strobe's CPUs): no ObserverCost about a relay, and none at
//           all unless the client nears one JS thread's ceiling (85% of a core); unpinned: OverheadBudget as for
//           any runner on the target's CPUs
//   flood   the same client against a server that answers at once (64 requests in flight): the client is the
//           bottleneck, near one JS thread's ceiling: pinned, ObserverCost 'the bench file's run() used …'
//   relay   400 ctx.call("ping") a run: the runs are mostly strobe relaying them: ObserverCost 'relaying 400 ctx.call a run'
import { bench, script } from "@hona/strobe"

let url = ""

const load = (inFlight: number) => async () => {
  const end = performance.now() + 1000
  while (performance.now() < end) await Promise.all(Array.from({ length: inFlight }, () => fetch(url).then((r) => r.text())))
}

export default bench({
  targets: { server: script("./server-host.ts") },
  setup: async ({ call, case: c }: any) => {
    url = `http://127.0.0.1:${await call("port")}/`
    await call("cost", c === "flood" ? 0 : 200)
  },
  cases: {
    load: { primary: "cpu.work", metrics: ["run.duration"], run: load(8) },
    flood: { primary: "cpu.work", metrics: ["run.duration"], run: load(64) },
    relay: {
      primary: "cpu.work",
      metrics: ["run.duration"],
      run: async ({ call }: any) => {
        for (let i = 0; i < 400; i++) await call("ping")
      },
    },
  },
})
