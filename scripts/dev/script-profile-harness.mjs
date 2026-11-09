// The script() target of script-profile.bench.ts (dev only, not shipped). Plain JavaScript, so Bun and Node
// both load it. Each export burns CPU in a function of its own name, so a profile says where each went.
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { pathToFileURL } from "node:url"

/** `ms` of wall time busy on this thread. */
function burn(ms) {
  const end = performance.now() + ms
  let x = 0
  while (performance.now() < end) x += Math.sqrt(x + 1)
  return x
}

// A host that loads many modules before its runs: 100 modules, each spending 2 ms in `moduleLoadWork` as it is
// evaluated. Only the whole host's profile may hold them.
const dir = join(tmpdir(), `strobe-script-profile-${process.pid}`)
mkdirSync(dir, { recursive: true })
for (let i = 0; i < 100; i++) {
  writeFileSync(
    join(dir, `m${i}.mjs`),
    `function moduleLoadWork() { const end = performance.now() + 2; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 1); return x }\nexport const value = moduleLoadWork() + ${i}\n`,
  )
}
for (let i = 0; i < 100; i++) await import(pathToFileURL(join(dir, `m${i}.mjs`)).href)
rmSync(dir, { recursive: true, force: true })

export function burnSetup(ms) {
  return burn(ms) > 0
}

export function burnRunPhaseA(ms) {
  return burn(ms) > 0
}

export function burnRunPhaseB(ms) {
  return burn(ms) > 0
}

export function burnUntimed(ms) {
  return burn(ms) > 0
}

export function burnReset(ms) {
  return burn(ms) > 0
}

/** Waits `ms` without running JavaScript (Bun's sampler takes no sample meanwhile). */
export function idle(ms) {
  return new Promise((resolve) => setTimeout(() => resolve(true), ms))
}

let calls = 0

/** Burns `ms`, and throws on its fourth call: after warmup, in a measured run. */
export function failMidRun(ms) {
  burn(ms)
  calls += 1
  if (calls === 4) throw new Error("failMidRun: the fourth call fails")
  return calls
}
