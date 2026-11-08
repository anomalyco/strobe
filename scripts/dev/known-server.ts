// The known-answer pages' server (scripts/dev/known-web.bench.ts): raw HTTP over TCP, so it knows every byte it
// sends. `/` is pages/known/index.html, `/blob/<n>` is n bytes. Alone, for `strobe record`:
//   bun scripts/dev/known-server.ts <port> <seconds>     serves for that long, then exits
const page = new URL("pages/known/index.html", import.meta.url)

export interface Known {
  /** Bytes (headers included) and requests of `/blob/*` answered so far: the network's truth. */
  wire: { bytes: number; requests: number }
  stop(): void
}

export function serve(port: number): Known {
  const wire = { bytes: 0, requests: 0 }
  const flush = (socket: any) => {
    while (socket.data.out.length) {
      const n = socket.write(socket.data.out)
      if (n <= 0) return
      socket.data.out = socket.data.out.subarray(n)
    }
  }
  const respond = (socket: any, head: string) => {
    const path = head.split(" ")[1]?.split("?")[0] ?? "/"
    const blob = path.match(/^\/blob\/(\d+)$/)
    let body = new Uint8Array(0)
    let type = "application/octet-stream"
    if (blob) body = new Uint8Array(Number(blob[1])).fill(97)
    else if (path === "/" || path === "/index.html") {
      body = new Uint8Array(require("fs").readFileSync(page))
      type = "text/html; charset=utf-8"
    }
    const status = body.length || blob ? "200 OK" : "404 Not Found"
    const header = new TextEncoder().encode(`HTTP/1.1 ${status}\r\nContent-Type: ${type}\r\nContent-Length: ${body.length}\r\nCache-Control: no-store\r\n\r\n`)
    const out = new Uint8Array(header.length + body.length)
    out.set(header)
    out.set(body, header.length)
    if (blob) {
      wire.bytes += out.length
      wire.requests++
    }
    socket.data.out = socket.data.out.length ? Buffer.concat([socket.data.out, out]) : Buffer.from(out)
    flush(socket)
  }
  const server = Bun.listen<{ buf: string; out: Buffer }>({
    hostname: "127.0.0.1",
    port,
    socket: {
      open(socket) {
        socket.data = { buf: "", out: Buffer.alloc(0) }
      },
      data(socket, data) {
        socket.data.buf += data.toString("latin1")
        let end
        while ((end = socket.data.buf.indexOf("\r\n\r\n")) >= 0) {
          const head = socket.data.buf.slice(0, end)
          socket.data.buf = socket.data.buf.slice(end + 4)
          respond(socket, head)
        }
      },
      drain(socket) {
        flush(socket)
      },
    },
  })
  return { wire, stop: () => server.stop(true) }
}

if (import.meta.main) {
  const known = serve(Number(process.argv[2] ?? 4824))
  setTimeout(() => {
    known.stop()
    process.exit(0)
  }, 1000 * Number(process.argv[3] ?? 60))
}
