// The script() target of script.bench.ts (dev only, not shipped): exports whose costs are known by construction.
// keep and clear also run on Node (script.bench.ts's `node` target); the rest use Bun's APIs.
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const bin = process.env.KNOWN_BIN ?? join(dirname(fileURLToPath(import.meta.url)), process.platform === "win32" ? "known.exe" : "known")
let kept: Uint8Array[] = []

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
  const b = new Uint8Array(mb * 1024 * 1024)
  for (let i = 0; i < b.length; i += 4096) b[i] = 1
  kept.push(b)
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
  await Bun.write(path, new Uint8Array(mb * 1024 * 1024))
  const n = (await Bun.file(path).arrayBuffer()).byteLength
  await Bun.file(path).delete()
  return n
}
