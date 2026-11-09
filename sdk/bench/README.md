# @hona/strobe

[strobe](https://github.com/anomalyco/strobe): measure and explain performance. This package is the `strobe` command
and the types and helpers for its bench files, so a project pins one strobe version for everyone who runs its
benches, people, agents and CI alike.

```sh
bun add -d @hona/strobe
```

```ts
import { bench, web } from "@hona/strobe"

export default bench({
  targets: { app: web({ url: "/" }) },
  cases: {
    "open settings": {
      primary: "main.work",
      run: (ctx) => ctx.page.click("[data-action=settings]", { until: "[role=dialog]" }),
      reset: (ctx) => ctx.page.press("Escape", { until: "idle" }),
    },
  },
})
```

```sh
bunx strobe bench perf/settings.bench.ts --check            # each case once: does it work?
bunx strobe bench perf/settings.bench.ts --base git:main    # is this branch faster?
```

A server: host it in a `script()` target and probe it from strobe's own threads while the runs load it. The
harness is a module strobe loads in a fresh Bun process per block; cases call its exports with `ctx.call`.

```ts
// perf/server.ts
let server: ReturnType<typeof Bun.serve> | undefined

/** Starts the server; its address. It answers only the bearer token whose SHA-256 is `tokenHash`. */
export function start(tokenHash: string) {
  const hash = (s: string) => new Bun.CryptoHasher("sha256").update(s).digest("hex")
  server ??= Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch: (req) =>
      hash(req.headers.get("authorization") ?? "") === tokenHash
        ? Response.json({ status: "ok" })
        : new Response("unauthorized", { status: 401 }),
  })
  return `http://127.0.0.1:${server.port}/health`
}

/** Holds the server's thread for `ms`, as a slow synchronous handler would: no request is answered meanwhile. */
export function stall(ms: number) {
  const end = performance.now() + ms
  let x = 0
  while (performance.now() < end) x += Math.sqrt(x + 1)
  return x > 0
}
```

```ts
// perf/server.bench.ts
import { bench, script } from "@hona/strobe"

export default bench({
  targets: { server: script("./server.ts") },
  async setup(ctx) {
    // The server gets the token's hash only; strobe keeps `authorization` in memory, never in a file.
    const token = crypto.randomUUID()
    const url = await ctx.call<string>("start", new Bun.CryptoHasher("sha256").update(`Bearer ${token}`).digest("hex"))
    // A probe every 100 ms, a new connection each (the defaults): a backlog stall shows too.
    await ctx.httpProbe({ url, authorization: `Bearer ${token}`, expect: { jsonPath: { path: "$.status", equals: "ok" } } })
  },
  cases: {
    "slow handler": {
      primary: "http.latency.p95",
      metrics: ["cpu.main", "cpu.main.busy", "http.timeouts"],
      // Four slow requests: about 22 probes in a run, enough for a p95 in a --check.
      run: async (ctx) => {
        for (let i = 0; i < 4; i++) await ctx.call("stall", 550)
      },
    },
  },
})
```

```sh
bunx strobe bench perf/server.bench.ts --head . --check     # http.latency.p95 ≈ 500 ms: the held cohorts
bunx strobe record --bench "perf/server.bench.ts#slow handler" --head .   # then: strobe query <rec> http
```

`http.latency.p95` is what a client waited while the runs held the server; `cpu.main` is the server's main
thread alone (`cpu.main.busy` near 100%: its event loop never waited).

- **Platforms:** the binary comes from `@hona/strobe-<platform>-<arch>`, installed with this package for your
  machine only: Windows x64 and Arm64, macOS Apple silicon and Intel, Linux x64 and Arm64 (static, glibc or
  musl).
- **One SDK per binary:** strobe runs a bench file with the SDK it was built with, so the types you import and
  the runner always match.
- **Old name:** `@hona/strobe-bench` re-exports this package. Import `@hona/strobe` instead.
