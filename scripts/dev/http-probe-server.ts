// The script() target of http-probe.bench.ts (dev only, not shipped): an HTTP server inside the host process that
// answers GET /health only to the bearer token whose SHA-256 is HTTP_PROBE_TOKEN_SHA256, and exports that hold its
// JavaScript thread or change its answers. The host never holds the token itself: an attribution block keeps its heap
// profile, which must not hold the token either (http-probe-check.ts reads every file for it).
let mode: "ok" | "500" | "wrong-json" = "ok"
let server: ReturnType<typeof Bun.serve> | undefined

const sha256 = (s: string) => new Bun.CryptoHasher("sha256").update(s).digest("hex")

/** Starts the server; its address. The token's hash comes from the bench file (http-probe-example.bench.ts), else the environment. */
export function start(tokenHash?: string) {
  const expected = tokenHash ?? process.env.HTTP_PROBE_TOKEN_SHA256
  if (!expected) throw new Error("set HTTP_PROBE_TOKEN_SHA256 (http-probe-check.ts does)")
  server ??= Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(req) {
      if (sha256(req.headers.get("authorization") ?? "") !== expected) return new Response("unauthorized", { status: 401 })
      if (mode === "500") return new Response("broken", { status: 500 })
      return Response.json({ status: mode === "wrong-json" ? "degraded" : "ok" })
    },
  })
  return `http://127.0.0.1:${server.port}/health`
}

export function setMode(m: typeof mode) {
  mode = m
  return true
}

/** Holds this thread (the server's event loop) for `ms`: no request is answered meanwhile. */
export function stall(ms: number) {
  const end = performance.now() + ms
  let x = 0
  while (performance.now() < end) x += Math.sqrt(x + 1)
  return x > 0
}

/** Waits `ms` with the event loop free: requests are answered at once. */
export async function idle(ms: number) {
  await Bun.sleep(ms)
  return true
}
