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

const mod = await import(pathToFileURL(process.argv[2]).href)

// The runtime's monotonic clock (µs): the one Node's CPU profile counts in, and Bun's
// `performance.now()` (Bun's profile counts in wall µs: `strobeClockMark` reads the two together).
const bun = !!globalThis.Bun
const mono = () => {
  if (bun) return performance.now() * 1000
  const ns = process.hrtime.bigint()
  return Number(ns / 1000n) + Number(ns % 1000n) / 1000
}

// Spins at least `ms` milliseconds (attribution blocks only, twice a block) and until it has read the
// wall clock at a Date.now() millisecond tick it caught tightly: two Date.now() calls a millisecond
// apart, under 50 µs apart on the monotonic clock, so the tick (between them) is known within 25 µs.
// A tick seen after the thread was off a CPU (the calls far apart, or two milliseconds in one step)
// is skipped. Without a tight tick in 20 ms, wall_us is 0 and strobe refuses the profile. The samples
// the CPU profile takes of this function show where the profile puts a moment strobe also timed.
function strobeClockMark(ms) {
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
  return { wall_us: wall, wall_mono_us: at }
}

createInterface({ input: socket }).on("line", async (line) => {
  if (!line.trim()) return
  const { id, op, name, args, mark } = JSON.parse(line)
  if (op === "clock") {
    const edge = mark ? strobeClockMark(5) : {}
    write({ id, ok: { ...edge, mono_us: mono(), runtime: bun ? "bun" : "node", version: bun ? Bun.version : process.versions.node } })
    return
  }
  // `js.heap.retained`: the heap after a full collection, between runs.
  if (op === "heap") {
    if (globalThis.Bun) Bun.gc(true)
    else globalThis.gc?.()
    write({ id, ok: process.memoryUsage().heapUsed })
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

write({ ready: true, pid: process.pid })
