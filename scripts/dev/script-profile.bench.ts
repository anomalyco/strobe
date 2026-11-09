// Known-answer bench for run-scoped script profiles (dev only, not shipped). From the repo root, per target
// (`--target bun`, `--target node`):
//   strobe record --bench 'scripts/dev/script-profile.bench.ts#phases' --head . --target bun
//   strobe query <rec> hotspots                 burnRunPhaseA and burnRunPhaseB about 30 ms each per measured run, and
//                                               nothing of burnSetup, burnReset, burnUntimed or moduleLoadWork; the idle
//                                               20 ms adds no CPU to any function
//   strobe query <rec> hotspots --phase a       burnRunPhaseA alone
//   strobe query <rec> hotspots --source host   all of them: moduleLoadWork about 200 ms (100 modules × 2 ms, as the
//                                               host loads), burnSetup 40 ms per block, burnReset 40 ms per run
//   strobe record --bench 'scripts/dev/script-profile.bench.ts#await-loop' --head . --target bun
//                                               an async loop: 20 × (100 µs in tickWork, then a 10 ms sleep). hotspots:
//                                               tickWork about 2 ms per run (20 × 0.1), never 20 × a sleep; meta.clock.cap_ms
//                                               about 4 sampling intervals and meta.sampling's median one interval (the sleeps
//                                               in neither); meta.starved null on Bun, near 0 on Node
//   strobe record --bench 'scripts/dev/script-profile.bench.ts#fail' --head . --target bun
//                                               fails (exit 1 or 4); no rec_ folder for it, and no *-prof folder left in
//                                               strobe's run folder
import { bench, script } from "@hona/strobe"

export default bench({
  targets: {
    bun: script("./script-profile-harness.mjs"),
    node: script("./script-profile-harness.mjs", { runtime: "node" }),
  },
  setup: ({ call }: any) => call("burnSetup", 40),
  reset: ({ call }: any) => call("burnReset", 40),
  cases: {
    phases: {
      primary: "cpu.work",
      metrics: ["phase.a", "phase.b"],
      run: async (ctx: any) => {
        await ctx.phase("a", () => ctx.call("burnRunPhaseA", 30))
        await ctx.phase("b", () => ctx.call("burnRunPhaseB", 30))
        await ctx.untimed(() => ctx.call("burnUntimed", 20))
        await ctx.call("idle", 20)
      },
    },
    "await-loop": { primary: "run.duration", run: ({ call }: any) => call("awaitLoop", 20) },
    fail: { primary: "cpu.work", run: ({ call }: any) => call("failMidRun", 30) },
  },
})
