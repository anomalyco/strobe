// Known-answer bench for the HTTP probe (SPEC §8.9; dev only, not shipped). http-probe-server.ts serves GET /health in
// the script host's own thread, so a stall of that thread holds every request. Run it through the check, which makes a
// token (HTTP_PROBE_TOKEN_FILE, its hash for the server) and greps strobe's files for it afterwards:
//   bun scripts/dev/http-probe-check.ts [path/to/strobe]
// Truth, a probe every 100 ms:
//   stall-750ms       the thread is held 750 ms: the ~8 probes due then form one cohort, the first ~4 timing out at
//                     400 ms and the rest answered as the stall ends (≈350, 250, 150, 50 ms); never one slow sample
//                     and a gap. p95 and max are right-censored (refused), never 400.
//   stall-750ms-long  the same with a 1500 ms timeout: no timeout, the cohort's slowest ≈750 ms.
//   stalls-100ms-apart  two 550 ms stalls of the thread 100 ms apart, a 1000 ms timeout: the server answers what
//                     the first held in the gap, so query http shows two cohorts, each "answered none for" ≈550 ms
//                     (a probe that connects as the second begins may wait it out too: it opens the second).
//   stalls-back-to-back  the same with no gap: the loop reads no request between the stalls, so the server answers
//                     none for ≈1.1 s, one cohort that says so; the first probe times out at 1000 ms, first in it.
// The held cases' server holds each request in the bench file's own process until the window it came in ends (a
// stall without a busy thread), two 550 ms windows 150 ms apart, so the next two say exactly what a stalled loop
// does to a client that connects as a stall ends (accept and read in separate turns: it waits out the next one):
//   held-gap-carried  a request in the gap waits for the second window's end: everything the first held was
//                     answered before that probe was sent, so two cohorts (the second opened by it, "answered
//                     none for" ≈700 ms), never one "answered none for" ≈1.25 s.
//   held-edge-carried also a request in the first window's last 100 ms waits for the second's end, with a 700 ms
//                     timeout: one cohort that "spanned 2 stalls" (longest silence ≈700 ms; 1 probe, the edge one,
//                     waited across a brief drain), which timed out in its middle and is said so.
//   driver-delayed    no stall: strobe's own scheduler is held 400 ms (STROBE_PROBE_PAUSE): missed sends, no
//                     timeouts, and http.latency.* refused OverheadBudget, never a slow server.
//   status-500        the server answers 500 for 300 ms: http.errors, and http.latency.* refused.
//   wrong-json        the server answers {"status":"degraded"} for 300 ms where expect asks for "ok": the same.
//   mixed             cpu.work from clean timing pairs, http.* from a probe pass after them (timed runs only).
//   page              a Chromium page beside a server in the bench file's own process: its attribution block
//                     (record --bench --headless) takes no heap snapshot, since the probe carries a credential.
import { bench, script, url } from "@hona/strobe"

const all = ["http.latency.p50", "http.latency.p95", "http.latency.max", "http.timeouts", "http.errors", "http.missed_sends", "http.probes", "http.idle.p50"]
// A primary that is never refused, so a check keeps the case's values beside what it refused.
const declare = (primary: string) => ({ targets: ["server"], primary, metrics: all.filter((m) => m !== primary) })
const stall = async ({ call }: any) => {
  await call("idle", 300)
  await call("stall", 750)
  await call("idle", 500)
}
const stalls = (gap: number) => async ({ call }: any) => {
  await call("idle", 300)
  await call("stall", 550)
  if (gap) await call("idle", gap)
  await call("stall", 550)
  await call("idle", 500)
}
const answering = (mode: string) => async ({ call }: any) => {
  await call("idle", 300)
  await call("setMode", mode)
  await call("idle", 300)
  await call("setMode", "ok")
  await call("idle", 300)
}

// The page case's server, in this (the runner's) process, by the same rule as http-probe-server.ts.
let here: ReturnType<typeof Bun.serve> | undefined
const serveHere = () => {
  const expected = process.env.HTTP_PROBE_TOKEN_SHA256 ?? ""
  here ??= Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch: (req) => (new Bun.CryptoHasher("sha256").update(req.headers.get("authorization") ?? "").digest("hex") === expected ? Response.json({ status: "ok" }) : new Response("unauthorized", { status: 401 })),
  })
  return `http://127.0.0.1:${here.port}/health`
}

// The held cases' server, in this process: a request is answered when the window it came in ends (performance.now()
// ms), or the next window's end when it came in the gap (`gap`) or the window's last 100 ms (`edge`); else at once.
let heldServer: ReturnType<typeof Bun.serve> | undefined
let windows: [number, number][] = []
let carry = { gap: false, edge: false }
const releaseAt = (now: number) => {
  for (const [i, [a, b]] of windows.entries()) {
    const next = windows[i + 1]
    if (now >= a && now < b) return next && carry.edge && now >= b - 100 ? next[1] : b
    if (next && now >= b && now < next[0]) return carry.gap ? next[1] : now
  }
  return now
}
const serveHeld = () => {
  const expected = process.env.HTTP_PROBE_TOKEN_SHA256 ?? ""
  heldServer ??= Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(req) {
      if (new Bun.CryptoHasher("sha256").update(req.headers.get("authorization") ?? "").digest("hex") !== expected) return new Response("unauthorized", { status: 401 })
      const now = performance.now()
      const at = releaseAt(now)
      if (at > now) await Bun.sleep(at - now)
      return Response.json({ status: "ok" })
    },
  })
  return `http://127.0.0.1:${heldServer.port}/health`
}
const held = (gap: boolean, edge: boolean) => async () => {
  const t = performance.now()
  carry = { gap, edge }
  windows = [
    [t + 300, t + 850],
    [t + 1000, t + 1550],
  ]
  await Bun.sleep(2050)
  windows = []
}

export default bench({
  targets: { server: script("./http-probe-server.ts"), page: url(new URL("pages/click.html", import.meta.url).href) },
  async setup(ctx) {
    const url = ctx.target === "page" ? serveHere() : ctx.case?.startsWith("held-") ? serveHeld() : await ctx.call<string>("start")
    // The bench file reads the token; the server knows only its hash. It goes in the URL's query too, which strobe
    // sends and must never write, and this hook prints it, which the experiment's runner.log must not keep.
    const token = (await Bun.file(process.env.HTTP_PROBE_TOKEN_FILE ?? "").text()).trim()
    console.log(`probing ${url}?key=${token}`)
    await ctx.httpProbe({
      url: `${url}?key=${token}`,
      authorization: `Bearer ${token}`,
      cadenceMs: 100,
      timeoutMs: { "stall-750ms-long": 1500, "stalls-100ms-apart": 1000, "stalls-back-to-back": 1000, "held-gap-carried": 1000, "held-edge-carried": 700 }[ctx.case ?? ""] ?? 400,
      expect: ctx.case === "wrong-json" ? { jsonPath: { path: "$.status", equals: "ok" } } : undefined,
    })
  },
  reset: ({ call }: any) => call("setMode", "ok"),
  cases: {
    "stall-750ms": { ...declare("http.timeouts"), run: stall },
    "stall-750ms-long": { ...declare("http.latency.p50"), run: stall },
    "stalls-100ms-apart": { ...declare("http.timeouts"), run: stalls(100) },
    "stalls-back-to-back": { ...declare("http.timeouts"), run: stalls(0) },
    "held-gap-carried": { ...declare("http.timeouts"), run: held(true, false) },
    "held-edge-carried": { ...declare("http.timeouts"), run: held(true, true) },
    "driver-delayed": { ...declare("http.missed_sends"), run: ({ call }: any) => call("idle", 1500) },
    "status-500": { ...declare("http.errors"), run: answering("500") },
    "wrong-json": { ...declare("http.errors"), run: answering("wrong-json") },
    // Timed only (the orchestrator's A/A, `--aa --pairs 6`): clean pairs for cpu.work, then a probe pass for
    // http.* (`pass.http`, `i HttpProbe`); stall-750ms-long alone pairs probe blocks only.
    mixed: { targets: ["server"], primary: "cpu.work", metrics: ["http.latency.p50", "http.timeouts"], run: stall },
    page: { targets: ["page"], primary: "http.latency.p50", metrics: ["http.errors"], run: ({ page }: any) => page.wait(1500), reset: async () => {} },
  },
})
