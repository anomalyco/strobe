// Replay unfinished blocks, not verdicts. Only the per-block readings are planted;
// the CLI computes the paired interval, measurement warnings, outcome and exit.
import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { tmpdir } from "node:os"
import { rendered } from "./rendered"
import { verdicts } from "../../ci-action"

const exe = process.env.STROBE_BIN ?? "strobe"
const scratch = mkdtempSync(join(tmpdir(), "compare-checkpoint-"))
const env = { ...process.env, LOCALAPPDATA: scratch, XDG_DATA_HOME: scratch, ...(process.platform === "darwin" ? {HOME:scratch} : {}) }
const run = (args: string[]) => Bun.spawnSync([exe, ...args], { timeout: 90000, env })
const seedRun = run(["bench", "scripts/dev/endings/rows.bench.ts", "--base", "git:HEAD^", "--head", ".", "--filter", "slow", "--primary","run.duration", "--pairs", "2", "--runs", "1", "--why", "never", "--json"])
assert.equal(seedRun.exitCode,0,seedRun.stdout.toString())
const seed = JSON.parse(seedRun.stdout.toString())
const home = dirname(dirname(seed.result)), label = seed.series[0].label
const state = JSON.parse(readFileSync(join(dirname(seed.result), "state.json"), "utf8"))
for (const [kind,load,strict,margin,code] of [
  ["same",false,false,false,0], ["same",true,false,false,0], ["same",true,true,false,5],
  ["no_worse",true,true,true,5], ["better",true,true,false,0], ["worse",true,true,true,0],
  ["unclear",false,false,false,0], ["unclear",false,true,false,5], ["refused",true,false,false,4],
] as const) {
  const id=`exp_end_${crypto.randomUUID()}`, dir=join(home,id)
  mkdirSync(dir)
  const next=structuredClone(state), flags=next.args.flags
  flags.id=id; flags.pairs="12"
  if(strict) flags["require-verdict"]="true"
  if(margin) flags.noworse="2%"
  const pairs=Array.from({length:12},(_,i)=>[2*i,2*i+1])
  const blocks=pairs.flatMap((_,i)=>["base","head"].map(side=>{
    const b=structuredClone(seed.series[0].blocks.find((b:any)=>b.variant===side))
    b.recording=null; b.pair=i
    b.values["run.duration"]=side==="base" ? 100 : kind==="better" ? 99 : kind==="worse" ? 130 : kind==="unclear" ? (i%2?110:90) : 100
    b.values["cpu.foreign.max"]=load && (kind!=="refused" || side==="head") ? 200 : 0
    b.values["cpu.app.total"]=10; b.values["background.own.max"]=10
    delete b.values["background.unread"]; delete b.values["background.subtick"]
    return b
  }))
  next.series[label]={blocks,pairs,decided_n:12}
  writeFileSync(join(dir,"state.json"), JSON.stringify(next))
  const p=run(["bench","--resume",id,"--json"]), r=JSON.parse(p.stdout.toString()), c=r.series?.[0]?.primary
  assert.equal(p.exitCode,code,`${kind}: ${r.error||c?.verdict}`)
  if(kind==="unclear") assert.ok(["unclear","underpowered"].includes(c.verdict_kind)); else assert.equal(c.verdict_kind,kind)
  assert.equal(c.warned,load && kind!=="refused" || undefined)
  if(c.warned){
    assert.deepEqual(verdicts(r),[kind==="same"||kind==="no_worse"?"unclear":kind])
    for(const format of ["pr","summary"]){
      const md=run(["report","experiment",id,"--format",format]).stdout.toString()
      const expected=kind==="better" ? /\*\*faster\*\* <sub>warned<\/sub>/ : kind==="worse" ? /\*\*worse\*\* <sub>(?:>|&gt;)2%<\/sub> <sub>warned<\/sub>/ : /needs confirmation <sub>warned<\/sub>/
      assert.match(md,expected)
    }
    if(kind==="same" || kind==="no_worse"){
      const html=rendered(exe,id,env)
      assert.match(html,/Needs confirmation/); assert.doesNotMatch(html,/Confirmed · Equivalent|A\/A Control · Passed|Confirmed · No worse/)
      for(const format of ["card","badge"]){
        const out=join(dir,`${format}.svg`), img=run(["report","experiment",id,"--format",format,"--json","--out",out])
        assert.equal(img.exitCode,0); assert.match(img.stdout.toString(),/needs confirmation/i)
      }
    }
  }
  console.log(`${kind} from ${pairs.length} pairs, warned=${!!c.warned}, strict=${strict}: ${code}`)
}
