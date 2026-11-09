// The server example of sdk/bench/README.md and the skill, runnable (dev only, not shipped): the README's
// perf/server.bench.ts with its harness, perf/server.ts, as http-probe-server.ts (the same `start` and `stall`).
//   strobe bench scripts/dev/http-probe-example.bench.ts --head . --check
// Truth: a run holds the server's thread four times for 550 ms, 100 ms apart (about 2.6 s); the probe's defaults (one
// every 100 ms, a new connection each, 1000 ms timeout) send about 26 in it, each stall's 5 or 6 held and answered as
// it ends (550 ms down to about 50), so http.latency.p95 is about 500 ms with no timeout, cpu.main about 2.2 s and
// cpu.main.busy about 85%.
import { bench, script } from "@hona/strobe"

export default bench({
  targets: { server: script("./http-probe-server.ts") },
  async setup(ctx) {
    // The server gets the token's hash only; strobe keeps `authorization` in memory, never in a file.
    const token = crypto.randomUUID()
    const url = await ctx.call<string>("start", new Bun.CryptoHasher("sha256").update(`Bearer ${token}`).digest("hex"))
    await ctx.httpProbe({ url, authorization: `Bearer ${token}`, expect: { jsonPath: { path: "$.status", equals: "ok" } } })
  },
  cases: {
    "slow handler": {
      primary: "http.latency.p95",
      metrics: ["cpu.main", "cpu.main.busy", "http.timeouts"],
      // Four slow requests 100 ms apart: about 26 probes at the default 100 ms, enough for a p95 in a --check. Back to
      // back, the next stall can start before the server answers what the last one held, and a probe waits out both.
      run: async (ctx) => {
        for (let i = 0; i < 4; i++) {
          await ctx.call("stall", 550)
          await Bun.sleep(100)
        }
      },
    },
  },
})
