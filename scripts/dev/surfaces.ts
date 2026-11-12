// Dev check (not shipped): one experiment, many outputs, and they must agree (SPEC §1 rule 1). For each
// experiment it reads result.json, validates it against schema/result.v1.json, checks it is consistent
// with itself (decided ⇔ verdict, refused ⇒ no numbers, the interval holds the estimate, a better or
// worse verdict points the way the metric's direction says), then renders every surface with the
// strobe given and checks each number, sign, interval, verdict word, tally and refusal against
// result.json: the text (`query <exp> summary`), `report --format md`, `--format pr`, the HTML report
// (rendered by a headless Chromium: its text after the page's own script ran), the card's alt text and
// the badges, and `--format summary` (a comparison's is its PR block; a check's states and tally, never a
// time; a leak run's verdicts; a --blocks run's median and interval; no other process named). A --blocks
// run's text is checked against its HTML report (median, interval, blocks, declared metrics). A number
// that differs between surfaces is printed as CRIT.
//
// Times print in the unit asked (`--unit us|ms|s`, SPEC §7.1 "Times"): every surface is drawn once per
// unit (`--units`, default all three) and each time it shows must be, character for character, the text
// this file's `tTime` gives its stored number in that unit (the table in `scripts/dev/stats-check.ts`);
// the card's alt text too. `--json` must be the same bytes in every unit and under `LC_ALL=C`.
//
//   bun scripts/dev/surfaces.ts exp_2606af exp_a88c60 [--strobe <exe>] [--no-html] [--units us,ms]
//   bun scripts/dev/surfaces.ts --sample 2                    the newest 2 of every kind (minutes)
//   bun scripts/dev/surfaces.ts --all [--since 2026-11-01]    every surface of every experiment (hours:
//                                                             an idle machine's job)
//   bun scripts/dev/surfaces.ts --scan [--since 2026-11-01]   result.json checks over every experiment
//
// Exit 1 when anything CRIT or HIGH was found.
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "fs"
import { homedir, platform, tmpdir } from "os"
import { join } from "path"

type J = any
const argv = process.argv.slice(2)
const flag = (n: string) => argv.includes(`--${n}`)
const opt = (n: string) => (argv.includes(`--${n}`) ? argv[argv.indexOf(`--${n}`) + 1] : undefined)
const STROBE = opt("strobe") ?? "strobe"
const HOME =
  opt("dir") ??
  (platform() === "win32"
    ? join(process.env.LOCALAPPDATA ?? join(homedir(), "AppData/Local"), "strobe")
    : platform() === "darwin"
      ? join(homedir(), "Library/Application Support/strobe")
      : join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local/share"), "strobe"))
const SCHEMA = JSON.parse(readFileSync(new URL("../../schema/result.v1.json", import.meta.url), "utf8"))
// The units every surface is drawn in, and the one this pass draws (`--unit` on each query and report).
const UNITS = (opt("units") ?? "us,ms,s").split(",")
let UNIT = UNITS[0]

type Sev = "CRIT" | "HIGH" | "MED" | "LOW"
const found: { exp: string; sev: Sev; where: string; what: string }[] = []
let current = ""
const flagIt = (sev: Sev, where: string, what: string) => found.push({ exp: current, sev, where, what })

// ---------- formatting, as each surface does it ----------

// Rust's `{:.d}`: the nearest decimal, an exact tie to even (JS toFixed breaks a tie away from zero).
function rfix(v: number, d: number): string {
  if (!Number.isFinite(v)) return v.toFixed(d)
  const dv = new DataView(new ArrayBuffer(8))
  dv.setFloat64(0, Math.abs(v))
  const hi = dv.getUint32(0), lo = dv.getUint32(4), exp = (hi >>> 20) & 0x7ff
  let mant = (BigInt(hi & 0xfffff) << 32n) | BigInt(lo)
  const e = exp === 0 ? -1074 : exp - 1075
  if (exp !== 0) mant |= 1n << 52n
  if (e >= 0) return v.toFixed(d)
  const num = mant * 10n ** BigInt(d), den = 1n << BigInt(-e)
  if ((num % den) * 2n !== den) return v.toFixed(d)
  let q = num / den
  if (q % 2n === 1n) q += 1n
  const digits = q.toString().padStart(d + 1, "0")
  const body = d ? `${digits.slice(0, -d)}.${digits.slice(-d)}` : digits
  return (v < 0 && q !== 0n ? "-" : "") + body
}

// fmt::digits_for: decimals that show a number's first significant digit, from `min` to `max`.
function digitsFor(v: number, min: number, max: number): number {
  const a = Math.abs(v)
  if (!Number.isFinite(a) || a === 0) return min
  let d = 0
  while (d <= max && Math.round(a * 10 ** d) < 1) d++
  if (d > max) d = min
  return Math.min(max, Math.max(min, d))
}

// fmt::time: a time stored in ms, in `unit` (SPEC §7.1 "Times"). Every whole digit grouped by threes,
// at least three significant ones, at most the unit's finest decimals (taken again from the rounded
// value: 99.96 ms is `100 ms`); `<0.01 µs` for a nonzero one that rounds to nothing; `0 µs` for 0.
const TU: Record<string, { label: string; perMs: number; finest: number }> = {
  us: { label: "µs", perMs: 1000, finest: 2 },
  ms: { label: "ms", perMs: 1, finest: 4 },
  s: { label: "s", perMs: 0.001, finest: 6 },
}
if (UNITS.some((u) => !TU[u])) {
  console.error(`--units takes us, ms and s, not ${UNITS.join(",")}`)
  process.exit(2)
}
const group = (s: string) => s.replace(/^(-?)(\d+)/, (_, sign, int) => sign + int.replace(/\B(?=(\d{3})+(?!\d))/g, ","))
function timeDigits(x: number, finest: number): number {
  const d = (x: number) => (x >= 100 ? 0 : x >= 10 ? 1 : x >= 1 ? 2 : Math.min(finest, [3, 4, 5, 6].find((k) => k <= Math.max(3, finest) && x * 10 ** k >= 100) ?? finest))
  const first = d(x)
  return Math.min(d(parseFloat(rfix(x, first))), first)
}
function tTime(ms: number | null, signed = false, unit = UNIT): string {
  if (ms == null || !Number.isFinite(ms)) return "—"
  const t = TU[unit], x = Math.abs(ms * t.perMs)
  const s = x === 0 ? "0" : rfix(x, timeDigits(x, t.finest))
  const body = x === 0 ? "0" : /[1-9]/.test(s) ? group(s) : `<${rfix(10 ** -t.finest, t.finest)}`
  return `${ms < 0 ? "−" : ms > 0 && signed ? "+" : ""}${body} ${t.label}`
}
// stats::verdict_text: a time's absolute bound or margin drawn again from `bound` in the unit shown.
function vText(c: J): string {
  const v = String(c.verdict ?? "")
  if (c.relative !== false || unitOf(c.metric) !== "ms" || !(Number.isFinite(c.bound) && c.bound >= 0)) return v
  for (const p of ["same (±", "no worse (≤", "worse (>"]) {
    if (!v.startsWith(p)) continue
    const ends = [v.indexOf(")", p.length), v.indexOf(", ", p.length)].filter((i) => i >= 0)
    if (ends.length) return p + tTime(c.bound) + v.slice(Math.min(...ends))
  }
  return v
}

// app::fmt::value / pct / pct_ci / signed (the text and the markdown report).
function tValue(v: number | null, unit: string): string {
  if (v == null || !Number.isFinite(v)) return "—"
  if (unit === "ms") return tTime(v)
  let d = Math.abs(v) >= 100 ? 0 : Math.abs(v) >= 10 ? 1 : digitsFor(v, 2, 4)
  if (Math.round(v * 10 ** d) === 0) {
    v = 0
    d = Math.min(d, 2)
  }
  switch (unit) {
    case "%": return `${rfix(v, d)}%`
    case "count": return group(Math.abs(v - Math.round(v)) < 1e-9 ? String(Math.round(v)) : rfix(v, 1))
    case "bytes": return tBytes(v)
    case "pp": return `${rfix(v, d)} pp`
    case "MB": return `${rfix(v, d)} MB`
    case "/s": return `${rfix(v, d)}/s`
    case "value": return rfix(v, d)
    case "instructions": {
      const a = Math.abs(v)
      return a >= 1e9 ? `${rfix(v / 1e9, 2)} G instr` : a >= 1e7 ? `${rfix(v / 1e6, 1)} M instr` : a >= 1e6 ? `${rfix(v / 1e6, 2)} M instr` : a >= 1e3 ? `${rfix(v / 1e3, 0)} k instr` : `${rfix(v, 0)} instr`
    }
    default: return `${rfix(v, d)} ${unit}`
  }
}
function tBytes(v: number): string {
  const a = Math.abs(v)
  const scaled = (x: number, unit: string) => `${rfix(x, Math.abs(x) >= 100 ? 0 : Math.abs(x) >= 10 ? 1 : 2)} ${unit}`
  return a >= 1024 ** 3 ? scaled(v / 1024 ** 3, "GB") : a >= 1024 ** 2 ? scaled(v / 1024 ** 2, "MB") : a >= 1024 ? `${rfix(v / 1024, Math.abs(v / 1024) >= 10 ? 0 : 1)} KB` : `${rfix(v, 0)} B`
}
function pctWith(r: number, digits: number): string {
  const p = r * 100
  const zero = Math.round(Math.abs(p) * 10 ** digits) === 0
  return `${zero ? "" : p < 0 ? "−" : "+"}${rfix(Math.abs(p), digits)}%`
}
const pctDefault = (p: number) => (Math.abs(p) >= 10 ? 0 : digitsFor(p, 1, 3))
function tPct(r: number | null): string {
  return r == null || !Number.isFinite(r) ? "—" : pctWith(r, pctDefault(r * 100))
}
// An estimate and its interval at the interval's digits.
function pctCI(est: number, lo: number, hi: number): [string, string, string] {
  if (!(Number.isFinite(lo) && Number.isFinite(hi)) || hi === lo) return [tPct(est), tPct(lo), tPct(hi)]
  // fmt::width_digits: as the Particle Data Group rounds an uncertainty.
  const w = Math.abs(hi * 100 - lo * 100)
  const exp = Math.floor(Math.log10(w))
  const common = Math.min(3, Math.max(0, (w / 10 ** (exp - 2) < 354.5 ? 2 : 1) - 1 - exp))
  const own = (x: number) => pctWith(x, Math.max(common, x === 0 ? 0 : digitsFor(x * 100, 0, 3)))
  return [own(est), own(lo), own(hi)]
}
const relCI = (c: J): [string, string, string] => ([c.estimate, c.lo, c.hi].every(Number.isFinite) ? pctCI(c.estimate, c.lo, c.hi) : [tPct(c.estimate), tPct(c.lo), tPct(c.hi)])
function tSigned(v: number | null, unit: string): string {
  if (v == null || !Number.isFinite(v)) return "—"
  if (unit === "ms") return tTime(v, true)
  const body = tValue(Math.abs(v), unit)
  return !/[1-9]/.test(body) ? body : v < 0 ? `−${body}` : v > 0 ? `+${body}` : body
}

// pr::extract::disp / signed_disp (the PR block and the card): `fmt::value` and `fmt::signed` since
// times print in one unit (no 3 significant figures, no second for a time past 1,000 ms).
const pDisp = (v: number | null, unit: string) => tValue(v, unit || "count")
const pSigned = (v: number, unit: string) => tSigned(v, unit || "count")

// ---------- reading result.json ----------

let CATALOG = new Map<string, J>()
function def(m: string): J | undefined {
  const base = m.split("{")[0]
  if (CATALOG.has(base)) return CATALOG.get(base)
  const fam = base.split(".")[0]
  const famName = base.startsWith("launch.") ? "launch.milestone" : fam
  return ["count", "phase", "span", "metric", "app", "launch.milestone"].includes(famName) ? CATALOG.get(famName) : undefined
}
const unitOf = (m: string) => def(m)?.unit ?? ""
const absUnit = (u: string) => (u === "%" ? "pp" : u)
// The direction a comparison counted as better: its own `better`, else its blocks' lines for a
// metric.<name>, else the catalog's.
function higher(s: J, c: J): boolean {
  if (c.better) return c.better === "higher"
  if (c.metric?.startsWith("metric.")) {
    const dirs = blocksFor(s, c.metric).map((b: J) => b.directions?.[c.metric]).filter(Boolean)
    if (dirs.length && dirs.every((d: string) => d === "higher")) return true
  }
  return def(c.metric)?.better === "higher"
}
function blocksFor(s: J, m: string): J[] {
  const p = s.pass
  if (p?.blocks?.length && (m === "cpu.instructions") === !!p.counters) return p.blocks
  return s.blocks ?? []
}
function pairs(s: J, m: string, below = Infinity): [number, number][] {
  const by = new Map<number, J>()
  for (const b of blocksFor(s, m)) {
    if (!Number.isFinite(b.pair) || b.pair >= 1e6) continue
    const e = by.get(b.pair) ?? {}
    e[b.variant] = b
    by.set(b.pair, e)
  }
  return [...by.entries()].filter(([p]) => p < below).sort((a, b) => a[0] - b[0]).map(([, e]) => [e.base?.values?.[m], e.head?.values?.[m]] as [number, number]).filter(([a, b]) => Number.isFinite(a) && Number.isFinite(b))
}
function tally(s: J, c: J) {
  // The primary's tally is of the pairs its verdict is of (`primary_pairs`, the look that decided it).
  const ps = pairs(s, c.metric, c === s.primary && Number.isFinite(s.primary_pairs) ? s.primary_pairs : Infinity), up = higher(s, c)
  return { better: ps.filter(([a, b]) => (up ? b > a : b < a)).length, worse: ps.filter(([a, b]) => (up ? b < a : b > a)).length, n: ps.length }
}
const rows = (s: J): { c: J; role: string }[] => [
  ...(s.primary?.metric && "verdict" in s.primary ? [{ c: s.primary, role: "primary" }] : []),
  ...(s.secondaries ?? []).map((c: J) => ({ c, role: "secondary" })),
  ...(s.descriptive ?? []).map((c: J) => ({ c, role: "descriptive" })),
]
// stats::kind_of: verdict_kind, or in results from before it, what the verdict's words say.
function kindOf(s: J, c: J): string {
  if (c.verdict_kind) return c.verdict_kind
  const v = String(c.verdict ?? "").trim(), first = v.split(/[ :,]/)[0]
  const words: string[] = def(c.metric)?.words ?? []
  const [good, bad] = higher(s, c) !== (def(c.metric)?.better === "higher") ? [words[1], words[0]] : words
  return first === "—" ? "untested" : first === "·" ? "descriptive" : first === "refused" ? "refused" : v.startsWith("no worse") ? "no_worse"
    : first === "worse" || first === "leaks" ? "worse" : ["same", "underpowered", "saturated", "stopped"].includes(first) ? first
    : first === good ? "better" : first === bad ? "worse" : "unclear"
}
// A bisect's `series` is the label it bisects, not a list.
const seriesOf = (r: J): J[] => (Array.isArray(r.series) ? r.series : [])
const compared = (s: J) => ["runs", "launch"].includes(s.kind) && s.not_run !== true
const DECIDED = new Set(["better", "worse", "same", "no_worse"])

// ---------- schema (the draft-07 subset result.v1.json uses) ----------

function validate(v: J, sc: J, path: string, root: J, out: string[]) {
  if (sc.$ref) return validate(v, sc.$ref.split("/").slice(1).reduce((o: J, k: string) => o[k], root), path, root, out)
  if (sc.anyOf || sc.oneOf) {
    const alts = sc.anyOf ?? sc.oneOf
    const errs = alts.map((a: J) => { const e: string[] = []; validate(v, a, path, root, e); return e })
    const ok = errs.filter((e: string[]) => !e.length).length
    if (sc.anyOf ? ok === 0 : ok !== 1) {
      // Say why the closest alternative failed, not just that none matched.
      const depth = (e: string[]) => Math.max(0, ...e.map((x) => x.indexOf(":")))
      const best = errs.reduce((a: string[], b: string[]) => (depth(b) > depth(a) ? b : a))
      out.push(ok ? `${path}: matches ${ok} of oneOf` : best.length ? best[0] : `${path}: matches none of ${sc.anyOf ? "anyOf" : "oneOf"}`)
    }
    return
  }
  if ("const" in sc && v !== sc.const) out.push(`${path}: ${JSON.stringify(v)} is not ${JSON.stringify(sc.const)}`)
  if (sc.enum && !sc.enum.includes(v)) out.push(`${path}: ${JSON.stringify(v)} not in enum`)
  if (sc.type) {
    const t = Array.isArray(sc.type) ? sc.type : [sc.type]
    const kind = v === null ? "null" : Array.isArray(v) ? "array" : Number.isInteger(v) ? "integer" : typeof v
    if (!t.some((x: string) => x === kind || (x === "number" && kind === "integer"))) { out.push(`${path}: ${kind}, schema says ${t.join("|")}`); return }
  }
  if (v && typeof v === "object" && !Array.isArray(v)) {
    for (const r of sc.required ?? []) if (!(r in v)) out.push(`${path}: missing ${r}`)
    for (const [k, x] of Object.entries(v)) {
      if (sc.properties?.[k]) validate(x, sc.properties[k], `${path}.${k}`, root, out)
      else if (sc.additionalProperties === false) out.push(`${path}: unknown key ${k}`)
      else if (typeof sc.additionalProperties === "object") validate(x, sc.additionalProperties, `${path}.${k}`, root, out)
    }
  }
  if (Array.isArray(v) && sc.items) v.forEach((x, i) => validate(x, sc.items, `${path}[${i}]`, root, out))
}

// ---------- result.json on its own ----------

function checkResult(r: J) {
  const errs: string[] = []
  validate(r, SCHEMA, "$", SCHEMA, errs)
  for (const e of errs.slice(0, 12)) flagIt("MED", "schema", e)
  if (errs.length > 12) flagIt("MED", "schema", `${errs.length - 12} more`)
  for (const s of Array.isArray(r.series) ? r.series : []) {
    if (!compared(s)) continue
    for (const { c, role } of rows(s)) {
      const where = `${s.label} ${c.metric}`
      const k = kindOf(s, c)
      if (role !== "descriptive" && c.decided !== DECIDED.has(k)) flagIt("CRIT", where, `decided ${c.decided} beside verdict_kind ${k} (${c.verdict})`)
      // Results from before Comparison::refuse cleared them (and before verdict_kind) still hold numbers.
      if (k === "refused" && [c.base, c.head, c.estimate, c.lo, c.hi].some((x) => x != null)) flagIt(c.verdict_kind ? "CRIT" : "LOW", where, "refused, yet result.json holds numbers")
      if ([c.lo, c.estimate, c.hi].every(Number.isFinite) && !(c.lo <= c.estimate + 1e-12 && c.estimate <= c.hi + 1e-12)) flagIt("CRIT", where, `estimate ${c.estimate} outside [${c.lo}, ${c.hi}]`)
      if ((k === "better" || k === "worse") && Number.isFinite(c.hi)) {
        const improved = (c.hi < 0) !== higher(s, c)
        if (improved !== (k === "better") && !(c.lo <= 0 && c.hi >= 0)) flagIt("CRIT", where, `verdict_kind ${k} but the interval [${c.lo}, ${c.hi}] says ${improved ? "better" : "worse"} for a metric better ${higher(s, c) ? "higher" : "lower"}`)
      }
      const n = pairs(s, c.metric, role === "primary" && Number.isFinite(s.primary_pairs) ? s.primary_pairs : Infinity).length
      if (Number.isInteger(c.n) && k !== "refused" && n && c.n !== n) flagIt("MED", where, `n ${c.n}, the blocks hold ${n} pairs with both values`)
    }
  }
}

// ---------- the surfaces ----------

// A query or report in this pass's unit.
function run(args: string[], unit: string | null = UNIT, env: Record<string, string> = {}): { out: string; err: string; code: number } {
  const p = Bun.spawnSync([STROBE, ...args, ...(unit ? ["--unit", unit] : [])], { stdout: "pipe", stderr: "pipe", env: { ...process.env, ...env } })
  return { out: p.stdout.toString(), err: p.stderr.toString(), code: p.exitCode ?? 0 }
}

function chrome(): string | undefined {
  if (process.env.CHROME) return process.env.CHROME
  const pw = platform() === "win32" ? join(process.env.LOCALAPPDATA ?? "", "ms-playwright") : platform() === "darwin" ? join(homedir(), "Library/Caches/ms-playwright") : join(homedir(), ".cache/ms-playwright")
  if (existsSync(pw)) {
    for (const d of readdirSync(pw).filter((d) => d.startsWith("chromium_headless_shell")).sort().reverse()) {
      for (const sub of readdirSync(join(pw, d))) {
        const exe = join(pw, d, sub, platform() === "win32" ? "chrome-headless-shell.exe" : "chrome-headless-shell")
        if (existsSync(exe)) return exe
      }
    }
  }
}

function domText(html: string): string {
  let t = html.replace(/<script[\s\S]*?<\/script>/gi, "").replace(/<style[\s\S]*?<\/style>/gi, "")
  t = t.replace(/<(br|\/div|\/p|\/tr|\/li|\/h\d|\/section|\/summary|\/header)[^>]*>/gi, "\n").replace(/<[^>]+>/g, " ")
  t = t.replace(/&nbsp;/g, " ").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(+n)).replace(/&amp;/g, "&")
  return t.split("\n").map((l) => l.replace(/[ \t\u00a0]+/g, " ").trim()).filter(Boolean).join("\n")
}

function htmlSurface(id: string, dir: string): { text: string; labels: Record<string, string> } | undefined {
  const exe = chrome()
  const file = join(dir, "report.html")
  if (run(["report", "experiment", id, "--out", file]).code !== 0 || !existsSync(file)) { flagIt("HIGH", "html", "report failed"); return }
  const src = readFileSync(file, "utf8")
  const line = src.split("\n").find((l) => l.startsWith("const STROBE = "))
  if (line) CATALOG = new Map(JSON.parse(line.slice(15).replace(/;\s*$/, "")).metrics.map((m: J) => [m.name, m]))
  const lb = /const LABELS = (\{[\s\S]*?\n\});/.exec(src)
  const labels = lb ? Function(`return ${lb[1]}`)() : {}
  if (!exe || flag("no-html")) return { text: "", labels }
  const p = Bun.spawnSync([exe, "--no-sandbox", "--disable-gpu", `--user-data-dir=${join(dir, "prof")}`, "--virtual-time-budget=5000", "--window-size=1400,4000", "--dump-dom", `file:///${file.replace(/\\/g, "/")}`], { stdout: "pipe", stderr: "pipe", timeout: 60000 })
  return { text: domText(p.stdout.toString()), labels }
}

const strip = (s: string) => s.replace(/\*\*/g, "").replace(/<sub>.*?<\/sub>/g, "").replace(/\u00a0/g, " ").replace(/&lt;/g, "<").replace(/&gt;/g, ">").trim()

// `handle`: the experiment's id, or the path of a result.json given on the command line (a fixture).
function checkSurfaces(r: J, handle: string = r.id) {
  const id = handle
  const dir = mkdtempSync(join(tmpdir(), "strobe-surfaces-"))
  try {
    // Every result has an HTML and md report; `bench --blocks` has no PR block, card or badge (it compares nothing).
    const reportable = true
    const blocksRun = seriesOf(r).some((s: J) => s.kind === "blocks")
    // The HTML is checked where something was compared, and a --blocks run's blocks (a --check's page says passed or failed).
    const html = seriesOf(r).some((s: J) => compared(s) || s.kind === "blocks") ? htmlSurface(id, dir) : undefined
    const text = run(["query", id, "summary"]).out
    const md = reportable ? run(["report", "experiment", id, "--format", "md"]).out : text
    // The step summary: a comparison's is its PR block; a check, pixels, leak or --blocks run's is a table of its own.
    const summary = run(["report", "experiment", id, "--format", "summary"])
    if (summary.code !== 0) flagIt("HIGH", "summary", `--format summary failed (exit ${summary.code}): ${summary.err.trim().split("\n")[0]}`)
    else checkSummary(r, summary.out, text)
    if (blocksRun) for (const s of seriesOf(r).filter((s: J) => s.kind === "blocks")) checkBlocks(s, text, html?.text ?? "")
    const judgedAny = seriesOf(r).some((s: J) => compared(s))
    const pr = judgedAny ? run(["report", "experiment", id, "--format", "pr"]).out : ""
    // A comparison's step summary is its PR block as `--format pr` writes it (no card or badges either way here).
    if (judgedAny && r.kind === "experiment" && summary.code === 0 && summary.out.trim() !== pr.trim()) flagIt("MED", "summary", "a comparison's --format summary differs from its --format pr block")
    const card = judgedAny ? JSON.parse(run(["report", "experiment", id, "--format", "card", "--json", "--out", join(dir, "card.svg")]).out || "{}") : {}
    const badge = judgedAny ? JSON.parse(run(["report", "experiment", id, "--format", "badge", "--json", "--out", join(dir, "badge.svg")]).out || "{}") : {}
    if (UNIT === UNITS[0] && r.text && r.text.trim() !== text.trim()) flagIt("LOW", "text", "the text bench printed and `query summary` now differ (a later strobe renders it otherwise)")
    // `--format md` of a comparison is drawn again from result.json (`bench::render`: today's digits and
    // ledger), so it is checked against result.json as the stored text is, row by row; where it prints the
    // stored text unchanged (a check, a recording, an older strobe) the text's check covers it.
    const mdBody = md.replace(/^```\n?|\n?```\s*$/g, "")
    const texts: [string, string][] = [["text", text]]
    if (reportable && mdBody.trim() !== text.trim()) texts.push(["md", mdBody])
    const sectionsOf = (t: string) => t.split(/^(?=strobe bench )/m)
    const prRows = pr.split("\n").filter((l) => l.startsWith("|")).map((l) => l.split(/(?<!\\)\|/).slice(1, -1).map(strip))
    const impl = r.comparison === "implementations"
    // An A/A: a better or worse verdict between identical builds is a false positive on every surface.
    const aa = r.plan?.aa === true
    for (const s of seriesOf(r)) {
      if (!compared(s)) continue
      const secs = texts.map(([name, t]) => [name, sectionsOf(t).find((x) => x.split("\n")[0].includes(`· ${s.label}`)) ?? ""] as const)
      // A stored text that never printed this series (a result rewritten after its bench printed).
      for (const [name, sec] of secs) if (!sec) flagIt("MED", `${name} ${s.label}`, `the ${name === "text" ? "stored text" : "md render"} has no section for this series`)
      for (const { c, role } of rows(s)) {
        const where = `${s.label} ${c.metric}`
        const u = unitOf(c.metric), k = kindOf(s, c), refused = k === "refused"
        for (const [name, sec] of secs) {
          // Text (what the bench printed, kept in result.json and shown by `query summary`, so an older
          // strobe's digits) and md (drawn again now): each number within its own printed rounding of
          // result.json's, the verdict word for word, and no number on a refused row.
          // The primary's row is starred; a secondary of the same name (results from before the primary
          // was kept out of its secondaries) is the unstarred one.
          const row = sec.split("\n").find((l) => new RegExp(`^${role === "primary" ? "★" : " "} ${c.metric.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s`).test(l))
          if (!row) {
            if (sec) flagIt("HIGH", `${name} ${where}`, "no row")
          } else {
            // The numbers between the metric's name and its verdict, in order: base, head, Δ, CI's ends.
            const body = row.trim().replace(/^★\s*/, "").slice(c.metric.length).split(vText(c))[0]
            const toks = body.match(/—|[−+-]?<?\d[\d.,]*(?: ?(?:[GMk] instr|instr|µs|ms|pp|MB|KB|GB|B|score|s(?![a-z])|%|\/s))?/g) ?? []
            const [tb, th, td] = toks
            const tci = toks.length >= 5 ? `${toks[3]} … ${toks[4]}` : toks[3]
            if (!row.includes(vText(c))) flagIt("CRIT", `${name} ${where}`, `verdict "${vText(c)}" not in: ${row.trim()}`)
            if (refused) {
              if ([tb, th, td, tci].some((x) => /\d/.test(x ?? ""))) flagIt(c.verdict_kind ? "CRIT" : "LOW", `${name} ${where}`, `refused, yet the row reads ${row.trim()}`)
            } else {
              const [lo, hi] = (tci ?? "").split(" … ")
              const scaleU = c.relative ? "%" : absUnit(u)
              const checks: [string | undefined, number, string][] = [[tb, c.base, u], [th, c.head, u], [td, c.relative ? c.estimate * 100 : c.estimate, scaleU]]
              if (!c.exact) checks.push([lo, c.relative ? c.lo * 100 : c.lo, scaleU], [hi, c.relative ? c.hi * 100 : c.hi, scaleU])
              for (const [tok, v, unit] of checks) if (!agrees(tok, v, unit)) flagIt("CRIT", `${name} ${where}`, `"${tok}" does not round from ${v} (${unit}): ${row.trim()}`)
              // A time is the unit's text of its number, to the character (the change and its ends signed).
              for (const [k, [tok, v, unit]] of checks.entries()) {
                const want = unit !== "ms" ? tok : k < 2 ? tTime(v) : tTime(v, true)
                if (tok !== want && agrees(tok, v, unit)) flagIt("CRIT", `${name} ${where}`, `"${tok}" where ${UNIT} prints "${want}": ${row.trim()}`)
              }
            }
          }
        }
        // PR: tested rows carry the same change and range; a refused one no number.
        // (A secondary of the primary's own name, in results from before it was kept out, is not shown.)
        if (pr && role !== "descriptive" && !(role === "secondary" && c.metric === s.primary?.metric)) {
          const ch = refused ? "—" : c.relative ? relCI(c)[0] : pSigned(c.estimate, absUnit(u))
          const rg = refused ? "—" : c.exact ? "exact" : [c.lo, c.hi].every(Number.isFinite) ? (c.relative ? [relCI(c)[1], relCI(c)[2]] : [pSigned(c.lo, absUnit(u)), pSigned(c.hi, absUnit(u))]).filter((x, i, a) => i === 0 || x !== a[0]).join(" to ") : "—"
          // Two implementations read an increase of 2× or more as a factor (`+96% to 2.0×`).
          const sameChange = (x: string) => x === ch || (/×$/.test(x) && Math.abs(parseFloat(x) - (1 + c.estimate)) < 0.06)
          // Every row with these numbers (two metrics can share them): the verdict must be on one of them.
          const hits = prRows.filter((cells) => (cells.includes(ch) && cells.includes(rg)) || (impl && c.relative && !refused && cells.length >= 6 && sameChange(cells[3]) && /×|%/.test(cells[4])))
          const word = aa && (k === "better" || k === "worse") ? "false positive" : vText(c).replace(/ \(small\)$/, "")
          if (!hits.length) flagIt("HIGH", `pr ${where}`, `no Metrics row with change ${ch} and range ${rg}`)
          else if (refused && hits.every((h) => h.slice(1, 3).some((x) => /\d/.test(x)))) flagIt("CRIT", `pr ${where}`, `refused, yet the row shows ${hits[0].join(" | ")}`)
          else if ((k === "better" || k === "worse") && !hits.some((h) => h.some((x) => x.startsWith(word)))) flagIt("CRIT", `pr ${where}`, `verdict ${word} not in its row: ${hits[0].join(" | ")}`)
          else if (k === "untested" && !hits.some((h) => h.some((x) => x.startsWith("not tested")))) flagIt("HIGH", `pr ${where}`, `untested, yet the row reads ${hits[0].join(" | ")}`)
          // Its medians as the text prints them, in this unit.
          else if (!impl && !refused && u === "ms" && !hits.some((h) => h[1] === pDisp(c.base, u) && h[2] === pDisp(c.head, u))) flagIt("CRIT", `pr ${where}`, `medians "${hits[0][1]} | ${hits[0][2]}", where ${UNIT} prints "${pDisp(c.base, u)} | ${pDisp(c.head, u)}"`)
        }
        // HTML: the forest row's base, head, paired change, range and outcome, for the series the page
        // opens on (its hero names it: `<label> · Primary: …`).
        if (html?.text && html.text.includes(`${s.label} · Primary:`)) {
          const label = html.labels[c.metric] ?? c.metric
          const lines = html.text.split("\n")
          const head = `${role === "primary" ? "★ " : ""}${label} `
          const want = refused ? `${head}— — —` : `${head}${hFmt(c.base, u)} ${hFmt(c.head, u)} ${hDelta(c, u)}`
          let i = lines.findIndex((l) => l === want || l.startsWith(`${want} `))
          if (i < 0) {
            // The row with other digits or units: the same numbers formatted otherwise is a lesser fault.
            const j = lines.findIndex((l) => l.startsWith(head) && /\d|—/.test(l.slice(head.length, head.length + 2)))
            const nums = (s: string) => (s.replace(/(?<=\d),(?=\d{3}(?!\d))/g, "").match(/[−+-]?\d+(\.\d+)?/g) ?? []).map((x) => parseFloat(x.replace("−", "-")))
            const same = j >= 0 && nums(lines[j].slice(head.length)).slice(0, 3).every((x, n) => Math.abs(x - nums(want.slice(head.length))[n]) <= 0.01 * Math.max(1, Math.abs(x)))
            if (j < 0) flagIt(refused ? "CRIT" : "HIGH", `html ${where}`, `no forest row "${want}"`)
            else flagIt(same ? "LOW" : refused ? "CRIT" : "HIGH", `html ${where}`, `forest row "${lines[j]}", the text's numbers read "${want}"`)
            i = j
          }
          if (i >= 0) {
            const next = lines[i + 1] ?? ""
            const want = refused ? /Refused/ : role === "descriptive" ? /Context$/ : k === "better" || k === "worse" ? new RegExp(`Confirmed ${(c.hi < 0) !== higher(s, c) ? "better" : "worse"}$|False positive$`) : k === "same" ? /Same \(/ : k === "no_worse" ? /No worse/ : k === "untested" ? /Not tested$/ : /Inconclusive|Unconfirmed/
            if (!want.test(next)) flagIt("CRIT", `html ${where}`, `outcome "${next}" for verdict_kind ${k}`)
            // A time's absolute bound, drawn again in the unit shown (`Same (±40.0 µs)`).
            else if ((k === "same" || k === "no_worse") && vText(c) !== c.verdict && !next.includes(tTime(c.bound))) flagIt("CRIT", `html ${where}`, `outcome "${next}", where ${UNIT} prints its bound ${tTime(c.bound)}`)
          }
        }
      }
      // The primary's tally and direction on the proof line, the HTML hero, the card and the badge.
      const p = s.primary, k = p?.metric ? kindOf(s, p) : undefined
      if ((k === "better" || k === "worse") && !aa) {
        const t = tally(s, p), word = k === "better" ? "better" : "worse", count = k === "better" ? t.better : t.worse
        if (pr && !impl && !pr.includes(`${word} in ${count} of ${t.n} pairs`)) flagIt("CRIT", `pr ${s.label}`, `proof line lacks "${word} in ${count} of ${t.n} pairs"`)
        if (card.alt && !impl && seriesOf(r).filter(compared).length === 1 && !card.alt.includes(`${word} in ${count} of ${t.n} pairs`)) flagIt("CRIT", `card ${s.label}`, `alt "${card.alt}" lacks "${word} in ${count} of ${t.n} pairs"`)
        const hero = !html?.text.includes(`${s.label} · Primary:`) ? null : html.text.match(/(outperformed|trailed) .*? in (?:all )?(\d+) of (\d+)/)
        if (hero && (hero[1] === "outperformed") !== (k === "better")) flagIt("CRIT", `html ${s.label}`, `hero says ${hero[1]} for a ${k} verdict`)
        if (hero && +hero[2] !== count) flagIt("CRIT", `html ${s.label}`, `hero counts ${hero[2]} of ${hero[3]} pairs, the blocks ${count} of ${t.n}`)
      }
      const b = (badge.series ?? []).find((x: J) => x.series === s.label || x.series?.startsWith(`${s.label} `))
      // A journey decided on its own but not after Holm is grey by design.
      if (b && !(impl && p?.holm_decided === false && (k === "better" || k === "worse"))) {
        const colour = /-([0-9a-f]{6})\?/.exec(b.url ?? "")?.[1]
        const want = aa ? "6e7781" : k === "better" ? "1a7f37" : k === "worse" ? "cf222e" : "6e7781"
        if (colour && colour !== want) flagIt("CRIT", `badge ${s.label}`, `colour ${colour} (${b.value}) for verdict_kind ${k}`)
        if (k === "refused" && /\d/.test(b.value)) flagIt("CRIT", `badge ${s.label}`, `refused, yet the badge reads ${b.value}`)
        if ((k === "better" || k === "worse") && /^[+−-]/.test(b.value) && (b.value.startsWith("+") !== p.estimate > 0)) flagIt("CRIT", `badge ${s.label}`, `sign of ${b.value} against estimate ${p.estimate}`)
      }
    }
    // The card's alt text: each time it holds is one of the shown series' primary numbers as this unit
    // prints it, never another unit's.
    if (card.alt) {
      const want = new Set<string>(), nums: number[] = []
      for (const s of seriesOf(r).filter(compared)) {
        const p = s.primary ?? {}
        if (unitOf(p.metric ?? "") !== "ms") continue
        for (const v of [p.base, p.head, p.bound].filter(Number.isFinite)) want.add(tTime(v)), nums.push(v)
        for (const v of [p.estimate, p.lo, p.hi].filter(Number.isFinite)) want.add(tTime(v, true)).add(tTime(Math.abs(v))), nums.push(v, Math.abs(v))
      }
      // A time that rounds from one of those numbers (others, a wait's `30 s`, are not the card's numbers).
      for (const m of card.alt.matchAll(/[−+]?<?\d[\d,]*(?:\.\d+)? ?(?:µs|ms|s)\b/g)) {
        if (nums.some((v) => agrees(m[0], v, "ms")) && !want.has(m[0])) flagIt("CRIT", "card", `alt "${card.alt}": ${m[0]} is not how ${UNIT} prints its number`)
      }
    }
    // Every surface's times are whole: digits grouped by threes, a shared-unit pair (`1,200–2,200 µs`,
    // `7,050 → 3,900 µs`, `−24 … −16 µs`) holds both its numbers (once `1–2,200 µs`), and nothing is −0.
    checkWhole([["text", text], ["md", mdBody], ["pr", pr], ["summary", summary.out], ["card", card.alt ?? ""], ["html", html?.text ?? ""]])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

// Times printed whole on each surface: a time's digits grouped by threes, no number printed as minus zero,
// and every pair that names its unit once (`1,200–2,200 µs`, `7,050 → 3,900 µs`) kept for `checkPairs`,
// which holds each pair to the same pair in the other units (a first number cut at its comma, once
// `1–2,200 µs`, reads 1,000 times smaller than its `0.00120–0.00220 s`).
const PAIRS = new Map<string, { text: string; ends: [number, number][] }[]>()
function checkWhole(surfaces: [string, string][]) {
  const N = "[−+-]?<?\\d[\\d,]*(?:\\.\\d+)?"
  const pair = new RegExp(`(?<![\\d.,])(${N})[ \\u00a0]?(?:→|–|to|…)[ \\u00a0]?(${N})[ \\u00a0](µs|us|ms|s)(?![\\w/])`, "g")
  for (const [name, t] of surfaces) {
    for (const m of t.matchAll(/(?<![\d.,])(\d[\d,]*)(?:\.\d+)?[ \u00a0](?:µs|us|ms)(?![\w/])/g)) {
      const groups = m[1].split(",")
      if (groups.length > 1 && (groups[0].length > 3 || groups.slice(1).some((g) => g.length !== 3))) flagIt("CRIT", name, `"${m[0]}" is not grouped by threes`)
    }
    const list = [...t.matchAll(pair)].map((m) => ({ text: m[0], ends: [m[1], m[2]].map((e) => span(`${e} ${m[3]}`)) as [number, number][] }))
    PAIRS.set(`${name} ${UNIT}`, list)
    for (const m of t.matchAll(/(?<![\w.,−-])[−-]0(?:\.0+)?(?=[ \u00a0]|%|$|[),;])/gm)) flagIt("HIGH", name, `minus zero "${m[0]}" in: ${t.slice(Math.max(0, m.index! - 40), m.index! + 20).replace(/\n/g, " ")}`)
  }
}
// A printed time's possible values in ms: [low, high] (its rounding; `<0.01 µs` from 0 up to the step).
function span(tok: string): [number, number] {
  const m = /([−+-]?)(<?)(\d+(?:\.(\d+))?)\s*(µs|us|ms|s)/.exec(tok.replace(/,/g, ""))
  if (!m) return [NaN, NaN]
  const scale = m[5] === "s" ? 1000 : m[5] === "ms" ? 1 : 0.001, x = parseFloat(m[3]) * scale, half = 0.5 * 10 ** -(m[4]?.length ?? 0) * scale
  const [lo, hi] = m[2] ? [0, x] : [x - half, x + half]
  return m[1] === "−" || m[1] === "-" ? [-hi, -lo] : [lo, hi]
}
// The same pair on the same surface in every unit: its ends must overlap.
function checkPairs() {
  const names = new Set([...PAIRS.keys()].map((k) => k.slice(0, k.lastIndexOf(" "))))
  for (const name of names) {
    const per = UNITS.map((u) => PAIRS.get(`${name} ${u}`) ?? [])
    if (per.some((l) => l.length !== per[0].length)) continue
    for (const [i, p] of per[0].entries()) {
      for (const [k, other] of per.entries()) {
        const q = other[i]
        const apart = [0, 1].some((e) => !(p.ends[e][0] <= q.ends[e][1] * (1 + 1e-9) + 1e-12 && q.ends[e][0] <= p.ends[e][1] * (1 + 1e-9) + 1e-12))
        if (apart) flagIt("CRIT", name, `"${p.text}" (${UNITS[0]}) and "${q.text}" (${UNITS[k]}) are not the same times`)
      }
    }
  }
  PAIRS.clear()
}

// A printed number in its metric's unit with the half-step its digits allow: `12.3 ms` → [12.3, 0.05],
// `12,300 µs` → [12.3, 0.0005]; a `<0.01 µs` floor → [0.000005, 0.000005] (anything from 0 to the step).
function readNum(tok: string, unit: string): [number, number] | undefined {
  const m = /([−+-]?)(<?)(\d+(?:\.(\d+))?)\s*([A-Za-zµ%/]*)/.exec(tok.replace(/,/g, ""))
  if (!m) return
  const dec = m[4]?.length ?? 0
  const scale = { KB: 1024, GB: 1024 ** 3, s: unit === "ms" ? 1000 : 1, µs: unit === "ms" ? 0.001 : 1, us: unit === "ms" ? 0.001 : 1, k: 1e3, M: 1e6, G: 1e9 }[m[5]] ?? (m[5] === "MB" && unit === "bytes" ? 1024 ** 2 : 1)
  const sign = m[1] === "−" || m[1] === "-" ? -1 : 1, x = parseFloat(m[3]) * scale
  return m[2] ? [(sign * x) / 2, x / 2] : [sign * x, 0.5 * 10 ** -dec * scale]
}
// Two printed numbers that can round from one value.
function near(a: string | undefined, b: string | undefined, unit: string): boolean {
  const x = a ? readNum(a, unit) : undefined, y = b ? readNum(b, unit) : undefined
  return !!x && !!y && Math.abs(x[0] - y[0]) <= (x[1] + y[1]) * (1 + 1e-9) + 1e-12
}
const squash = (s: string) => s.replace(/[ \t\u00a0]+/g, " ").trim()
const NUM = /—|[−+-]?<?\d[\d.,]*(?: ?(?:[GMk] instr|instr|µs|us|ms|pp|MB|KB|GB|B|score|s(?![a-z])|%|\/s))?/g

// `bench --blocks`: the text's median, interval and blocks against the HTML report's hero and the step summary's row,
// and each declared metric's line against the HTML's row (one build: nothing compared).
function checkBlocks(s: J, text: string, html: string) {
  const m = s.primary?.metric ?? ""
  const u = unitOf(m)
  const sec = text.split(/^(?=strobe bench )/m).find((x) => x.split("\n")[0].includes(`· ${s.label} ·`)) ?? ""
  if (!sec) return flagIt("MED", `text ${s.label}`, "no --blocks section in the text")
  const star = sec.split("\n").find((l) => l.startsWith(`★ ${m} `)) ?? ""
  const blocksLine = sec.split("\n").find((l) => l.trim().startsWith("blocks "))?.trim().slice(7).trim() ?? ""
  // The page opens on its first series: only that one's hero is in the HTML's text.
  if (!html || !html.includes(`${s.label} · Primary:`)) return
  const lines = html.split("\n").map(squash)
  const med = /median (\S+(?: (?:µs|us|ms|MB|KB|GB|B|pp|instr|s))?)\s+\[(.+?) … (.+?)\]/.exec(star)
  if (med) {
    const hero = lines.find((l) => / median /.test(l) && l.includes(": median "))
    const heroMed = hero ? /: median (.+)$/.exec(hero)?.[1] : undefined
    if (!near(med[1], heroMed, u)) flagIt("CRIT", `html ${s.label} ${m}`, `hero median "${heroMed}", the text's "${med[1]}"`)
    const prose = lines.find((l) => l.includes(" … ") && /interval|range/.test(l)) ?? ""
    const pr = / is (.+?) … (.+?)\. Nothing/.exec(prose)
    if (!pr || !near(med[2], pr[1], u) || !near(med[3], pr[2], u)) flagIt("CRIT", `html ${s.label} ${m}`, `interval "${pr ? `${pr[1]} … ${pr[2]}` : prose}", the text's "${med[2]} … ${med[3]}"`)
  } else if (/ \(1 block\)| \(1 of \d+ blocks\)/.test(star)) {
    const v = /★ \S+ {2}(.+?) \(/.exec(star)?.[1]
    if (!lines.some((l) => l.includes(": ") && l.endsWith("(1 block)") && near(v, /: (.+) \(1 block\)$/.exec(l)?.[1], u))) flagIt("HIGH", `html ${s.label} ${m}`, `no hero "… (1 block)" for the text's ${v}`)
  }
  const strip = lines.find((l) => l.startsWith("Blocks: "))?.slice(8) ?? ""
  const tb = blocksLine.split(" · "), hb = strip.split(" · ")
  if (tb.length !== hb.length || tb.some((x, i) => x !== hb[i] && !near(x, hb[i], u))) flagIt("CRIT", `html ${s.label} ${m}`, `blocks "${strip}", the text's "${blocksLine}"`)
  // Each metric line (declared, then the main thread's of a windowless target): the same numbers (the text pads
  // columns; the page does not).
  for (const l of sec.split("\n").filter((l) => /^ {2}[a-z][\w.{}=,-]+ {2,}/.test(l) && !l.trim().startsWith("blocks ") && !l.trim().startsWith("rec "))) {
    const [name, ...rest] = l.trim().split(/ {2,}/)
    const want = squash(rest.join(" "))
    const i = lines.indexOf(name)
    const got = i >= 0 ? lines[i + 1] ?? "" : lines.find((x) => x.startsWith(`${name} `))?.slice(name.length + 1) ?? ""
    if (!got) {
      flagIt("HIGH", `html ${s.label} ${name}`, "no row for a metric the text shows")
      continue
    }
    const a = want.match(NUM) ?? [], b = got.match(NUM) ?? []
    const words = (x: string) => x.replace(NUM, "#")
    // The page may say more after the values (`(1 block)`, `(range over 2 blocks, 50% median coverage)`).
    if (b.length < a.length || a.some((x, k) => x !== b[k] && !near(x, b[k], unitOf(name)))) flagIt("HIGH", `html ${s.label} ${name}`, `"${got}", the text's "${want}"`)
    else if (!words(got).startsWith(words(want))) flagIt("LOW", `html ${s.label} ${name}`, `the same numbers in other words: "${got}", the text's "${want}"`)
  }
}

// `report --format summary`: a check's states and tally against result.json, never a time; a leak run's verdicts; a
// --blocks run's median and interval against the text; and no other process's name anywhere in it.
function checkSummary(r: J, md: string, text: string) {
  const rowsOf = (t: string) => t.split("\n").filter((l) => l.startsWith("| ") && !l.startsWith("| ---")).map((l) => l.split(/(?<!\\)\|/).slice(1, -1).map(strip))
  const rows = rowsOf(md).slice(1)
  // Names a published summary must never carry (SPEC §1 rule 9): what the reservation moved or could not move.
  // and the busiest other processes each block saw (`blocks[].background`), where no case or target has the name.
  const own = new Set(seriesOf(r).flatMap((s: J) => [s.case, s.target, s.label]).filter(Boolean))
  const busy = seriesOf(r).flatMap((s: J) => [...(s.blocks ?? []), ...(s.pass?.blocks ?? [])].flatMap((b: J) => (Array.isArray(b.background) ? b.background.map((x: J) => x?.[0]) : [])))
  const names = [...new Set([...Object.keys(r.plan?.reserve?.moved ?? {}), ...Object.keys(r.plan?.reserve?.failed ?? {}), ...busy])].filter((n) => typeof n === "string" && n.length > 3 && ![...own].some((o: any) => String(o).includes(n)))
  for (const n of names) if (md.includes(n)) flagIt("CRIT", "summary", `names another process: ${n}`)
  const series = seriesOf(r)
  if (r.kind === "check") {
    const passed = series.filter((s: J) => (s.state ?? (s.passed ? "pass" : "fail")) === "pass" && s.passed !== false).length
    if (!md.includes(`**${passed} of ${series.length} case`)) flagIt("CRIT", "summary", `tally lacks "${passed} of ${series.length}"`)
    for (const s of series) {
      const row = rows.find((c) => c[0] === (s.case ?? "") && (c[1] === (s.target ?? "") || !s.target))
      if (!row) {
        flagIt("HIGH", `summary ${s.label}`, "no row")
        continue
      }
      const want = s.state === "pass" && s.passed === false ? "fail" : s.state ?? (s.kind === "refused" ? "refused" : s.passed ? "pass" : "fail")
      if (row[2].replace(/\*/g, "") !== want && !(want === "gap" || s.not_run)) flagIt("CRIT", `summary ${s.label}`, `status "${row[2]}", result.json ${want}`)
      // One cold run measures no time: only exact counters.
      const timed = (row[3] ?? "").match(/`([a-z][\w.]*)`/g)?.map((x) => x.slice(1, -1)).filter((x) => !/^count\.|^net\.(requests|bytes)$|^tty\.frames$|^dom\.nodes$/.test(x)) ?? []
      if (timed.length) flagIt("HIGH", `summary ${s.label}`, `a check row shows ${timed.join(", ")}, not an exact counter`)
    }
  } else if (series.length && series.every((s: J) => s.kind === "leak" || s.kind === "refused" || s.not_run)) {
    for (const s of series.filter((s: J) => s.kind === "leak")) {
      for (const role of ["base", "head"]) {
        const v = s.leak?.[role]?.js_heap?.verdict ?? s.leak?.[role]?.native_host?.verdict
        if (v && !md.includes(v.replace(/\|/g, "\\|"))) flagIt("CRIT", `summary ${s.label}`, `${role}'s verdict "${v}" not in the summary`)
      }
    }
  } else if (series.length && series.every((s: J) => s.kind === "blocks" || s.kind === "refused" || s.not_run)) {
    for (const s of series.filter((s: J) => s.kind === "blocks")) {
      const m = s.primary?.metric ?? ""
      const row = rows.find((c) => c[0] === (s.case ?? "") && c[2].replace(/`/g, "") === m)
      const sec = text.split(/^(?=strobe bench )/m).find((x) => x.split("\n")[0].includes(`· ${s.label} ·`)) ?? ""
      const star = sec.split("\n").find((l) => l.startsWith(`★ ${m} `)) ?? ""
      const t = /median (.+?)\s+\[(.+?) … (.+?)\]/.exec(star)
      const sm = row ? /^(.+?) \[(.+?) … (.+?)\]$/.exec(row[3]) : null
      // One block: its value, no interval.
      const one = /★ \S+ {2}(.+?) \(1 (?:block|of)/.exec(star)?.[1]
      if (!row) flagIt("HIGH", `summary ${s.label}`, "no row")
      else if (t && (!sm || ![1, 2, 3].every((k) => near(t[k], sm[k], unitOf(m))))) flagIt("CRIT", `summary ${s.label}`, `"${row[3]}", the text's median ${t[1]} [${t[2]} … ${t[3]}]`)
      else if (one && !near(one, /^(.+?) \(one block/.exec(row[3])?.[1], unitOf(m))) flagIt("CRIT", `summary ${s.label}`, `"${row[3]}", the text's one block ${one}`)
      else if (row && Number(row[4]) !== (s.blocks ?? []).length) flagIt("HIGH", `summary ${s.label}`, `${row[4]} blocks, result.json ${(s.blocks ?? []).length}`)
    }
  }
}

// Whether a printed number (`12.3 ms`, `−4.1%`, `1.2 MB` of bytes, `1.00 s`, `12,300 µs`) rounds from v:
// within half its last digit, in the metric's unit; `<0.01 µs` from a nonzero v under that step.
function agrees(tok: string | undefined, v: number, unit: string): boolean {
  if (tok == null) return false
  if (!Number.isFinite(v)) return /^—/.test(tok.trim())
  const m = /([−+-]?)(<?)(\d+(?:\.(\d+))?)\s*([A-Za-zµ%/]*)/.exec(tok.replace(/,/g, ""))
  if (!m) return false
  const sign = m[1] === "−" || m[1] === "-" ? -1 : 1
  const dec = m[4]?.length ?? 0
  const scale = { KB: 1024, GB: 1024 ** 3, s: unit === "ms" ? 1000 : 1, µs: unit === "ms" ? 0.001 : 1, us: unit === "ms" ? 0.001 : 1, k: 1e3, M: 1e6, G: 1e9 }[m[5]] ?? (m[5] === "MB" && unit === "bytes" ? 1024 ** 2 : 1)
  // In the printed number's own unit, so a big one (`17,105,146 µs`) keeps its last digit's precision.
  const x = sign * parseFloat(m[3]), w = v / scale
  if (m[2]) return v !== 0 && Math.sign(v) === sign && Math.abs(w) < 0.5 * Math.abs(x) * (1 + 1e-9)
  return Math.abs(x - w) <= 0.5 * 10 ** -dec * (1 + 1e-9) + 1e-9 * Math.max(1e-3, Math.abs(w))
}

// report.html's fmtVal / fmtDelta (Rust's rounding since cleanup/audit-surfaces).
function hFmt(v: number | null, u: string): string {
  if (v == null || !Number.isFinite(v)) return "—"
  return tValue(v, u)
}
function hDelta(c: J, u: string): string {
  return c.relative !== false ? relCI(c)[0] : tSigned(c.estimate, absUnit(u))
}

// `--json` is the same bytes in every unit and locale: numbers in their metric's unit, prose in µs (SPEC §7.1:
// the unit is display only).
function checkJson(id: string) {
  for (const view of ["summary", "insights"]) {
    const outs = [...UNITS.map((u) => ({ how: `--unit ${u}`, ...run(["query", id, view, "--json"], u) })), { how: "LC_ALL=C", ...run(["query", id, view, "--json"], null, { LC_ALL: "C" }) }]
    if (outs.some((o) => o.code !== outs[0].code)) flagIt("HIGH", `json ${view}`, `exit ${outs.map((o) => `${o.code} (${o.how})`).join(", ")}`)
    for (const o of outs.slice(1)) if (o.out !== outs[0].out) flagIt("CRIT", `json ${view}`, `${o.how} gives other --json bytes than ${outs[0].how}`)
  }
}

// ---------- main ----------

const ids = argv.filter((a, i) => !a.startsWith("--") && !["--strobe", "--dir", "--since", "--units", "--sample"].includes(argv[i - 1]))
const root = join(HOME, "experiments")
// What kinds of result one holds, for `--sample`: its kind, a comparison of implementations, an A/A, each
// series' kind and each verdict kind it reached, an absolute (non-ratio) primary.
function kindsOf(r: J): string[] {
  const ks = new Set<string>([`kind ${r.kind}`])
  if (r.comparison === "implementations") ks.add("implementations")
  if (r.plan?.aa) ks.add("aa")
  for (const s of seriesOf(r)) {
    ks.add(`series ${s.kind}`)
    for (const c of [s.primary, ...(s.secondaries ?? [])].filter(Boolean)) if (c.verdict_kind) ks.add(`verdict ${c.verdict_kind}`)
    if (s.primary?.relative === false) ks.add("absolute primary")
  }
  return [...ks]
}
const all = () => readdirSync(root).filter((d) => existsSync(join(root, d, "result.json")) && (!opt("since") || statSync(join(root, d, "result.json")).mtime >= new Date(opt("since")!)))
// `--sample N`: the newest N experiments of every kind (a few minutes, where `--all` takes hours).
function sample(n: number): string[] {
  const got = new Map<string, number>(), pick: string[] = []
  const newest = all().map((d) => ({ d, t: statSync(join(root, d, "result.json")).mtimeMs })).sort((a, b) => b.t - a.t)
  for (const { d } of newest) {
    let ks: string[]
    try { ks = kindsOf(JSON.parse(readFileSync(join(root, d, "result.json"), "utf8"))) } catch { continue }
    if (!ks.some((k) => (got.get(k) ?? 0) < n)) continue
    pick.push(d)
    for (const k of ks) got.set(k, (got.get(k) ?? 0) + 1)
  }
  console.error(`sample: ${pick.length} experiments covering ${[...got].sort().map(([k, c]) => `${k} ×${c}`).join(", ")}`)
  return pick
}
const list = flag("scan") || flag("all") ? all() : flag("sample") ? sample(Number(opt("sample")) || 2) : ids
if (!list.length) {
  console.error("usage: bun scripts/dev/surfaces.ts <exp_…> … [--strobe <exe>] [--no-html] [--units us,ms,s] | --sample <n> | --all|--scan [--since <date>]")
  process.exit(2)
}
if (flag("scan")) {
  // The catalog from one report, once.
  const any = list.find((d) => d) as string
  const tmp = mkdtempSync(join(tmpdir(), "strobe-surfaces-"))
  if (run(["report", "experiment", any, "--out", join(tmp, "r.html")]).code === 0) {
    const l = readFileSync(join(tmp, "r.html"), "utf8").split("\n").find((x) => x.startsWith("const STROBE = "))
    if (l) CATALOG = new Map(JSON.parse(l.slice(15).replace(/;\s*$/, "")).metrics.map((m: J) => [m.name, m]))
  }
  rmSync(tmp, { recursive: true, force: true })
}
for (const [i, id] of list.entries()) {
  current = id
  if (list.length > 1) console.error(`${i + 1}/${list.length} ${id}`)
  const file = existsSync(id) ? id : join(root, id, "result.json")
  let r: J
  try { r = JSON.parse(readFileSync(file, "utf8")) } catch (e) { flagIt("HIGH", "result.json", `unreadable: ${e}`); continue }
  // One odd result never stops the run: the checker's own failure is reported as one.
  try {
    if (!flag("scan")) {
      for (const u of UNITS) {
        UNIT = u
        current = UNITS.length > 1 ? `${id} ${u}` : id
        checkSurfaces(r, existsSync(id) ? id : r.id)
      }
      current = id
      checkPairs()
      checkJson(id)
    }
    checkResult(r)
  } catch (e) {
    flagIt("HIGH", "checker", `failed on this result: ${e}`)
  }
}
const order: Sev[] = ["CRIT", "HIGH", "MED", "LOW"]
found.sort((a, b) => order.indexOf(a.sev) - order.indexOf(b.sev) || a.exp.localeCompare(b.exp))
for (const f of found) console.log(`${f.sev.padEnd(4)} ${f.exp} ${f.where}: ${f.what}`)
const bySev = order.map((s) => `${found.filter((f) => f.sev === s).length} ${s}`).join(" · ")
console.log(`\n${list.length} experiment${list.length === 1 ? "" : "s"} · ${bySev}`)
process.exit(found.some((f) => f.sev === "CRIT" || f.sev === "HIGH") ? 1 : 0)
