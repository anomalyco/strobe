// The script() target of script.bench.ts (dev only, not shipped): exports whose costs are known by construction.
// keep, hold and clear also run on Node (script.bench.ts's `node` target); the rest use Bun's APIs.
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const bin = process.env.KNOWN_BIN ?? join(dirname(fileURLToPath(import.meta.url)), process.platform === "win32" ? "known.exe" : "known")
let kept: unknown[] = []
const MiB = 1024 * 1024
const touch = (u8: Uint8Array) => {
  for (let i = 0; i < u8.length; i += 4096) u8[i] = 1
  return u8
}

/**
 * `mb` MiB more held in one of the shapes a module holds buffer memory in, every page written. The runtimes
 * report them differently: Bun's heap counts every buffer's bytes, and its `arrayBuffers` repeats them once an
 * ArrayBuffer object exists (all but `typed` and `nodebuffer`); Node's heap counts none, its `arrayBuffers` all.
 * `objects`: about 70-90 MiB of small objects per 1,000 (`mb` thousands of them), no buffer.
 */
export async function hold(shape: string, mb: number) {
  const n = mb * MiB
  let v: unknown
  if (shape === "typed") v = touch(new Uint8Array(n))
  else if (shape === "view") {
    const u = touch(new Uint8Array(n))
    void u.buffer
    v = u
  } else if (shape === "arraybuffer") {
    const ab = new ArrayBuffer(n)
    touch(new Uint8Array(ab))
    v = ab
  } else if (shape === "dataview") {
    const dv = new DataView(new ArrayBuffer(n))
    touch(new Uint8Array(dv.buffer))
    v = dv
  } else if (shape === "response") v = await new Response(touch(new Uint8Array(n))).arrayBuffer()
  else if (shape === "nodebuffer") v = touch(Buffer.alloc(n))
  else if (shape === "objects") {
    const a: { i: number; s: string }[] = []
    for (let i = 0; i < mb * 1000; i++) a.push({ i, s: "x" + i })
    v = a
  } else throw new Error(`hold: no shape ${shape}`)
  kept.push(v)
  return kept.length
}

/** `ms` of wall time busy on this thread: its CPU when nothing else runs on its core. */
export function spin(ms: number) {
  const end = performance.now() + ms
  let x = 0
  while (performance.now() < end) x += Math.sqrt(x + 1)
  return x > 0
}

export async function sleep(ms: number) {
  await Bun.sleep(ms)
  return true
}

/** `mb` MiB more held, every page written. */
export function keep(mb: number) {
  kept.push(touch(new Uint8Array(mb * MiB)))
  return kept.length
}

export function clear() {
  kept = []
  if (globalThis.Bun) Bun.gc(true)
  else (globalThis as any).gc?.()
  return true
}

/** A child process that runs `ms` of CPU and exits inside the call. */
export function child(ms: number) {
  const r = Bun.spawnSync([bin, "spin", String(ms)])
  return r.exitCode
}

/** Writes `mb` MiB to a file and reads it back. */
export async function io(mb: number) {
  const path = join(process.env.TEMP ?? process.env.TMPDIR ?? "/tmp", `known-script-io-${process.pid}`)
  await Bun.write(path, new Uint8Array(mb * MiB))
  const n = (await Bun.file(path).arrayBuffer()).byteLength
  await Bun.file(path).delete()
  return n
}
