// The script() target of derive.bench.ts (dev only, not shipped): CPU of known size, on the host's own thread
// or in a child process that ends inside the call.

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
