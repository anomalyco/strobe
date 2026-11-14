// Run manually from the repo root with the binary to check. --baseline prints the old endings.
import assert from "node:assert/strict"
import { existsSync, readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { rendered } from "./rendered"

const exe = process.env.STROBE_BIN ?? "strobe"
const file = "scripts/dev/endings/rows.bench.ts"
const baseline = process.argv.includes("--baseline")
const base = ["--base", "git:HEAD^"]
const counter = [...base, "--noworse", "count.items"]
const rows: [string, string[], number, string[], string][] = [
  ["pass", [], 0, ["pass"], "1 of 1 cases passed"],
  ["base", base, 0, ["pass"], "base and head each checked"],
  ["counter", counter, 0, ["pass"], "only the --noworse counters compared"],
  ["secondary", [], 0, ["pass"], "1 of 1 cases passed"],
  ["worse", counter, 1, ["fail"], "0 of 1 cases passed"],
  ["basefail", base, 1, ["fail"], "1 failed on base"],
  ["somegap", [], 0, ["pass", "gap"], "1 parity gap"],
  ["bare-gap", [...base,"--noworse","0%"], 0, ["pass","gap"], "1 parity gap"],
  ["bad-gap", [...base,"--noworse","main.work=2%"], 2, [], ""],
  ["garbage-gap", [...base,"--noworse","banana"], 2, [], ""],
  ["nofixture", [], 4, ["refused"], "nothing checked"],
  ["failure-wins", [...base,"--noworse","count.absent","--require-parity"], 1, ["fail","gap"], "1 parity gap"],
  ["empty", [...base, "--noworse", "count.absent"], 4, ["pass"], "nothing compared"],
  ["defect", [], 1, ["fail"], "0 of 1 cases passed"],
  ["phase", [], 1, ["fail"], "0 of 1 cases passed"],
  ["frames", [], 4, ["refused"], "nothing checked"],
  ["mixed", [], 0, ["pass", "refused"], "1 refused"],
  ["strict", ["--require-verdict"], 4, ["pass", "refused"], "1 refused"],
  ["gap", [], 0, ["gap"], "nothing checked"],
  ["gaps-counter", counter, 4, ["gap"], "nothing compared"],
  ["gaps-parity", ["--require-parity"], 4, ["gap"], "nothing checked"],
  ["flags", ["--noworse", "main.work"], 2, [], ""],
  ["nomatch", [], 2, [], ""],
]
let experiments = ""
for (const [name, flags, code, states, summary] of rows) {
  const filter = ({ mixed: "ok,frames", strict: "ok,frames", somegap:"ok,gap", "failure-wins":"defect,gap", "gaps-parity": "gap", "gaps-counter": "gap" } as Record<string,string>)[name] ?? (["nomatch","secondary","worse","basefail","defect","phase","frames","gap"].includes(name) ? name : "ok")
  const run = (args: string[]) => Bun.spawnSync([exe, ...args], { timeout: 100000, stdout: "pipe", stderr: "pipe" })
  const id = `exp_end_${crypto.randomUUID()}`
  const selected = ({"bare-gap":"ok,gap","bad-gap":"gap","garbage-gap":"gap",nofixture:"nofixture"} as Record<string,string>)[name] ?? filter
  const p = run(["bench", file, "--id", id, "--head", ".", "--filter", selected, "--check", ...flags, "--json"])
  const r = JSON.parse(p.stdout.toString().trim() || "{}")
  console.log(name, p.exitCode, r.passed, r.series?.map((s: any) => [s.state, s.passed, s.decided]), r.text?.split("\n").find((l: string) => /cases passed|nothing checked|nothing compared/.test(l)) ?? r.error ?? p.stderr.toString().slice(-300))
  if (baseline) continue
  assert.equal(p.exitCode, code, name)
  if (name === "garbage-gap") assert.match(r.error, /--noworse:.*banana/)
  if (!states.length) { assert.doesNotMatch(p.stderr.toString(), / · exp_/); assert.ok(experiments && !existsSync(join(experiments,id))); continue }
  experiments = dirname(dirname(r.result))
  const saved = JSON.parse(readFileSync(r.result, "utf8"))
  assert.deepEqual(saved.series, r.series)
  assert.equal(saved.passed, code === 0 && states.includes("pass"))
  assert.equal(r.passed, saved.passed)
  assert.deepEqual(r.series.map((s: any) => s.state), states)
  assert.ok(r.text.includes(summary), name)
  const pr = run(["report", "experiment", r.id, "--format", "summary"])
  assert.equal(pr.exitCode, 0)
  for (const s of r.series) {
    assert.equal(s.passed, s.state === "pass")
    assert.equal(s.decided, s.passed)
    const word = s.state === "fail" ? "FAIL" : s.state
    assert.match(r.text, new RegExp(`^${word}\\s+${s.case}\\b`, "m"))
    assert.ok(pr.stdout.toString().includes(`| ${s.case} |`))
  }
  if (code === 4) assert.doesNotMatch(pr.stdout.toString(), /\*\*\d+ of \d+ cases? passed/)
  if (name === "gap" || name === "empty") {
    const html = rendered(exe,id)
    assert.match(html,name === "gap" ? /not compared/i : /nothing compared/i)
    assert.doesNotMatch(html,/Confirmed · Equivalent|A\/A Control · Passed/)
  }
}
if (!baseline) {
  const id=`exp_end_${crypto.randomUUID()}`
  const p=Bun.spawnSync([exe,"bench","scripts/dev/endings/line.bench.ts","--check","--head",".","--id",id,"--json"],{timeout:30000})
  assert.equal(p.exitCode,2,p.stdout.toString())
  assert.match(JSON.parse(p.stdout.toString()).error,/&/)
  assert.doesNotMatch(p.stderr.toString(), / · exp_/)
  assert.ok(!existsSync(join(experiments,id)))
  const parity=Bun.spawnSync([exe,"bench","scripts/dev/endings/parity.bench.ts","--base","left","--head","right","--headless","--check","--json"],{timeout:90000})
  const r=JSON.parse(parity.stdout.toString()); assert.equal(parity.exitCode,1,r.error||r.text)
  assert.equal(r.series.length,2); assert.ok(r.series.every((s:any)=>s.state==="fail" && s.journey_refusal?.validator==="NotEquivalent"))
  assert.doesNotMatch(r.text,/refused  this journey:/)
}
