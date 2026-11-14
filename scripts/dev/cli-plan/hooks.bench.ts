// The two SDK entry points use the same planner. Catching a refusal cannot validate the first leg.
import { bench, cli } from "@hona/strobe"
const plain = "bun --version"
const lingering = process.platform === "win32" ? 'cmd /c "start /b bun scripts/dev/leftover/child.mjs"' : "sh -c 'bun scripts/dev/leftover/child.mjs >/dev/null 2>&1 &'"
export default bench({
  targets: { program: cli(plain) },
  cases: {
    default: { primary: "run.duration", run: (ctx) => ctx.exec() },
    usage: { primary: "run.duration", run: async (ctx) => { await ctx.exec(); await ctx.exec("bun --version &").catch(() => {}) } },
    leftover: { primary: "run.duration", run: async (ctx) => { await ctx.exec(); await ctx.exec(lingering).catch(() => {}) } },
  },
})
