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
    ["run.duration", [...repeat(4,10,10.5), ...repeat(4,1000,960)]],
    ["run.duration", [...repeat(4,1,1.0002), ...repeat(4,100,99.99968)]],
    ["io.read", [[100,100], ...repeat(7,100,101)]],
  ]
  for (const [metric,pairs] of edges) inputs.push({op:"compare", metric:metric as typeof inputs[number]["metric"], pairs})
  const firstGenerated = inputs.length
  let seed = 0x51f701
  const random = () => ((seed = (Math.imul(seed,1664525)+1013904223)>>>0) / 2**32)
  for (let i=0; i<216; i++) {
    const base=2**Math.floor(random()*8), large=base*10**(2+Math.floor(random()*3)), sign=random()<0.5?-1:1
    const small=10**(-5+Math.floor(random()*5))*(0.5+random()), far=small*(1+random()*5)
    const bytes=i%3===0
    const pairs = bytes ? [[large,large], ...repeat(7,large,large+sign*(1+2*Math.floor(random()*5)))]
      : [...repeat(4,base,base+sign*small), ...repeat(4,large,large+(i%3===1?-1:1)*sign*far)]
    inputs.push({op:"compare",metric:bytes?"io.read":"run.duration",pairs})
  }
  const p = Bun.spawnSync([strobe, "stats"], {stdin: Buffer.from(inputs.map(x => JSON.stringify(x)).join("\n") + "\n"), timeout: 30000})
  assert.equal(p.exitCode, 0, p.stderr.toString())
  const out = p.stdout.toString().trim().split("\n").map(x => JSON.parse(x))
  const printed=out.slice(firstGenerated).filter(c=>c.shift).length
  console.log(`generated shifts: ${printed} printed, ${out.length-firstGenerated-printed} omitted (${out.length-firstGenerated} pair sets)`)
  return inputs.map(({metric,pairs}, i) => {
    const c = out[i]; assert.equal(c.metric, metric)
    if (i === 9) Object.assign(c, {verdict:"underpowered",verdict_kind:"underpowered",decided:false})
    if (i === 10) Object.assign(c, {warned:true, verdict:c.verdict + " (warned)"})
    if (i === 13) { delete c.shift; Object.assign(c, {verdict_kind:"refused",verdict:"refused (Throttled): clock",refusal:{validator:"Throttled",summary:"clock"},decided:false,base:null,head:null,estimate:null,lo:null,hi:null,p:null,confidence:null}) }
    if (i < firstGenerated) assert.equal(!!c.shift, i < 11 || (i >= 16 && i !== 25 && i !== 26), `row-${i} ${metric}: wrong shift presence`)
    const blocks = pairs.flatMap((p,pair) => p.map((v,k) => ({pair,variant:k?"head":"base",values:{[metric]:v}})))
    const label = `row-${i}`
    const r = {id:`exp_shift${i}`,kind:"experiment",created:"2026-01-01",file:"fixture.bench.ts",plan:{looks:[pairs.length],runs_per_block:1,aa:i===16},variants:[{role:"base",label:"base"},{role:"head",label:"head"}],series:[{kind:"runs",label,case:label,primary:c,target:"app",blocks, pairs:pairs.length,launches:pairs.length*2,primary_pairs:pairs.length,decided:c.decided}]}
    if (i===24) {
      const m='phase.other'
      Object.assign(r.series[0],{descriptive:[{...c,metric:m,verdict:'·',verdict_kind:'descriptive',decided:false}]})
      for(const b of blocks) b.values[m]=b.values[metric]
    }
    mkdirSync(join(dir,label)); const file=join(dir,label,"result.json"); writeFileSync(file,JSON.stringify(r)); return file
  })
}
