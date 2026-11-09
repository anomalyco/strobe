// The server example of sdk/bench/README.md and the skill, runnable (dev only, not shipped): the README's
// perf/server.bench.ts with its harness, perf/server.ts, as http-probe-server.ts (the same `start` and `stall`).
//   strobe bench scripts/dev/http-probe-example.bench.ts --head . --check
// Truth: one 500 ms run holds the server's thread; the ~25 probes due then (every 20 ms) form one cohort answered as
// the stall ends, 500 ms down to about 20, so http.latency.p95 ≈ 475 ms, no timeout (1000 ms), cpu.main ≈ 500 ms and
// cpu.main.busy near 100%.
import { bench, script } from "@hona/strobe"

export default bench({
  targets: { server: script("./http-probe-server.ts") },
  async setup(ctx) {
    // The server gets the token's hash only; strobe keeps `authorization` in memory, never in a file.
    const token = crypto.randomUUID()
    const url = await ctx.call<string>("start", new Bun.CryptoHasher("sha256").update(`Bearer ${token}`).digest("hex"))
    await ctx.httpProbe({
      url,
      authorization: `Bearer ${token}`,
      cadenceMs: 20,
      expect: { jsonPath: { path: "$.status", equals: "ok" } },
    })
  },
  cases: {
    "slow handler": {
      primary: "http.latency.p95",
      metrics: ["cpu.main", "cpu.main.busy", "http.timeouts"],
      run: (ctx) => ctx.call("stall", 500),
    },
  },
})
