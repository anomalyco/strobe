// Harness for chaos.bench.ts's script() cases (not shipped): the same fixed integer work as spin.ts.
export function spin(n = 300_000_000) {
  let h = 2166136261
  for (let i = 0; i < n; i++) h = Math.imul(h ^ i, 16777619)
  return h
}

// The work in a child process that exits inside the run: its CPU leaves the host's process tree.
export async function spinInChild(n = 300_000_000) {
  const p = Bun.spawn([process.execPath, new URL("spin.ts", import.meta.url).pathname.replace(/^\/(\w:)/, "$1"), String(n)], { stdout: "ignore" })
  return await p.exited
}

// Exits the host mid-run, as a crash would.
export function crash() {
  process.exit(3)
}
