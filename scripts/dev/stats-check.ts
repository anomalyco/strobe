// Known-answer checks of strobe's statistics (SPEC §9.3) through the hidden `strobe stats` command:
// synthetic pairs go in, what a bench would decide comes out, and each answer is printed beside the
// truth computed here independently (brute force, simulation or arithmetic).
//
//   bun scripts/dev/stats-check.ts <strobe> [<other strobe> …] [--only wilcoxon,theil,…]
//
// Give an older build and a newer one to see a fix: every row prints each binary's answer.
import { spawnSync } from "node:child_process"

const argv = process.argv.slice(2)
const onlyAt = argv.indexOf("--only")
const only = onlyAt >= 0 ? new Set(argv[onlyAt + 1].split(",")) : null
const bins = argv.filter((_, i) => onlyAt < 0 || (i !== onlyAt && i !== onlyAt + 1))
if (!bins.length) throw new Error("usage: bun scripts/dev/stats-check.ts <strobe> [<strobe> …] [--only <groups>]")

type Q = Record<string, unknown>
const ask = (bin: string, qs: Q[]): any[] => {
  const r = spawnSync(bin, ["stats"], { input: qs.map((q) => JSON.stringify(q)).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 28 })
  if (r.status !== 0) throw new Error(`${bin} stats: exit ${r.status}: ${r.stderr}`)
  return r.stdout.trim().split("\n").map((l) => JSON.parse(l))
}
const all = (qs: Q[]) => bins.map((b) => ask(b, qs))
const short = (b: string) => b.replace(/^.*[\\/]/, "")
const f = (x: unknown, d = 4) => (typeof x === "number" ? x.toFixed(d) : String(x))
const group = (name: string) => !only || only.has(name)

// Seeded normal draws, so a run is repeatable.
let seed = 12345
const rand = () => ((seed = (seed * 1103515245 + 12345) % 2 ** 31) + 0.5) / 2 ** 31
const normal = () => Math.sqrt(-2 * Math.log(rand())) * Math.cos(2 * Math.PI * rand())

// Exact two-sided signed-rank p with ties: mid-ranks of |d| (zeros dropped), every one of the 2^n
// sign assignments enumerated.
function exactWilcoxon(d: number[]): number {
  const nz = d.filter((x) => x !== 0)
  const n = nz.length
  if (!n) return 1
  const order = nz.map((x, i) => [Math.abs(x), i]).sort((a, b) => a[0] - b[0])
  const rank2 = new Array(n).fill(0) // doubled mid-ranks: integers
  for (let i = 0; i < n; ) {
    let j = i
    while (j + 1 < n && order[j + 1][0] === order[i][0]) j++
    for (let k = i; k <= j; k++) rank2[order[k][1]] = i + j + 2
    i = j + 1
  }
  const t = nz.reduce((s, x, i) => s + (x > 0 ? rank2[i] : 0), 0)
  let le = 0, ge = 0
  for (let mask = 0; mask < 1 << n; mask++) {
    let s = 0
    for (let i = 0; i < n; i++) if (mask & (1 << i)) s += rank2[i]
    if (s <= t) le++
    if (s >= t) ge++
  }
  return Math.min(1, (2 * Math.min(le, ge)) / 2 ** n)
}

if (group("wilcoxon")) {
  console.log("\n## cstat-4 signed-rank p with ties (exact enumeration is the truth)")
  const vs: number[][] = [
    [1, 1, 1, 1, 1, 1, 1, 1, -1, -1, 2],
    [1, 1, 1, 1, 1, 1, 1, -1, 2, 2],
    [2, 2, 2, 1, 1, 1, 1, -1, -1, 3, 3],
    [1, 1, 1, 1, 1, 1, -1, -1, 1, 1, 1, 1],
    [3, 3, 3, -3, 3, 3, 3, 3, -3, 3],
    [1, 2, 3, 4, 5, 6, 7, 8, -9, 10], // no ties: both must agree
    [0, 0, 1, 1, 1, 1, 1, 1, 1, -1, 1, 2, 2],
  ]
  const res = all(vs.map((d) => ({ op: "wilcoxon", d })))
  for (const [i, d] of vs.entries()) console.log(`d=[${d}]  truth ${f(exactWilcoxon(d))}  ${bins.map((b, k) => `${short(b)} ${f(res[k][i].p)}`).join("  ")}`)
  // Large n with ties: the normal approximation with the tie-corrected variance, against a Monte
  // Carlo of the exact permutation distribution (200k sign flips).
  const big = Array.from({ length: 80 }, (_, i) => (i % 7 === 0 ? -1 : i % 3 === 0 ? 2 : 1))
  const mc = (() => {
    const nz = big.filter((x) => x !== 0), n = nz.length
    const order = nz.map((x, i) => [Math.abs(x), i]).sort((a, b) => a[0] - b[0])
    const r2 = new Array(n).fill(0)
    for (let i = 0; i < n; ) { let j = i; while (j + 1 < n && order[j + 1][0] === order[i][0]) j++; for (let k = i; k <= j; k++) r2[order[k][1]] = i + j + 2; i = j + 1 }
    const t = nz.reduce((s, x, i) => s + (x > 0 ? r2[i] : 0), 0), mean = r2.reduce((a, b) => a + b, 0) / 2
    let hit = 0
    const N = 200000
    for (let k = 0; k < N; k++) { let s = 0; for (let i = 0; i < n; i++) if (rand() < 0.5) s += r2[i]; if (Math.abs(s - mean) >= Math.abs(t - mean)) hit++ }
    return hit / N
  })()
  const rb = all([{ op: "wilcoxon", d: big }])
  console.log(`n=80 tied  monte-carlo ${f(mc)}  ${bins.map((b, k) => `${short(b)} ${f(rb[k][0].p)}`).join("  ")}`)
  // Above 50 pairs with every |d| tied: the signed-rank test is the sign test, whose exact p is binomial.
  for (const [n, plus] of [[60, 39], [80, 50], [120, 72]]) {
    const d = Array.from({ length: n }, (_, i) => (i < plus ? 1 : -1))
    let tail = 0, term = 1
    for (let k = 0; k <= n; k++) { if (k > 0) term = (term * (n - k + 1)) / k; if (k >= plus) tail += term }
    const truth = Math.min(1, (2 * tail) / 2 ** n)
    const r = all([{ op: "wilcoxon", d }])
    console.log(`n=${n} all tied, ${plus} up  binomial ${f(truth)}  ${bins.map((b, k) => `${short(b)} ${f(r[k][0].p)}`).join("  ")}`)
  }
  // Small quantized samples whose p lands on the wrong side of 5%: a false rejection.
  const flips: number[][] = []
  for (let k = 0; k < 4000 && flips.length < 6; k++) {
    const n = 8 + Math.floor(rand() * 10)
    const d = Array.from({ length: n }, () => Math.round(normal() * 1.2 + 0.6))
    const truth = exactWilcoxon(d)
    if (truth > 0.05 && truth < 0.08) flips.push(d)
  }
  const fr = all(flips.map((d) => ({ op: "wilcoxon", d })))
  for (const [i, d] of flips.entries()) console.log(`d=[${d}]  truth ${f(exactWilcoxon(d))}  ${bins.map((b, k) => `${short(b)} ${f(fr[k][i].p)}${fr[k][i].p <= 0.05 ? " (rejects at 5%: wrong)" : ""}`).join("  ")}`)
}

if (group("tails")) {
  console.log("\n## cstat-21 normal tails above 50 pairs (truth: the normal approximation evaluated with an exact erfc)")
  // erfc from its continued fraction (Lentz), good to ~1e-15 relative for x > 2.
  const erfcTail = (x: number) => {
    let fr = x, c = x, dd = 0
    for (let i = 1; i < 200; i++) {
      const a = i / 2
      dd = x + a * dd; dd = 1 / dd
      c = x + a / c
      const delta = c * dd; fr *= delta
      if (Math.abs(delta - 1) < 1e-16) break
    }
    return Math.exp(-x * x) / Math.sqrt(Math.PI) / fr
  }
  // Up to 400 pairs the exact p is 2·2⁻ⁿ when every pair went one way.
  const cases = [60, 70, 90].map((n) => Array.from({ length: n }, (_, i) => i + 1))
  const res = all(cases.map((d) => ({ op: "wilcoxon", d })))
  for (const [i, d] of cases.entries()) {
    const n = d.length, mean = (n * (n + 1)) / 4, sd = Math.sqrt((n * (n + 1) * (2 * n + 1)) / 24)
    const z = ((n * (n + 1)) / 2 - mean - 0.5) / sd
    console.log(`n=${n} all one way  exact ${(2 / 2 ** n).toExponential(3)} (normal tail at z ${z.toFixed(2)}: ${erfcTail(z / Math.SQRT2).toExponential(3)})  ${bins.map((b, k) => `${short(b)} ${Number(res[k][i].p).toExponential(3)}`).join("  ")}`)
  }
  // Above 400 pairs (the normal approximation): untied ranks, a strong shift.
  const n = 450
  const d = Array.from({ length: n }, (_, i) => (i % 4 === 0 ? -(i + 1) : i + 1))
  const t = d.reduce((s, x) => s + (x > 0 ? x : 0), 0), mean = (n * (n + 1)) / 4, sd = Math.sqrt((n * (n + 1) * (2 * n + 1)) / 24)
  const z = (Math.abs(t - mean) - 0.5) / sd
  const r = all([{ op: "wilcoxon", d }])
  console.log(`n=${n} three in four up  z ${z.toFixed(2)}  truth ${erfcTail(z / Math.SQRT2).toExponential(3)}  ${bins.map((b, k) => `${short(b)} ${Number(r[k][0].p).toExponential(3)}`).join("  ")}`)
}

// Pairs with a little noise around fixed base and head values.
const noisy = (n: number, base: number, head: number, sd: number) => Array.from({ length: n }, () => [base + normal() * sd, head + normal() * sd])

if (group("bound")) {
  console.log("\n## cstat-5 / cstat-13 equivalence bound in a verdict (truth: arithmetic)")
  const cases: [string, Q][] = [
    ["layout.shift 0 → 0.30 (a score: 2% of 0.30 = 0.006)", { op: "compare", metric: "layout.shift", pairs: Array.from({ length: 12 }, () => [0, 0.3]), bound: 0.02, confidence: 0.95 }],
    ["layout.shift 0 → about 0.1 (shifts that come and go)", { op: "compare", metric: "layout.shift", pairs: Array.from({ length: 12 }, (_, i) => [0, [0, 0.3, 0.1, 0.05][i % 4]]), bound: 0.02, confidence: 0.95 }],
    ["layout.shift 0 → 0 every pair", { op: "compare", metric: "layout.shift", pairs: Array.from({ length: 12 }, () => [0, 0]), bound: 0.02, confidence: 0.95 }],
    ["layout.count 40 → 40 ±0 (count: half a count floor stays)", { op: "compare", metric: "layout.count", pairs: Array.from({ length: 12 }, (_, i) => [40 + (i % 2), 40 + ((i + 1) % 2)]), bound: 0.002, confidence: 0.95 }],
    ["run.duration ±0.3% bound, same", { op: "compare", metric: "run.duration", pairs: noisy(12, 100, 100, 0.01), bound: 0.003, confidence: 0.95 }],
    ["run.duration ±1.5% bound, same", { op: "compare", metric: "run.duration", pairs: noisy(12, 100, 100, 0.05), bound: 0.015, confidence: 0.95 }],
    ["run.duration no worse 1.5%", { op: "compare", metric: "run.duration", pairs: noisy(12, 100, 100, 0.05), noworse: 0.015, confidence: 0.95 }],
  ]
  const res = all(cases.map((c) => c[1]))
  for (const [i, [label]] of cases.entries()) console.log(`${label}\n    ${bins.map((b, k) => `${short(b)}: ${res[k][i].verdict} (bound ${f(res[k][i].bound)})`).join("\n    ")}`)
}

if (group("digits")) {
  console.log("\n## cstat-14 percentages: precision follows the interval (truth: the value itself)")
  const vs = [0.0003, -0.00004, 0.152, 0.0996, -0.077, 0]
  const res = all(vs.map((r) => ({ op: "pct", r })))
  for (const [i, r] of vs.entries()) console.log(`pct(${r})  ${bins.map((b, k) => `${short(b)} ${res[k][i].text}`).join("  ")}`)
  const cis: Q[] = [
    { op: "pct_ci", est: 0.0003, lo: 0.0002, hi: 0.0004 },
    { op: "pct_ci", est: 0.152, lo: 0.151, hi: 0.153 },
    { op: "pct_ci", est: 0.052, lo: 0.037, hi: 0.067 },
    { op: "pct_ci", est: 0.4, lo: 0.0004, hi: 0.8 },
    { op: "pct_vs", r: 0.152, threshold: 0.15 },
    { op: "bound", fraction: 0.003, relative: true, abs: 0.003, unit: "ms" },
    { op: "bound", fraction: 0.015, relative: true, abs: 0.015, unit: "ms" },
  ]
  // Older builds have no pct_ci/pct_vs/bound: they answer an error, printed as such.
  const r2 = all(cis)
  for (const [i, q] of cis.entries()) console.log(`${JSON.stringify(q)}  ${bins.map((b, k) => `${short(b)} ${r2[k][i].text ?? r2[k][i].error}`).join("  ")}`)
}

if (group("needed")) {
  console.log("\n## cstat-19 pairs a verdict needs (truth: the normal projection with a spread that is not 0)")
  // Over half the differences 0 (a quantized metric), the rest spread ±5%: its MAD is 0.
  const d = [0, 0, 0, 0, 0, 0, 0, 0.05, -0.05, 0.08, -0.06, 0.07]
  const qs: Q[] = [
    { op: "pairs_needed", d, bound: Math.log(1.02), confidence: 0.9842 },
    { op: "pairs_needed", d, bound: Math.log(1.02), confidence: 0.9842, power: 0.8 },
    { op: "pairs_for", sigma: 0.0001, dist: 0.05, confidence: 0.9842, power: 0.8 },
  ]
  const res = all(qs)
  const mean = d.reduce((a, b) => a + b, 0) / d.length
  const sd = Math.sqrt(d.reduce((a, b) => a + (b - mean) ** 2, 0) / (d.length - 1))
  console.log(`d=[${d}]  sample sd ${f(sd)}: a projection near ((2.41·${f(sd, 3)}/0.02)² / 0.955) ≈ ${Math.ceil(((2.41 * sd) / Math.log(1.02)) ** 2 / 0.955)} pairs (50%)`)
  for (const [i, q] of qs.entries()) console.log(`${JSON.stringify({ ...q, d: undefined })}  ${bins.map((b, k) => `${short(b)} ${res[k][i].pairs ?? res[k][i].error}`).join("  ")}`)
  console.log("  (pairs_for's floor is the fewest pairs whose interval reaches the confidence: 7 at 98.42%, 6 at 95%)")
}

if (group("median")) {
  console.log("\n## cstat-10 a median beside a Hodges–Lehmann interval (truth: the median must lie inside its interval)")
  const med = (v: number[]) => { const s = [...v].sort((a, b) => a - b); const m = s.length; return m % 2 ? s[(m - 1) / 2] : (s[m / 2 - 1] + s[m / 2]) / 2 }
  // Skewed pairs (most a little faster, some much slower), as a port's ln ratios can be.
  const cands = Array.from({ length: 400 }, () => Array.from({ length: 26 + Math.floor(rand() * 15) }, () => (rand() < 0.55 ? -0.01 + normal() * 0.001 : 0.05 + normal() * 0.01)))
  const hl0 = ask(bins[0], cands.map((d) => ({ op: "hl", d })))
  const found = cands.filter((v, i) => med(v) < hl0[i].lo || med(v) > hl0[i].hi).slice(0, 3)
  console.log(`${cands.filter((v, i) => med(v) < hl0[i].lo || med(v) > hl0[i].hi).length} of ${cands.length} samples have their median outside their HL interval`)
  const res = found.length ? all(found.flatMap((v) => [{ op: "hl", d: v }, { op: "median_interval", v }])) : []
  for (const [i, v] of found.entries()) {
    console.log(`n=${v.length} median ${f(med(v))}`)
    for (const [k, b] of bins.entries()) {
      const hl = res[k][2 * i], mi = res[k][2 * i + 1]
      const out = med(v) < hl.lo || med(v) > hl.hi
      console.log(`    ${short(b)}: printed median ${f(med(v))} with HL interval [${f(hl.lo)} … ${f(hl.hi)}]${out ? "  ← median outside" : ""}; HL point ${f(hl.est)}; median interval ${mi.error ? mi.error : `[${f(mi.lo)} … ${f(mi.hi)}] (${f(mi.achieved, 3)})`}`)
    }
  }
}

if (group("theil")) {
  console.log("\n## cstat-11 Theil-Sen 95% interval: coverage of the true slope (simulation, 3000 series of 10 points)")
  const qs: Q[] = []
  for (let k = 0; k < 3000; k++) qs.push({ op: "theil_sen", ys: Array.from({ length: 10 }, (_, i) => 2 * i + normal() * 3), confidence: 0.95 })
  const res = all(qs)
  for (const [k, b] of bins.entries()) {
    let below = 0, above = 0
    for (const r of res[k]) { if (r.hi < 2) below++; if (r.lo > 2) above++ }
    console.log(`${short(b)}  missed ${(((below + above) / qs.length) * 100).toFixed(1)}% (nominal ≤ 5%): interval below the slope ${((below / qs.length) * 100).toFixed(1)}%, above ${((above / qs.length) * 100).toFixed(1)}%`)
  }
}
