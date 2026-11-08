// Known-answer bench for a process that ends inside a run (dev only, not shipped). From the repo root (build
// scripts/dev/known first for `breakaway`):
//   strobe bench scripts/dev/derive/script.bench.ts --head . --check
// Truth per run:
//   child timed   a child process spins 200 ms and ends inside the run: its CPU left the host's tree. Linux
//                 with launch cgroups: cpu.work about 200 ms (the cgroup kept it); Windows about 200 ms too
//                 (strobe held the child from its start in the launch's job); elsewhere refused RunIncomplete
//                 (it read the host's few ms before)
//   orphan timed  a child starts a grandchild that spins 200 ms and exits at once; the grandchild is adopted
//                 and reaped outside the host's tree and ends inside the run. Linux with launch cgroups: about
//                 200 ms; elsewhere refused (what the host reaped is the child's few ms, never the grandchild's;
//                 on Windows the child's runtime ends the grandchild with it, a run of about 30 ms, all of it
//                 counted)
//   outlive       as orphan timed, but the call returns while the grandchild still spins: the run ends with it
//                 running in the launch's cgroup outside the host's tree, which counts none of it: refused
//                 RunIncomplete everywhere (Linux once read a short cpu.work, the grandchild's CPU taken out
//                 as running; on Windows the child's runtime ends the grandchild with it, as orphan timed);
//                 the reset waits for it
//   breakaway     a child whose own child spins 200 ms after asking to leave the job it starts in (Windows:
//                 CREATE_BREAKAWAY_FROM_JOB, which strobe's launch job keeps): about 200 ms where child timed
//                 is, refused where it is
//   breakaway hop 2  the same 200 ms in a great-grandchild that asked again from a child that broke away:
//                 it starts (the run throws where it cannot) and reads as breakaway
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
    breakaway: { primary: "run.duration", metrics: ["cpu.work"], run: ({ call }: any) => call("breakaway", 200) },
    "breakaway hop 2": { primary: "run.duration", metrics: ["cpu.work"], run: ({ call }: any) => call("breakaway", 200, 2) },
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
