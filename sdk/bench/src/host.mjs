// The process a `script()` target benchmarks (SPEC §9.6): it imports the variant's module and calls
// its exports when strobe asks. Plain JavaScript, so Bun and Node both run it as is.
// Protocol: newline-delimited JSON over a loopback socket to strobe (STROBE_HOST_PORT). The process's
// own stdio is the module's: stdin is empty and stdout and stderr go to the experiment's script.log,
// so a child the module spawns (Bun's `$` passes stdin on) never sees or blocks the protocol.
import { connect } from "node:net"
import { createInterface } from "node:readline"
import { pathToFileURL } from "node:url"

const socket = connect(Number(process.env.STROBE_HOST_PORT), "127.0.0.1")
await new Promise((resolve, reject) => {
  socket.once("connect", resolve)
  socket.once("error", reject)
})
socket.setNoDelay(true)
const write = (message) => socket.write(JSON.stringify(message) + "\n")

// The runtime that runs, from its own version record and before the module loads (a module may define
// a `globalThis.Bun` of its own on Node, or replace `gc`): its collector is taken now too. Node runs
// with --expose-gc; a Node started without it (a wrapper of strobe's) exposes it through V8's flag.
const bun = typeof process.versions?.bun === "string"
const version = bun ? process.versions.bun : process.versions.node
const bunGc = bun ? globalThis.Bun?.gc : undefined
let nodeGc = bun ? undefined : globalThis.gc
if (!bun && typeof nodeGc !== "function") {
  try {
    const { setFlagsFromString } = await import("node:v8")
    const { runInNewContext } = await import("node:vm")
    setFlagsFromString("--expose-gc")
    nodeGc = runInNewContext("gc")
  } catch {}
}

const mod = await import(pathToFileURL(process.argv[2]).href)

// The runtime's monotonic clock (µs): the one Node's CPU profile counts in, and Bun's
// `performance.now()` (Bun's profile counts in wall µs: `strobeClockMark` reads the two together).
const mono = () => {
  if (bun) return performance.now() * 1000
  const ns = process.hrtime.bigint()
  return Number(ns / 1000n) + Number(ns % 1000n) / 1000
}

// Spins at least `ms` milliseconds (attribution blocks only, twice a block), saying when on the
// monotonic clock, and until it has read the
// wall clock at a Date.now() millisecond tick it caught tightly: two Date.now() calls a millisecond
// apart, under 50 µs apart on the monotonic clock, so the tick (between them) is known within 25 µs.
// A tick seen after the thread was off a CPU (the calls far apart, or two milliseconds in one step)
// is skipped. Without a tight tick in 20 ms, wall_us is 0 and strobe refuses the profile. The samples
// the CPU profile takes of this function show where the profile puts a moment strobe also timed.
function strobeClockMark(ms) {
  const first = mono()
  const start = performance.now()
  let wall = 0
  let at = 0
  let before = mono()
  let d0 = Date.now()
  for (;;) {
    const spun = performance.now() - start
    if (spun >= ms && (wall || spun >= 20)) break
    if (wall) continue
    const b = mono()
    const d = Date.now()
    const a = mono()
    // The tick lies between the last call (after `before`) and this one (before `a`).
    if (d === d0 + 1 && a - before < 50) {
      wall = d * 1000
      at = (before + a) / 2
    }
    before = b
    d0 = d
  }
  // The spin's own span on the monotonic clock: where its samples must lie.
  return { wall_us: wall, wall_mono_us: at, spin_mono_us: [first, mono()] }
}

createInterface({ input: socket }).on("line", async (line) => {
  if (!line.trim()) return
  const { id, op, name, args, mark } = JSON.parse(line)
  if (op === "clock") {
    const edge = mark ? strobeClockMark(5) : {}
    write({ id, ok: { ...edge, mono_us: mono(), runtime: bun ? "bun" : "node", version } })
    return
  }
  // `js.heap.retained`: the heap after a full collection, between runs, buffer contents included, once.
  // Bun's `heapUsed` counts every buffer's bytes already, and its `arrayBuffers` repeats them whenever an
  // ArrayBuffer object exists (`new ArrayBuffer`, a typed array's `.buffer` read, `arrayBuffer()`): the
  // sum read 200 MiB for 100 MiB. Node's `heapUsed` counts none of them and `arrayBuffers` all of them,
  // once a second collection after a turn of the event loop has freed the backing stores the first one
  // left (Response.arrayBuffer()'s copies read 200-300 MiB for 100 MiB after one collection).
  // On Bun the figure is what `Bun.gc(true)` returns: the heap's size after that full collection. Before
  // Bun 1.4.0 `heapUsed` was the size after the last eden collection, which a full one never updated
  // (0, or bytes the collection freed); from 1.4.0 the two are the same number. Without a collector to
  // call the heap would hold uncollected garbage: refused, never read.
  if (op === "heap") {
    let used
    if (bun) {
      used = typeof bunGc === "function" ? bunGc(true) : undefined
      if (typeof used !== "number") {
        write({ id, error: `js.heap.retained: Bun ${version}'s Bun.gc(true) gave no heap size, so the heap after a collection cannot be read` })
        return
      }
    } else {
      if (typeof nodeGc !== "function") {
        write({ id, error: `js.heap.retained: Node ${version} gives no garbage collector to call (no --expose-gc), so the heap would hold uncollected garbage` })
        return
      }
      nodeGc()
      await new Promise((r) => setImmediate(r))
      nodeGc()
      const m = process.memoryUsage()
      used = m.heapUsed + (m.arrayBuffers ?? 0)
    }
    write({ id, ok: used })
    return
  }
  if (op === "exit") {
    write({ id, ok: true })
    // A normal exit, so --cpu-prof and --heap-prof write their profiles.
    setTimeout(() => process.exit(0), 10)
    return
  }
  try {
    const fn = name.split(".").reduce((o, k) => o?.[k], mod)
    if (typeof fn !== "function") throw new Error(`${name} is not a function exported by ${process.argv[2]}`)
    const value = await fn(...(args ?? []))
    write({ id, ok: value === undefined ? null : value })
  } catch (error) {
    write({ id, error: String(error?.stack ?? error) })
  }
})
// strobe going away ends the host.
socket.on("close", () => process.exit(0))

// The runtime that actually runs (a `node` on PATH may be Bun's shim): strobe reads it, not the declared one.
write({ ready: true, pid: process.pid, runtime: bun ? "bun" : "node" })
