// Checks the run-time net for command lines (dev only): every row of rows.ts on this OS through
// `strobe bench leftover.bench.ts --check`, each refused row failing RunIncomplete ("still running") and every
// other passing. Run from the repo root:
//   bun scripts/dev/leftover/check.ts            (STROBE_BIN=<strobe> to check another build)
// Exit 0 when every row did what it states. Needs bun on PATH (the rows run it) and cargo for the build row.
import { spawnSync } from "node:child_process"
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { basename, dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { consoleRows, rows } from "./rows.ts"

const here = dirname(fileURLToPath(import.meta.url))
const bin = Bun.which(process.env.STROBE_BIN ?? "strobe")
if (!bin) throw Error("set STROBE_BIN to the build to check")
const win = process.platform === "win32"
const entries = win ? ["strobe.exe", "strobew.exe"].map(n => join(dirname(bin), n)) : [bin]
const crate = mkdtempSync(join(tmpdir(), "leftover-crate-"))
mkdirSync(join(crate, "src"))
writeFileSync(join(crate, "Cargo.toml"), '[package]\nname = "leftover"\nversion = "0.1.0"\nedition = "2021"\n')
writeFileSync(join(crate, "src", "main.rs"), "fn main() {}\n")
const cargo = spawnSync("cargo", ["--version"]).status === 0
const want = rows(cargo ? crate : undefined)
let failed = 0
try {
for (const strobe of entries) {
console.log(basename(strobe))
const r = spawnSync(strobe, ["bench", join(here, "leftover.bench.ts"), "--head", ".", "--check", "--runs", "3", "--power", "keep", "--reserve-cpus", "off", "--pin", "none"], {
  env: { ...process.env, LEFTOVER_CRATE: cargo ? crate : "" },
  encoding: "utf8",
  maxBuffer: 64 << 20,
  windowsHide: true, timeout: 240000,
})
const out = `${r.stdout ?? ""}\n${r.stderr ?? ""}`
const lines = out.split("\n")
for (const row of want) {
  const at = lines.findIndex((l) => new RegExp(`^(pass|FAIL)\\s+${row.key} `).test(l))
  const got = at < 0 ? "missing" : lines[at].startsWith("pass") ? "pass" : "FAIL"
  // The failure's text: the lines under it up to the next case (a refusal quotes its line, new lines too).
  const end = at < 0 ? at : lines.findIndex((l, i) => i > at && /^(pass|FAIL)\s/.test(l))
  const why = at < 0 ? "" : lines.slice(at + 1, end < 0 ? undefined : end).join(" ").replace(/\s+/g, " ").trim()
  const list = why.match(/still running: (.*?); strobe ended/)?.[1] ?? ""
  const ok = row.expect === "pass" ? got === "pass" : got === "FAIL" && why.includes("RunIncomplete") && list && !/\bpid \d+|conhost\.exe|OpenConsole\.exe/i.test(list)
  if (!ok) failed++
  console.log(`${ok ? "ok  " : "BAD "} ${row.key.padEnd(20)} expect ${row.expect.padEnd(7)} got ${got.padEnd(7)} ${row.proves}`)
  if (got === "FAIL") console.log(`       ${why.slice(0, 220)}`)
}
}
if (win) {
  const worker = join(crate, "worker.exe"), namesake = join(crate, "conhost.exe")
  const build = spawnSync("rustc", [join(here, "worker.rs"), "-o", worker], { windowsHide: true, timeout: 60000 })
  if (build.status !== 0) throw Error(String(build.stderr || build.error))
  copyFileSync(worker, namesake)
  for (const exe of entries) for (const row of consoleRows(entries[1], worker, namesake)) {
    const receipt = join(crate, `${basename(exe)}-${row.key}`)
    const r = spawnSync(exe, ["run", "--check", "--head", row.line, "--reserve-cpus", "off", "--power", "keep", "--json"], {
      windowsHide: true, encoding: "utf8", timeout: 60000, env: { ...process.env, LEFTOVER_RECEIPT: receipt },
    })
    const j = JSON.parse(r.stdout), stored = JSON.parse(readFileSync(j.result, "utf8")), why = stored.series[0].error
    let ok = r.status === 0 && row.expect === "pass" && !why && stored.series[0].passed
    if (row.expect === "refused") {
      const child = row.key === "namesake" ? basename(namesake) : basename(worker)
      const named = `still running: ${child}; strobe ended it,`
      const pid = Number(readFileSync(receipt, "utf8").trim())
      let gone = false
      try { process.kill(pid, 0) } catch (e: any) { gone = e.code === "ESRCH" }
      ok = r.status === 1 && gone && why?.includes(named) && !why.includes(`pid ${pid}`)
      const report = spawnSync(exe, ["report", j.result, "--format", "summary"], { windowsHide: true, encoding: "utf8", timeout: 30000 })
      ok &&= report.status === 0 && report.stdout.includes(child) && !report.stdout.includes(`pid ${pid}`)
    }
    console.log(`${ok ? "ok" : "BAD"} ${basename(exe)} ${row.key}: ${why ?? "pass"}`)
    if (!ok) failed++
  }
}
} finally { rmSync(crate, { recursive: true, force: true }) }
if (!cargo) console.log("(no cargo on PATH: the build row was not run)")
console.log(failed ? `${failed} rows did not do what they state` : "all rows did what they state")
process.exit(failed ? 1 : 0)
