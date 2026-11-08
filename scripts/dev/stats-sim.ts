// The simulations SPEC §9 quotes, through a bench's own decision code (dev, not shipped): each row
// asks the hidden `strobe stats` `simulate` op for synthetic paired series (normal, t3 or spiky noise
// on a log scale, σ per block unless a row says per pair; an effect on head, penalties on a first
// block or a pair's second block) that run
// through `bench::sequential` (looks, Pocock level, futility, run-on for Holm and the family, closing
// verdict, gate and Holm) and, for families, `family::Board`. Prints each row beside what SPEC says.
//
//   bun scripts/dev/stats-sim.ts <strobe> [--only aa,noworse,family,first,secondaries] [--trials N]
//
// Counts carry a Wilson 95% interval. A row's numbers depend only on its seed, not on the threads.
import { spawnSync } from "node:child_process"

const argv = process.argv.slice(2)
const flag = (name: string) => {
  const i = argv.indexOf(`--${name}`)
  return i >= 0 ? argv[i + 1] : undefined
}
const bin = argv[0]
if (!bin || bin.startsWith("--")) throw new Error("usage: bun scripts/dev/stats-sim.ts <strobe> [--only groups] [--trials N]")
const only = flag("only") ? new Set(flag("only")!.split(",")) : null
const scale = flag("trials") ? Number(flag("trials")) / 4000 : 1
const group = (g: string) => !only || only.has(g)

type Q = Record<string, unknown>
const sim = (qs: Q[]): any[] => {
  const r = spawnSync(bin, ["stats"], { input: qs.map((q) => JSON.stringify({ op: "simulate", ...q })).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 26 })
  if (r.status !== 0) throw new Error(`stats: exit ${r.status}: ${r.stderr}`)
  return r.stdout.trim().split("\n").map((l) => JSON.parse(l))
}
const wilson = (p: number, n: number) => {
  const z = 1.96, den = 1 + (z * z) / n, c = (p + (z * z) / (2 * n)) / den
  const h = (z * Math.sqrt((p * (1 - p)) / n + (z * z) / (4 * n * n))) / den
  return `${(100 * p).toFixed(2)}% [${(100 * Math.max(0, c - h)).toFixed(2)}–${(100 * (c + h)).toFixed(2)}]`
}
const share = (o: any, k: string) => (o.verdicts[k] ?? 0) / o.trials
const n = (t: number) => Math.round(t * scale)
const normal = (sigma: number) => ({ kind: "normal", sigma })
const t3 = (sigma: number) => ({ kind: "t3", sigma })
const spike = (sigma: number, p: number) => ({ kind: "spike", sigma, p })
const row = (label: string, o: any, extra = "") => {
  const v = (k: string) => wilson(share(o, k), o.trials)
  console.log(`${label.padEnd(46)} n ${o.trials}  faster ${v("better")}  slower ${v("worse")}  same ${(100 * share(o, "same")).toFixed(1)}%  no worse ${(100 * share(o, "no_worse")).toFixed(1)}%  underp ${(100 * share(o, "underpowered")).toFixed(1)}%  pairs ${o.pairs_mean.toFixed(1)}${extra}`)
}

if (group("aa")) {
  console.log("\n## §9.1 stop rule: A/A false positives (SPEC: looking at 95% after every pair 10.4% near the bound; five Pocock looks 1.5–3.7%; each side ≤ 2.5%)")
  const noises = [normal(0.002), normal(0.01), normal(0.03), normal(0.08), t3(0.01), t3(0.03), spike(0.01, 0.1), spike(0.02, 0.2)]
  const res = sim(noises.map((noise, i) => ({ trials: n(12000), seed: 100 + i, noise })))
  for (const [i, o] of res.entries()) row(`aa auto ${JSON.stringify(noises[i])}`, o, `  coverage ${(100 * o.coverage).toFixed(2)}%`)
  // A 95% look after every pair (not strobe's design), noise near the bound: a pair's difference
  // σ 1% and 2% (a block's σ/√2).
  const every = Array.from({ length: 39 }, (_, i) => i + 2)
  const e = sim([0.01, 0.02].map((s, i) => ({ trials: n(4000), seed: 7 + i, noise: normal(s / Math.SQRT2), looks: every, confidence: 0.95 })))
  for (const [i, o] of e.entries()) row(`aa, a 95% look after every pair, pair σ ${[1, 2][i]}%`, o)
  const fixed = sim([6, 8, 10, 20].map((p, i) => ({ trials: n(12000), seed: 200 + i, noise: normal(0.03), pairs: p })))
  for (const [i, o] of fixed.entries()) row(`aa --pairs ${[6, 8, 10, 20][i]} normal 3%`, o)
}

if (group("noworse")) {
  console.log("\n## §9.3 non-inferiority Checked, σ per pair difference (SPEC: at θ = δ each claim 2.2–2.8% (nominal 2.5%); θ = 0, σ 3%, margin 2%: no worse 94% after 22 pairs vs same 87% after 29; t3 77% vs 53%; +5% at margin 2%: worse 97–100%)")
  // σ here is the per-pair difference's (SPEC's): a block's is σ/√2.
  const d = (s: number) => s / Math.SQRT2
  const rows: [string, Q][] = []
  for (const [nn, noise] of [["normal 1%", normal(d(0.01))], ["normal 3%", normal(d(0.03))], ["t3 1%", t3(d(0.01))], ["t3 3%", t3(d(0.03))]] as const) {
    rows.push([`θ = δ = +2%, ${nn}`, { trials: n(4000), seed: rows.length + 300, noise, effect: 0.02, noworse: 0.02 }])
    rows.push([`θ = δ = +2%, ${nn}, higher is better`, { trials: n(4000), seed: rows.length + 300, noise, effect: -0.02, noworse: 0.02, metric: "metric.score", higher: true }])
  }
  rows.push(["A/A, margin 0%, normal 3%", { trials: n(4000), seed: 330, noise: normal(d(0.03)), noworse: 0.0 }])
  rows.push(["θ = 0, normal 3%, margin 2%", { trials: n(4000), seed: 331, noise: normal(d(0.03)), noworse: 0.02 }])
  rows.push(["θ = 0, normal 3%, two-sided ±2%", { trials: n(4000), seed: 332, noise: normal(d(0.03)) }])
  rows.push(["θ = 0, t3 3%, margin 2%", { trials: n(4000), seed: 333, noise: t3(d(0.03)), noworse: 0.02 }])
  rows.push(["θ = 0, t3 3%, two-sided ±2%", { trials: n(4000), seed: 334, noise: t3(d(0.03)) }])
  rows.push(["θ = +5%, normal 3%, margin 2%", { trials: n(4000), seed: 335, noise: normal(d(0.03)), effect: 0.05, noworse: 0.02 }])
  rows.push(["θ = +5%, normal 1%, margin 2%", { trials: n(4000), seed: 336, noise: normal(d(0.01)), effect: 0.05, noworse: 0.02 }])
  const res = sim(rows.map((r) => r[1]))
  for (const [i, o] of res.entries()) row(rows[i][0], o)
}

if (group("family")) {
  console.log("\n## §9.3 families Checked (SPEC: A/A M = 3, 5, 8, 10: any null better 0.25–1.4%, worse 0.25–2.0%; one at a 5% margin: no worse in all 1.9–2.7%; all −30%: every one counted; 3 × −20% of 8: 3.00 of 3, a null 2.4%)")
  // σ per pair difference, as SPEC states it (a block's σ/√2).
  const d = (s: number) => s / Math.SQRT2
  const fams: [string, Q][] = []
  for (const m of [3, 5, 8, 10]) for (const [nn, noise] of [["normal 3%", normal(d(0.03))], ["t3 1%", t3(d(0.01))]] as const) fams.push([`A/A M = ${m}, pair ${nn}`, { trials: n(2000), seed: 400 + fams.length, noise, journeys: m, effects: [0] }])
  fams.push(["M = 5, one at a 5% margin, rest 0, pair 1%", { trials: n(2000), seed: 420, noise: normal(d(0.01)), journeys: 5, effects: [0.05, 0, 0, 0, 0], noworse: 0.05 }])
  fams.push(["M = 5, all at a 5% margin, pair 1%", { trials: n(2000), seed: 421, noise: normal(d(0.01)), journeys: 5, effects: [0.05], noworse: 0.05 }])
  for (const m of [3, 5, 7, 10]) fams.push([`M = ${m}, all −30%, pair normal 3%`, { trials: n(2000), seed: 430 + m, noise: normal(d(0.03)), journeys: m, effects: [-0.3] }])
  fams.push(["M = 8, three −20%, five null, pair normal 3%", { trials: n(2000), seed: 440, noise: normal(d(0.03)), journeys: 8, effects: [-0.2, 0, 0, -0.2, 0, 0, -0.2, 0] }])
  const res = sim(fams.map((f) => f[1]))
  for (const [i, o] of res.entries()) {
    const f = o.family
    console.log(`${fams[i][0].padEnd(46)} families ${o.trials}  any null better ${wilson(f.any_null_better, o.trials)}  worse ${wilson(f.any_null_worse, o.trials)}  true counted ${(100 * f.true_counted).toFixed(1)}%  no worse in all ${(100 * f.all_no_worse).toFixed(1)}%  pairs ${o.pairs_mean.toFixed(1)}`)
  }
}

if (group("first")) {
  console.log("\n## stats-4: a slow first block (pair 0's base), without and with the discarded warm-up blocks")
  const rows: [string, Q][] = []
  for (const [pen, noise, nn] of [[0.034, normal(0.06), "normal 6%"], [0.3, normal(0.01), "normal 1%"], [0.1, normal(0.03), "normal 3%"]] as const) {
    rows.push([`first +${(pen * 100).toFixed(1)}%, ${nn}`, { trials: n(12000), seed: 500 + rows.length, noise, first: pen }])
    rows.push([`first +${(pen * 100).toFixed(1)}%, ${nn}, discarded`, { trials: n(12000), seed: 500 + rows.length, noise, first: pen, discard: true }])
    rows.push([`first +${(pen * 100).toFixed(1)}%, ${nn}, no worse 0% (each claim 2.5%)`, { trials: n(12000), seed: 500 + rows.length, noise, first: pen, noworse: 0.0 }])
    rows.push([`first +${(pen * 100).toFixed(1)}%, ${nn}, no worse 0%, discarded`, { trials: n(12000), seed: 500 + rows.length, noise, first: pen, noworse: 0.0, discard: true }])
  }
  const res = sim(rows.map((r) => r[1]))
  for (const [i, o] of res.entries()) row(rows[i][0], o, `  pair 0 ln(head/base) ${(100 * o.first_pair_mean).toFixed(2)}%`)
}

if (group("secondaries")) {
  console.log("\n## §9.3 gate and Holm: secondaries (5 null at σ 3%; FWER must stay ≤ 5%)")
  const null5 = Array.from({ length: 5 }, () => ({ effect: 0, sigma: 0.03, rho: 0 }))
  const corr5 = Array.from({ length: 5 }, () => ({ effect: 0, sigma: 0.03, rho: 0.8 }))
  const rows: [string, Q][] = [
    ["primary A/A, 5 null", { trials: n(12000), seed: 600, noise: normal(0.01), secondaries: null5 }],
    ["primary +10% (σ 1%), 5 null", { trials: n(12000), seed: 601, noise: normal(0.01), effect: 0.1, secondaries: null5 }],
    ["primary +3% (σ 3%), 5 null, ρ 0.8", { trials: n(12000), seed: 602, noise: normal(0.03), effect: 0.03, secondaries: corr5 }],
  ]
  const res = sim(rows.map((r) => r[1]))
  for (const [i, o] of res.entries()) row(rows[i][0], o, `  secondaries: any wrong ${wilson(o.secondaries.any_wrong, o.trials)}, per null test ${(100 * o.secondaries.null_called).toFixed(2)}%`)
}
