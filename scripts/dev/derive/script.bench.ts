// Known-answer bench for a process that ends inside a run (dev only, not shipped). From the repo root:
//   strobe bench scripts/dev/derive/script.bench.ts --head . --check
// Truth per run:
//   child timed   a child process spins 200 ms and ends inside the run: its CPU left the host's tree. Linux
//                 with launch cgroups: cpu.work about 200 ms (the cgroup kept it); elsewhere refused
//                 RunIncomplete (it read the host's few ms before)
//   orphan timed  a child starts a grandchild that spins 200 ms and exits at once; the grandchild is adopted
//                 and reaped outside the host's tree and ends inside the run. Linux with launch cgroups: about
//                 200 ms; elsewhere refused (what the host reaped is the child's few ms, never the grandchild's;
//                 on Windows the child's runtime ends the grandchild with it, a run of about 30 ms)
//   outlive       as orphan timed, but the call returns while the grandchild still spins: the run ends with it
//                 running in the launch's cgroup outside the host's tree, which counts none of it: refused
//                 RunIncomplete everywhere (Linux once read a short cpu.work, the grandchild's CPU taken out
//                 as running); the reset waits for it
//   child untimed the same child inside ctx.untimed, then 100 ms of the host's own: cpu.work about 100 ms
//   plain         100 ms of the host's own: cpu.work about 100 ms (no child: nothing refused)
import { bench, script } from "@hona/strobe"

export default bench({
  targets: { harness: script("./harness.ts") },
  cases: {
    "child timed": { primary: "run.duration", metrics: ["cpu.work"], run: ({ call }: any) => call("child", 200) },
    "orphan timed": { primary: "run.duration", metrics: ["cpu.work"], run: ({ call }: any) => call("orphan", 200) },
    outlive: {
      primary: "run.duration",
      metrics: ["cpu.work"],
      run: async ({ call }: any) => {
        await call("outlive", 200)
        await call("spin", 50)
      },
      reset: ({ call }: any) => call("settle"),
    },
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
