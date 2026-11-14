// Try the alternatives on the actual headless target, without changing the machine's display state.
// Run once with a lit display and once with it off; name a different supported engine as the second argument.
import assert from "node:assert/strict"
import { fileURLToPath } from "node:url"
const [exe="strobe", engine="chromium"] = process.argv.slice(2)
const file = fileURLToPath(new URL("./headless.bench.ts",import.meta.url))
function check(label: string, primary: string) {
  const p=Bun.spawnSync([exe,"bench",file,"--head",".","--check","--headless","--filter",label,"--primary",primary,"--engine",engine,"--runs","2","--why","never","--json"],{timeout:120000,stdout:"pipe",stderr:"pipe"})
  assert.ok(p.exitCode===0 || p.exitCode===4, `${primary}: ${p.exitCode} ${p.stderr.toString().slice(-500)}`)
  const r=JSON.parse(p.stdout.toString()); assert.equal(r.series.length,2)
  return r.series
}
for(const label of ["draw","ready"]) {
  const refusal=check(label,"frames.interval.p50")
  for(const s of refusal) assert.equal(s.primary.refusal.validator,"Headless",s.label)
  const fixes = refusal.map((s:any)=>s.primary.refusal.fix)
  const options: string[] = label==="ready" ? ["launch.ready"] : engine==="chromium" ? ["main.work","cpu.work","count.items","run.duration","cpu.app.total"] : ["cpu.work","run.duration"]
  for(const m of options) {
    assert.ok(fixes.every((f:string)=>f.includes(m==="count.items"?"counter":m)),`${m}: ${fixes}`)
    for(const s of check(label,m)) {
      const dark = s.primary.refusal?.validator === "Throttled" && s.primary.refusal.summary.includes("console display was off")
      if(dark) {
        assert.ok(["run.duration","cpu.app.total"].includes(m) && fixes.every((f:string)=>f.includes("with the display on")))
        assert.ok(["main.work","cpu.work","counter"].every(k=>s.primary.refusal.fix.includes(k)))
      } else assert.ok(s.passed && !s.refused?.[m] && !s.primary.refusal,`${s.label}: ${m}: ${JSON.stringify(s.primary)}`)
      console.log(`${engine} ${s.target} ${label} ${m}: ${dark?"qualified alternative (display off)":"pass"}`)
    }
  }
}
