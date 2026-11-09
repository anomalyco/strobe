// Known-answer bench for the HTTP probe (SPEC §8.9; dev only, not shipped). http-probe-server.ts serves GET /health in
// the script host's own thread, so a stall of that thread holds every request. Run it through the check, which sets the
// server's token and greps strobe's files for it afterwards:
//   bun scripts/dev/http-probe-check.ts [path/to/strobe]
// Truth, a probe every 100 ms:
//   stall-750ms       the thread is held 750 ms: the ~8 probes due then form one cohort, the first ~4 timing out at
//                     400 ms and the rest answered as the stall ends (≈350, 250, 150, 50 ms); never one slow sample
//                     and a gap. p95 and max are right-censored (refused), never 400.
//   stall-750ms-long  the same with a 1500 ms timeout: no timeout, the cohort's slowest ≈750 ms.
//   driver-delayed    no stall: strobe's own scheduler is held 400 ms (STROBE_PROBE_PAUSE): missed sends, no
//                     timeouts, and http.latency.* refused OverheadBudget, never a slow server.
//   status-500        the server answers 500 for 300 ms: http.errors, and http.latency.* refused.
//   wrong-json        the server answers {"status":"degraded"} for 300 ms where expect asks for "ok": the same.
import { bench, script } from "@hona/strobe"

const all = ["http.latency.p50", "http.latency.p95", "http.latency.max", "http.timeouts", "http.errors", "http.missed_sends", "http.probes", "http.idle.p50"]
// A primary that is never refused, so a check keeps the case's values beside what it refused.
const declare = (primary: string) => ({ primary, metrics: all.filter((m) => m !== primary) })
const stall = async ({ call }: any) => {
  await call("idle", 300)
  await call("stall", 750)
  await call("idle", 500)
}
const answering = (mode: string) => async ({ call }: any) => {
  await call("idle", 300)
  await call("setMode", mode)
  await call("idle", 300)
  await call("setMode", "ok")
  await call("idle", 300)
}

export default bench({
  targets: { server: script("./http-probe-server.ts") },
  async setup(ctx) {
    const { url, token } = await ctx.call<{ url: string; token: string }>("start")
    await ctx.httpProbe({
      url,
      authorization: `Bearer ${token}`,
      cadenceMs: 100,
      timeoutMs: ctx.case === "stall-750ms-long" ? 1500 : 400,
      expect: ctx.case === "wrong-json" ? { jsonPath: { path: "$.status", equals: "ok" } } : undefined,
    })
  },
  reset: ({ call }: any) => call("setMode", "ok"),
  cases: {
    "stall-750ms": { ...declare("http.timeouts"), run: stall },
    "stall-750ms-long": { ...declare("http.latency.p50"), run: stall },
    "driver-delayed": { ...declare("http.missed_sends"), run: ({ call }: any) => call("idle", 1500) },
    "status-500": { ...declare("http.errors"), run: answering("500") },
    "wrong-json": { ...declare("http.errors"), run: answering("wrong-json") },
  },
})
