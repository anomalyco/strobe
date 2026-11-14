import { spawnSync } from "node:child_process"
import { basename } from "node:path"

// The Rust driver explicitly creates a console (invisible) or detaches, then checks GetConsoleCP.
// Do not infer any of these states from stdout.isTTY or from the shell running this audit.
export function parents(worker: string, entries: string[], reference = false) {
  let bad = 0
  for (const state of ["attached", "none", "script"]) for (const exe of entries) {
    const args = ["run", "--check", "--head", "bun -e 0", "--power", "keep", "--reserve-cpus", "off", "--json"]
    const from = state === "script" ? ["attached", process.execPath, import.meta.path, exe, ...args] : [state, exe, ...args]
    const r = spawnSync(worker, ["parent", ...from], { encoding: "utf8", windowsHide: true, timeout: 60000 })
    const result = JSON.parse(r.stdout), row = result.series[0]
    const ok = r.status === 0 && row.passed || reference && r.status === 1 && /still running: conhost\.exe \(pid \d+\)/i.test(row.error)
    console.log(`${ok ? "ok" : "BAD"} ${state} -> ${basename(exe)}: ${r.status} ${row.error ?? "pass"}`)
    if (!ok) bad++
  }
  return bad
}

if (import.meta.main) {
  const r = spawnSync(process.argv[2], process.argv.slice(3), { stdio: "inherit", timeout: 55000 })
  process.exit(r.status ?? 1)
}
