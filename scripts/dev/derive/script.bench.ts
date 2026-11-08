// Known-answer bench for a process that ends inside a run (dev only, not shipped). From the repo root:
//   strobe bench scripts/dev/derive/script.bench.ts --head . --check
// Truth per run:
//   child timed   a child process spins 200 ms and ends inside the run: its CPU left the host's tree, so
//                 cpu.work is refused RunIncomplete (it read the host's few ms before)
//   child untimed the same child inside ctx.untimed, then 100 ms of the host's own: cpu.work about 100 ms
//   plain         100 ms of the host's own: cpu.work about 100 ms (no child: nothing refused)
import { bench, script } from "@hona/strobe"

export default bench({
  targets: { harness: script("./harness.ts") },
  cases: {
    "child timed": { primary: "run.duration", metrics: ["cpu.work"], run: ({ call }: any) => call("child", 200) },
    "child untimed": {
      primary: "cpu.work",
      run: async (ctx: any) => {
        await ctx.untimed(() => ctx.call("child", 200))
        await ctx.call("spin", 100)
      },
    },
    plain: { primary: "cpu.work", run: ({ call }: any) => call("spin", 100) },
  },
})
