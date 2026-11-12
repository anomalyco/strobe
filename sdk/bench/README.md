# @hona/strobe

[strobe](https://github.com/anomalyco/strobe): the referee for a performance claim in a pull request. This package is
the `strobe` command and the types and helpers for its bench files, so a project pins one strobe version for everyone
who runs its benches, people, agents and CI alike.

Two commands, compared in alternating pairs, with a verdict and its interval (no install needed):

```sh
bunx @hona/strobe run --base "<command on main>" --head "<command on your branch>"
```

A project's first bench:

```sh
bun add -d @hona/strobe
bunx strobe init                                          # strobe.json and a runnable perf/smoke.bench.ts
bunx strobe bench perf/smoke.bench.ts --head . --check    # each case once: does it work?
```

`strobe init` reads the folder's package.json (`build`, `preview`, `start`, `dev`, `exports`, `module`,
`types`, `main`, `bin`, an `electron` dependency), Cargo.toml binaries or index.html, and writes a bench
that passes `--check` as written: `--target web|lib|server|native|electron|tty|cli` picks the kind. It
never replaces a file without `--force`.
Then edit its cases to what your users do, or write your own:

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
      // Four slow requests 100 ms apart: about 26 probes a run, enough for a p95 in a --check.
      run: async (ctx) => {
        for (let i = 0; i < 4; i++) {
          await ctx.call("stall", 550)
          await Bun.sleep(100)
        }
      },
    },
  },
})
```

```sh
bunx strobe bench perf/server.bench.ts --head . --check     # http.latency.p95 ≈ 500,000 µs: the held cohorts
bunx strobe record --bench "perf/server.bench.ts#slow handler" --head .   # then: strobe query <rec> http
```

`http.latency.p95` is what a client waited while the runs held the server; `cpu.main` is the server's main
thread alone (`cpu.main.busy` near 100%: its event loop never waited). Times print in microseconds; `--unit ms`
or `"unit": "ms"` in strobe.json prints milliseconds.

A native app (`native(…, { output: true })`) that prints a line when it is ready: wait for it in setup, so the first run does not measure the
launch's work. `since: "launch"` also takes a line the app wrote before the call (strobe keeps its output's last
2,000 lines); the default takes only lines written after it.

```ts
async setup(ctx) {
  await ctx.waitForLine(/listening on/, { since: "launch", timeout: 60_000 })
},
```

In GitHub Actions, strobe's action runs the same check on every pull request: each case once, and with `noworse`
each exact counter held to the base commit's. `version: project` runs the strobe this package pins.

```yaml
- uses: anomalyco/strobe@v0.29
  with:
    bench: perf/settings.bench.ts
    noworse: net.requests=0,dom.nodes=0
    version: project
```

Timing verdicts need a machine of your own: `mode: compare` on a self-hosted runner (a GitHub-hosted runner's
timings move with the jobs beside it, so strobe refuses them there). Every input is in the action's
[action.yml](https://github.com/anomalyco/strobe/blob/main/action.yml).

- **Platforms:** the binary comes from `@hona/strobe-<platform>-<arch>`, installed with this package for your
  machine only: Windows x64 and Arm64, macOS Apple silicon and Intel, Linux x64 and Arm64 (static, glibc or
  musl).
- **One SDK per binary:** strobe runs a bench file with the SDK it was built with, so the types you import and
  the runner always match.
- **Old name:** `@hona/strobe-bench` re-exports this package. Import `@hona/strobe` instead.
- **Changes:** each release's notes are on [GitHub Releases](https://github.com/anomalyco/strobe/releases).
