// The script() target of client.bench.ts (dev only, not shipped): an HTTP server on the loopback, started at its
// import. `port` says where; each request costs the server about `work` µs of CPU.
let work = 200
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch() {
    const end = performance.now() + work / 1000
    let x = 0
    while (performance.now() < end) x += Math.sqrt(x + 1)
    return new Response(x > 0 ? "ok" : "")
  },
})

export function port() {
  return server.port
}

export function cost(us: number) {
  work = us
  return true
}

export function ping() {
  return "pong"
}
