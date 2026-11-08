// The fixture of derive.bench.ts (dev only, not shipped): serves this folder's pages on 127.0.0.1:<port>,
// and /hop, a 302 to /payload (10 000 bytes), for the network counts' redirects.
const [port] = process.argv.slice(2)
const dir = import.meta.dir
// 127.0.0.2 too: a third site (localhost is the second), all on the loopback.
for (const hostname of ["127.0.0.1", "127.0.0.2"]) Bun.serve({
  hostname,
  port: Number(port),
  idleTimeout: 0,
  fetch(req) {
    const path = new URL(req.url).pathname
    // Never answered: each page holds a connection to the fixture for as long as it is open, which the
    // fixture's connection check reads at every run's edges.
    if (path === "/hold") return new Promise<Response>(() => {})
    if (path === "/hop") return new Response(null, { status: 302, headers: { location: "/payload", "cache-control": "no-store" } })
    if (path === "/payload") return new Response("x".repeat(10_000), { headers: { "content-type": "text/plain", "cache-control": "no-store" } })
    const file = Bun.file(`${dir}${path === "/" ? "/a.html" : path}`)
    return new Response(file, { headers: { "cache-control": "no-store" } })
  },
})
console.log(`listening on ${port}`)
