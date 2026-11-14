// Real comparisons through `stats`, with known block values and the states a series can assign later.
import assert from "node:assert/strict"
import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

export function shiftRows(strobe: string, dir: string) {
  const cases = [
    ["run.duration", 0.08, 0.02], ["cpu.work", 4.2, -0.7], ["interaction.p95", 738, 150],
    ["frames.interval.p95", 18_300, 1200], ["memory.peak", 80, -1.5], ["io.read", 2 ** 30, 2 ** 20],
    ["main.work", 4, 0], ["phase.load", 5, 0], ["launch.ready", 10, 0], ["run.duration", 15, 0],
    ["run.duration", 150, 150], ["count.items", 12, 2], ["cpu.app.total", 80, 3], ["run.duration", 200, 20],
    ["run.duration", 1, 1],
  ] as const
  const inputs = cases.map(([metric, base, step], i) => ({ op: "compare", metric, ...(i === 7 ? {noworse: 0.02} : {}),
    pairs: Array.from({length: 8}, (_, k) => i === 14 ? (k < 4 ? [1, 3] : [120, 100]) : [base, base + step + (i === 8 || i === 9 ? (k % 2 ? 1 : -1) * base / 3 : step * (k - 3) / 300)]) }))
  const repeat = (n: number, a: number, b: number) => Array.from({length:n}, () => [a,b])
  const edges: [string, number[][]][] = [
    ["run.duration", [...repeat(7,1,3), ...repeat(3,500,400)]],
    ["run.duration", [...repeat(4,100,100.2), ...repeat(4,110,109.79)]],
    ["run.duration", [...repeat(7,100,100.5), [100,250]]],
    ["cpu.work", [...repeat(4,0.3,0.4), ...repeat(4,0.4,0.3-Number.EPSILON/4)]],
    ["cpu.work", [...repeat(4,2,2), ...repeat(4,2,3)]],
    ["run.duration", repeat(8,10000,10001)], ["run.duration", repeat(8,1,10001)],
    ["io.read", repeat(8,2**30,2**30+2**16)], ["io.read", repeat(8,512,512+2**23)],
    ["run.duration", [...repeat(4,100,250), ...repeat(4,200,350)]],
  ]
  for (const [metric,pairs] of edges) inputs.push({op:"compare", metric:metric as typeof inputs[number]["metric"], pairs})
  const p = Bun.spawnSync([strobe, "stats"], {stdin: Buffer.from(inputs.map(x => JSON.stringify(x)).join("\n") + "\n"), timeout: 30000})
  assert.equal(p.exitCode, 0, p.stderr.toString())
  const out = p.stdout.toString().trim().split("\n").map(x => JSON.parse(x))
  return inputs.map(({metric,pairs}, i) => {
    const c = out[i]; assert.equal(c.metric, metric)
    if (i === 9) Object.assign(c, {verdict:"underpowered",verdict_kind:"underpowered",decided:false})
    if (i === 10) Object.assign(c, {warned:true, verdict:c.verdict + " (warned)"})
    if (i === 13) { delete c.shift; Object.assign(c, {verdict_kind:"refused",verdict:"refused (Throttled): clock",refusal:{validator:"Throttled",summary:"clock"},decided:false,base:null,head:null,estimate:null,lo:null,hi:null,p:null,confidence:null}) }
    assert.equal(!!c.shift, i < 11 || i >= 16, `row-${i} ${metric}: wrong shift presence`)
    const blocks = pairs.flatMap((p,pair) => p.map((v,k) => ({pair,variant:k?"head":"base",values:{[metric]:v}})))
    const label = `row-${i}`
    const r = {id:`exp_shift${i}`,kind:"experiment",created:"2026-01-01",file:"fixture.bench.ts",plan:{looks:[pairs.length],runs_per_block:1,aa:i===16},variants:[{role:"base",label:"base"},{role:"head",label:"head"}],series:[{kind:"runs",label,case:label,primary:c,target:"app",blocks, pairs:pairs.length,launches:pairs.length*2,primary_pairs:pairs.length,decided:c.decided}]}
    mkdirSync(join(dir,label)); const file=join(dir,label,"result.json"); writeFileSync(file,JSON.stringify(r)); return file
  })
}
