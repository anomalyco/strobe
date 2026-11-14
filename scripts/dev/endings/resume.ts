// Drive the finalizer and result readers with completed series at the existing
// checkpoint boundary. No foreign result is opened or changed. The seed is a real
// two-pair run of a windowless script; the replay is not a measurement.
import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { tmpdir } from "node:os"

const exe = process.env.STROBE_BIN ?? "strobe"
const scratch = mkdtempSync(join(tmpdir(), "compare-checkpoint-"))
const env = { ...process.env, LOCALAPPDATA: scratch, XDG_DATA_HOME: scratch, ...(process.platform === "darwin" ? {HOME:scratch} : {}) }
const run = (args: string[]) => Bun.spawnSync([exe, ...args], { timeout: 90000, env })
const p = run(["bench", "scripts/dev/endings/rows.bench.ts", "--aa", "--head", ".", "--filter", "ok", "--pairs", "2", "--runs", "1", "--why", "never", "--json"])
assert.equal(p.exitCode, 0, p.stdout.toString().slice(0, 250))
const seed = JSON.parse(p.stdout.toString())
const home = dirname(dirname(seed.result))
const state = JSON.parse(readFileSync(join(dirname(seed.result), "state.json"), "utf8"))
const label = seed.series[0].label
for (const [kind, warned, strict, code] of [
  ["same",false,false,0], ["same",true,false,0], ["same",true,true,5],
  ["no_worse",true,true,5], ["better",true,true,0], ["worse",true,true,0],
  ["unclear",false,false,0], ["unclear",false,true,5], ["refused",false,false,4],
] as const) {
  const id = `exp_end_${crypto.randomUUID()}`
  const dir = join(home, id)
  mkdirSync(dir)
  const next = structuredClone(state)
  next.args.flags.id = id
  if (strict) next.args.flags["require-verdict"] = "true"
  const s = next.series[label].done
  const c = s.primary
  s.decided = c.decided = !["unclear","refused"].includes(kind)
  c.verdict_kind = kind
  c.verdict = ({ same:"same (±2%)", no_worse:"no worse (≤2%)", better:"faster", worse:"slower", unclear:"unclear", refused:"refused (NoPhase)" })[kind]
  if (warned) { c.warned=true; c.verdict+=" (warned)"; s.warnings=[{id:"Background",level:"warn",text:"contention can mask a difference"}] }
  if (kind === "refused") { c.refusal={validator:"NoPhase",summary:"fixture lacks its phase"}; for (const k of ["base","head","lo","hi","estimate","p","confidence"]) c[k]=null }
  writeFileSync(join(dir,"state.json"), JSON.stringify(next))
  const replay = run(["bench","--resume",id,"--json"])
  const r = JSON.parse(replay.stdout.toString())
  assert.equal(replay.exitCode, code, `${kind} ${JSON.stringify(r.error)}`)
  assert.equal(r.series[0].primary.verdict_kind, kind)
  assert.equal(r.series[0].primary.warned, warned || undefined)
  if (warned) {
    assert.match(r.text, /warned/)
    for (const format of ["pr","summary"]) {
      const doc = run(["report","experiment",id,"--format",format])
      assert.equal(doc.exitCode,0)
      assert.match(doc.stdout.toString(), /warned|inconclusive|unclear|confirm|Background/i)
    }
  }
  console.log(`checkpoint ${kind}, warned=${warned}, strict=${strict}: exit ${code}`)
}
