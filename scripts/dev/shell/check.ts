// Runs shell.bench.ts and checks each row of its table (dev only): the case passed (its programs printed what the row
// says) and strobe noted `ShellLine` on it exactly when the row says the shell runs the line. STROBE_BIN picks the
// strobe to run (default: strobe on PATH). From the repo root: bun scripts/dev/shell/check.ts
import { rows } from "./rows.ts"

const strobe = process.env.STROBE_BIN ?? "strobe"
const p = Bun.spawnSync([strobe, "bench", "scripts/dev/shell/shell.bench.ts", "--head", ".", "--check", "--progress", "stdout"], { stdout: "pipe", stderr: "pipe" })
const text = p.stdout.toString() + p.stderr.toString()
const lines = text.split(/\r?\n/)
let bad = 0
for (const [name, row] of Object.entries(rows)) {
  const label = `${name} · sh`
  const passed = lines.some((l) => l.startsWith(`pass  ${label}  `))
  const noted = lines.some((l) => new RegExp(`^\\s*[i!] ShellLine\\s+${label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}: `).test(l))
  const ok = passed && noted === row.shell
  if (!ok) bad++
  console.log(`${ok ? "ok  " : "BAD "} ${name.padEnd(14)} ${row.shell ? "shell " : "direct"}  ${passed ? "passed" : "FAILED"}${noted === row.shell ? "" : noted ? ", noted ShellLine" : ", no ShellLine"}  ${row.line}`)
  if (!passed) for (const l of lines.filter((l) => l.includes(`${name}:`) || l.startsWith(`FAIL  ${label}`))) console.log(`       ${l.trim()}`)
}
console.log(`${Object.keys(rows).length - bad} of ${Object.keys(rows).length} rows hold (strobe exited ${p.exitCode})`)
process.exit(bad ? 1 : 0)
