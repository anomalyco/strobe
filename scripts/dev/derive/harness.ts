// The script() target of derive.bench.ts (dev only, not shipped): CPU of known size, on the host's own thread
// or in a child process that ends inside the call.
import { join } from "node:path"

/** `ms` of wall time busy on this thread. */
export function spin(ms: number) {
  const end = performance.now() + ms
  let x = 0
  while (performance.now() < end) x += Math.sqrt(x + 1)
  return x > 0
}

/** A child process that spins `ms` and exits before the call returns. */
export function child(ms: number) {
  return Bun.spawnSync([process.execPath, "-e", `const e = performance.now() + ${ms}; while (performance.now() < e) {}`]).exitCode
}

/** known's `breakaway`: a child (scripts/dev/known, built first) whose own child spins `ms` and asks to leave the
 * job it starts in (Windows: CREATE_BREAKAWAY_FROM_JOB), through `hops` such children each asking again; all
 * end before the call returns. Throws where a hop could not start. */
export function breakaway(ms: number, hops = 1) {
  const known = join(import.meta.dir, "..", "known", process.platform === "win32" ? "known.exe" : "known")
  const code = Bun.spawnSync([known, "breakaway", String(ms), String(hops)]).exitCode
  if (code !== 0) throw new Error(`known breakaway exited ${code}`)
  return code
}

/** A child that starts a grandchild spinning `ms`, says its pid and exits at once: the grandchild is orphaned
 * (adopted and reaped outside the host's tree). Its pid. */
function grandchild(ms: number) {
  const spin = `const e = performance.now() + ${ms}; while (performance.now() < e) {}`
  const child = `const p = Bun.spawn([process.execPath, "-e", ${JSON.stringify(spin)}], { stdio: ["ignore", "ignore", "ignore"] }); p.unref(); console.log(p.pid)`
  return Number(Bun.spawnSync([process.execPath, "-e", child]).stdout.toString().trim())
}

function alive(pid: number) {
  try {
    return pid > 0 && process.kill(pid, 0)
  } catch {
    return false
  }
}

/** `grandchild`, which ends before the call returns (the call waits for it). */
export function orphan(ms: number) {
  const pid = grandchild(ms)
  while (alive(pid)) Bun.sleepSync(1)
  return pid > 0
}

let lingering = 0

/** `grandchild`, still spinning when the call returns: the run ends with it running outside the host's tree. */
export function outlive(ms: number) {
  lingering = grandchild(ms)
  return lingering > 0
}

/** Waits for `outlive`'s grandchild to end. */
export function settle() {
  while (alive(lingering)) Bun.sleepSync(1)
  return true
}
