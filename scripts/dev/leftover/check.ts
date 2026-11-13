// Checks the run-time net for command lines (dev only): every row of rows.ts on this OS through
// `strobe bench leftover.bench.ts --check`, each refused row failing RunIncomplete ("still running") and every
// other passing. Run from the repo root:
//   bun scripts/dev/leftover/check.ts            (STROBE_BIN=<strobe> to check another build)
// Exit 0 when every row did what it states. Needs bun on PATH (the rows run it) and cargo for the build row.
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { rows } from "./rows.ts"

const here = dirname(fileURLToPath(import.meta.url))
const strobe = process.env.STROBE_BIN ?? "strobe"
const crate = mkdtempSync(join(tmpdir(), "leftover-crate-"))
mkdirSync(join(crate, "src"))
writeFileSync(join(crate, "Cargo.toml"), '[package]\nname = "leftover"\nversion = "0.1.0"\nedition = "2021"\n')
writeFileSync(join(crate, "src", "main.rs"), "fn main() {}\n")
const cargo = spawnSync("cargo", ["--version"]).status === 0
const want = rows(cargo ? crate : undefined)
const r = spawnSync(strobe, ["bench", join(here, "leftover.bench.ts"), "--head", ".", "--check", "--runs", "3"], {
  env: { ...process.env, LEFTOVER_CRATE: cargo ? crate : "" },
  encoding: "utf8",
  maxBuffer: 64 << 20,
})
rmSync(crate, { recursive: true, force: true })
const out = `${r.stdout ?? ""}\n${r.stderr ?? ""}`
const lines = out.split("\n")
let failed = 0
for (const row of want) {
  const at = lines.findIndex((l) => new RegExp(`^(pass|FAIL)\\s+${row.key} `).test(l))
  const got = at < 0 ? "missing" : lines[at].startsWith("pass") ? "pass" : "FAIL"
  // The failure's text: the lines under it up to the next case (a refusal quotes its line, new lines too).
  const end = at < 0 ? at : lines.findIndex((l, i) => i > at && /^(pass|FAIL)\s/.test(l))
  const why = at < 0 ? "" : lines.slice(at + 1, end < 0 ? undefined : end).join(" ").replace(/\s+/g, " ").trim()
  const ok = row.expect === "pass" ? got === "pass" : got === "FAIL" && why.includes("RunIncomplete") && why.includes("still running")
  if (!ok) failed++
  console.log(`${ok ? "ok  " : "BAD "} ${row.key.padEnd(20)} expect ${row.expect.padEnd(7)} got ${got.padEnd(7)} ${row.proves}`)
  if (got === "FAIL") console.log(`       ${why.slice(0, 220)}`)
}
if (!cargo) console.log("(no cargo on PATH: the build row was not run)")
console.log(failed ? `${failed} of ${want.length} rows did not do what they state` : `all ${want.length} rows did what they state`)
process.exit(failed ? 1 : 0)
