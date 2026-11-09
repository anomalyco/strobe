// The script() target of http-probe.bench.ts (dev only, not shipped): an HTTP server inside the host process that
// answers GET /health only with `Authorization: Bearer <HTTP_PROBE_TOKEN>`, and exports that hold its JavaScript
// thread or change its answers. http-probe-check.ts sets the token, a fresh random one per check.
let mode: "ok" | "500" | "wrong-json" = "ok"
let server: ReturnType<typeof Bun.serve> | undefined

/** Starts the server; its address (the token in its query too, which strobe must never write) and the token. */
export function start() {
  const token = process.env.HTTP_PROBE_TOKEN
  if (!token) throw new Error("set HTTP_PROBE_TOKEN (http-probe-check.ts does)")
  server ??= Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(req) {
      if (req.headers.get("authorization") !== `Bearer ${token}`) return new Response("unauthorized", { status: 401 })
      if (mode === "500") return new Response("broken", { status: 500 })
      return Response.json({ status: mode === "wrong-json" ? "degraded" : "ok" })
    },
  })
  return { url: `http://127.0.0.1:${server.port}/health?key=${token}`, token }
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
