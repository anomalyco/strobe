// Runs shell.bench.ts and checks each row of the command-line meaning's table (rows.ts), dev only: the case passed
// (its programs printed what the row says, or its line was refused with the row's text), strobe noted `ShellLine` on
// it exactly when the row's outcome is `shell`, and `DirectLine` exactly when the row says `note`. STROBE_BIN picks
// the strobe to run (default: strobe on PATH). From the repo root: bun scripts/dev/shell/check.ts
import { rows } from "./rows.ts"

const strobe = process.env.STROBE_BIN ?? "strobe"
const p = Bun.spawnSync([strobe, "bench", "scripts/dev/shell/shell.bench.ts", "--head", ".", "--check", "--progress", "stdout"], { stdout: "pipe", stderr: "pipe" })
const text = p.stdout.toString() + p.stderr.toString()
const lines = text.split(/\r?\n/)
const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
let bad = 0
let skipped = 0
for (const [name, row] of Object.entries(rows)) {
  if (row.needs && !Bun.which(row.needs)) {
    skipped++
    console.log(`skip ${name.padEnd(18)} ${row.outcome.padEnd(8)} (no ${row.needs} on PATH)`)
    continue
  }
  const label = `${name} · sh`
  const passed = lines.some((l) => l.startsWith(`pass  ${label}  `))
  const noted = (id: string) => lines.some((l) => new RegExp(`^\\s*[i!] ${id}\\s+${esc(label)}: `).test(l))
  const shell = noted("ShellLine")
  const direct = noted("DirectLine")
  const ok = passed && shell === (row.outcome === "shell") && direct === !!row.note
  if (!ok) bad++
  const flags = [shell !== (row.outcome === "shell") ? (shell ? "noted ShellLine" : "no ShellLine") : "", direct !== !!row.note ? (direct ? "noted DirectLine" : "no DirectLine") : ""].filter(Boolean)
  console.log(`${ok ? "ok  " : "BAD "} ${name.padEnd(18)} ${row.outcome.padEnd(8)}${row.note ? " +note" : "      "}  ${passed ? "passed" : "FAILED"}${flags.length ? ", " + flags.join(", ") : ""}  ${JSON.stringify(row.line)}`)
  if (!passed) for (const l of lines.filter((l) => l.includes(`${name}:`) || l.startsWith(`FAIL  ${label}`))) console.log(`       ${l.trim()}`)
}
const total = Object.keys(rows).length - skipped
console.log(`${total - bad} of ${total} rows hold, ${skipped} left out (strobe exited ${p.exitCode})`)
process.exit(bad ? 1 : 0)
