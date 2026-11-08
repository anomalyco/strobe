// Dev check (not shipped): one experiment, many outputs, and they must agree (SPEC §1 rule 1). For each
// experiment it reads result.json, validates it against schema/result.v1.json, checks it is consistent
// with itself (decided ⇔ verdict, refused ⇒ no numbers, the interval holds the estimate, a better or
// worse verdict points the way the metric's direction says), then renders every surface with the
// strobe given and checks each number, sign, interval, verdict word, tally and refusal against
// result.json: the text (`query <exp> summary`), `report --format md`, `--format pr`, the HTML report
// (rendered by a headless Chromium: its text after the page's own script ran), the card's alt text and
// the badges. A number that differs between surfaces is printed as CRIT.
//
//   bun scripts/dev/surfaces.ts exp_2606af exp_a88c60 [--strobe <exe>] [--no-html]
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
const exactTie = (v: number, d: number) => rfix(v, d) !== v.toFixed(d)

// app::fmt::value / pct / signed (the text and the markdown report).
function tValue(v: number | null, unit: string): string {
  if (v == null || !Number.isFinite(v)) return "—"
  const d = Math.abs(v) >= 100 ? 0 : Math.abs(v) >= 10 ? 1 : 2
  if (Math.round(v * 10 ** d) === 0) v = 0
  switch (unit) {
    case "ms": return `${rfix(v, d)} ms`
    case "%": return `${rfix(v, d)}%`
    case "count": return Math.abs(v - Math.round(v)) < 1e-9 ? String(Math.round(v)) : rfix(v, 1)
    case "bytes": return tBytes(v)
    case "pp": return `${rfix(v, d)} pp`
    case "MB": return `${rfix(v, 0)} MB`
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
  return a >= 1024 ** 3 ? `${rfix(v / 1024 ** 3, 2)} GB` : a >= 1024 ** 2 ? `${rfix(v / 1024 ** 2, 1)} MB` : a >= 1024 ? `${rfix(v / 1024, 0)} KB` : `${rfix(v, 0)} B`
}
function tPct(r: number | null): string {
  if (r == null || !Number.isFinite(r)) return "—"
  const p = r * 100
  const body = Math.abs(p) < 10 ? rfix(Math.abs(p), 1) : rfix(Math.abs(p), 0)
  return `${p < -0.05 ? "−" : p > 0.05 ? "+" : ""}${body}%`
}
function tSigned(v: number | null, unit: string): string {
  if (v == null || !Number.isFinite(v)) return "—"
  const body = tValue(Math.abs(v), unit)
  return body === tValue(0, unit) ? body : v < 0 ? `−${body}` : v > 0 ? `+${body}` : body
}

// pr::extract::sig3 / disp / signed_disp (the PR block and the card).
function sig3(v: number): string {
  if (!Number.isFinite(v)) return "—"
  const a = Math.abs(v)
  if (a === 0) return "0"
  const d = a >= 100 ? 0 : Math.min(4, Math.max(0, 2 - Math.floor(Math.log10(a))))
  const s = rfix(v, d)
  return /^-?[0.]+$/.test(s) ? "0" : s
}
function pDisp(v: number | null, unit: string): string {
  if (v == null || !Number.isFinite(v)) return "—"
  if (unit === "ms" && Math.abs(v) >= 1000) return `${sig3(v / 1000)} s`
  if (unit === "bytes") return tBytes(v)
  if (unit === "count" || unit === "") return tValue(v, "count")
  if (unit === "value") return sig3(v)
  return ["ms", "pp", "MB"].includes(unit) ? `${sig3(v)} ${unit}` : ["%", "/s"].includes(unit) ? `${sig3(v)}${unit}` : `${sig3(v)} ${unit}`
}
function pSigned(v: number, unit: string): string {
  const body = pDisp(Math.abs(v), unit)
  return body === pDisp(0, unit) ? body : v < 0 ? `−${body}` : v > 0 ? `+${body}` : body
}

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
function pairs(s: J, m: string): [number, number][] {
  const by = new Map<number, J>()
  for (const b of blocksFor(s, m)) {
    if (!Number.isFinite(b.pair) || b.pair >= 1e6) continue
    const e = by.get(b.pair) ?? {}
    e[b.variant] = b
    by.set(b.pair, e)
  }
  return [...by.entries()].sort((a, b) => a[0] - b[0]).map(([, e]) => [e.base?.values?.[m], e.head?.values?.[m]] as [number, number]).filter(([a, b]) => Number.isFinite(a) && Number.isFinite(b))
}
function tally(s: J, c: J) {
  const ps = pairs(s, c.metric), up = higher(s, c)
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
      const n = pairs(s, c.metric).length
      if (Number.isInteger(c.n) && k !== "refused" && n && c.n !== n) flagIt("MED", where, `n ${c.n}, the blocks hold ${n} pairs with both values`)
      for (const side of ["base", "head"]) {
        const v = c[side], u = unitOf(c.metric)
        if (Number.isFinite(v) && (u === "MB" ? exactTie(v, 0) : false)) flagIt("LOW", where, `${side} ${v} MB is an exact tie: Rust prints ${rfix(v, 0)}, toFixed ${v.toFixed(0)}`)
      }
    }
  }
}

// ---------- the surfaces ----------

function run(args: string[]): { out: string; err: string; code: number } {
  const p = Bun.spawnSync([STROBE, ...args], { stdout: "pipe", stderr: "pipe" })
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

function checkSurfaces(r: J) {
  const id = r.id
  const dir = mkdtempSync(join(tmpdir(), "strobe-surfaces-"))
  try {
    // `bench --blocks` measured one build: no report of it, by design (only `query`).
    const reportable = (r.series ?? []).some((s: J) => s.kind !== "blocks")
    const html = reportable ? htmlSurface(id, dir) : undefined
    const text = run(["query", id, "summary"]).out
    const md = reportable ? run(["report", "experiment", id, "--format", "md"]).out : text
    const judgedAny = (r.series ?? []).some((s: J) => compared(s))
    const pr = judgedAny ? run(["report", "experiment", id, "--format", "pr"]).out : ""
    const card = judgedAny ? JSON.parse(run(["report", "experiment", id, "--format", "card", "--json", "--out", join(dir, "card.svg")]).out || "{}") : {}
    const badge = judgedAny ? JSON.parse(run(["report", "experiment", id, "--format", "badge", "--json", "--out", join(dir, "badge.svg")]).out || "{}") : {}
    if (r.text && r.text.trim() !== text.trim()) flagIt("LOW", "text", "the text bench printed and `query summary` now differ (a later strobe renders it otherwise)")
    for (const l of text.split("\n").filter((l) => /^[★ ] \S/.test(l))) if (!md.includes(l.trimEnd())) flagIt("HIGH", "md", `a text row is not in --format md: ${l.trim()}`)
    const sections = text.split(/^(?=strobe bench )/m)
    const prRows = pr.split("\n").filter((l) => l.startsWith("|")).map((l) => l.split(/(?<!\\)\|/).slice(1, -1).map(strip))
    const impl = r.comparison === "implementations"
    for (const s of r.series ?? []) {
      if (!compared(s)) continue
      const sec = sections.find((x) => x.split("\n")[0].includes(`· ${s.label}`)) ?? ""
      for (const { c, role } of rows(s)) {
        const where = `${s.label} ${c.metric}`
        const u = unitOf(c.metric), k = kindOf(s, c), refused = k === "refused"
        // Text: base, head, Δ, CI and the verdict as result.json has them.
        const row = sec.split("\n").find((l) => new RegExp(`^[★ ] ${c.metric.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s`).test(l))
        if (!row) flagIt("HIGH", `text ${where}`, "no row")
        else {
          // A refused metric shows no number, whatever an older result still holds.
          const delta = refused ? "—" : c.relative ? tPct(c.estimate) : tSigned(c.estimate, absUnit(u))
          const ci = refused ? "—" : c.exact ? "exact" : c.relative ? `${tPct(c.lo)} … ${tPct(c.hi)}` : `${tSigned(c.lo, absUnit(u))} … ${tSigned(c.hi, absUnit(u))}`
          for (const want of [refused ? "—" : tValue(c.base, u), refused ? "—" : tValue(c.head, u), delta, ci, c.verdict]) if (!row.includes(want)) flagIt(refused ? "CRIT" : "HIGH", `text ${where}`, `row lacks "${want}": ${row.trim()}`)
        }
        // PR: tested rows carry the same change and range; a refused one no number.
        if (pr && role !== "descriptive") {
          const ch = refused ? "—" : c.relative ? tPct(c.estimate) : pSigned(c.estimate, absUnit(u))
          const rg = refused ? "—" : c.exact ? "exact" : [c.lo, c.hi].every(Number.isFinite) ? (c.relative ? [tPct(c.lo), tPct(c.hi)] : [pSigned(c.lo, absUnit(u)), pSigned(c.hi, absUnit(u))]).filter((x, i, a) => i === 0 || x !== a[0]).join(" to ") : "—"
          // Two implementations read an increase of 2× or more as a factor (`+96% to 2.0×`).
          const sameChange = (x: string) => x === ch || (/×$/.test(x) && Math.abs(parseFloat(x) - (1 + c.estimate)) < 0.06)
          const hit = prRows.find((cells) => (cells.includes(ch) && cells.includes(rg)) || (impl && c.relative && !refused && cells.length >= 6 && sameChange(cells[3]) && /×|%/.test(cells[4])))
          if (!hit) flagIt("HIGH", `pr ${where}`, `no Metrics row with change ${ch} and range ${rg}`)
          else if (refused && hit.slice(1, 3).some((x) => /\d/.test(x))) flagIt("CRIT", `pr ${where}`, `refused, yet the row shows ${hit.join(" | ")}`)
          else if ((k === "better" || k === "worse") && !hit.some((x) => x.startsWith(String(c.verdict).replace(/ \(small\)$/, "")))) flagIt("CRIT", `pr ${where}`, `verdict ${c.verdict} not in its row: ${hit.join(" | ")}`)
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
            const nums = (s: string) => (s.match(/[−+-]?\d+(\.\d+)?/g) ?? []).map((x) => parseFloat(x.replace("−", "-")))
            const same = j >= 0 && nums(lines[j].slice(head.length)).slice(0, 3).every((x, n) => Math.abs(x - nums(want.slice(head.length))[n]) <= 0.01 * Math.max(1, Math.abs(x)))
            if (j < 0) flagIt(refused ? "CRIT" : "HIGH", `html ${where}`, `no forest row "${want}"`)
            else flagIt(same ? "LOW" : refused ? "CRIT" : "HIGH", `html ${where}`, `forest row "${lines[j]}", the text's numbers read "${want}"`)
            i = j
          }
          if (i >= 0) {
            const next = lines[i + 1] ?? ""
            const want = refused ? /Refused/ : role === "descriptive" ? /Context$/ : k === "better" || k === "worse" ? new RegExp(`Confirmed ${(c.hi < 0) !== higher(s, c) ? "better" : "worse"}$|False positive$`) : k === "same" ? /Same \(/ : k === "no_worse" ? /No worse/ : k === "untested" ? /Not tested|Inconclusive|Unconfirmed/ : /Inconclusive|Unconfirmed/
            if (!want.test(next)) flagIt("CRIT", `html ${where}`, `outcome "${next}" for verdict_kind ${k}`)
          }
        }
      }
      // The primary's tally and direction on the proof line, the HTML hero, the card and the badge.
      const p = s.primary, k = p?.metric ? kindOf(s, p) : undefined
      if (k === "better" || k === "worse") {
        const t = tally(s, p), word = k === "better" ? "better" : "worse", count = k === "better" ? t.better : t.worse
        if (pr && !impl && !pr.includes(`${word} in ${count} of ${t.n} pairs`)) flagIt("CRIT", `pr ${s.label}`, `proof line lacks "${word} in ${count} of ${t.n} pairs"`)
        if (card.alt && !impl && (r.series ?? []).filter(compared).length === 1 && !card.alt.includes(`${word} in ${count} of ${t.n} pairs`)) flagIt("CRIT", `card ${s.label}`, `alt "${card.alt}" lacks "${word} in ${count} of ${t.n} pairs"`)
        const hero = !html?.text.includes(`${s.label} · Primary:`) ? null : html.text.match(/(outperformed|trailed) .*? in (?:all )?(\d+) of (\d+)/)
        if (hero && (hero[1] === "outperformed") !== (k === "better")) flagIt("CRIT", `html ${s.label}`, `hero says ${hero[1]} for a ${k} verdict`)
        if (hero && +hero[2] !== count) flagIt("CRIT", `html ${s.label}`, `hero counts ${hero[2]} of ${hero[3]} pairs, the blocks ${count} of ${t.n}`)
      }
      const b = (badge.series ?? []).find((x: J) => x.series === s.label || x.series?.startsWith(`${s.label} `))
      // A journey decided on its own but not after Holm is grey by design.
      if (b && !(impl && p?.holm_decided === false && (k === "better" || k === "worse"))) {
        const colour = /-([0-9a-f]{6})\?/.exec(b.url ?? "")?.[1]
        const want = k === "better" ? "1a7f37" : k === "worse" && !r.args?.flags?.aa ? "cf222e" : "6e7781"
        if (colour && colour !== want) flagIt("CRIT", `badge ${s.label}`, `colour ${colour} (${b.value}) for verdict_kind ${k}`)
        if (k === "refused" && /\d/.test(b.value)) flagIt("CRIT", `badge ${s.label}`, `refused, yet the badge reads ${b.value}`)
        if ((k === "better" || k === "worse") && /^[+−-]/.test(b.value) && (b.value.startsWith("+") !== p.estimate > 0)) flagIt("CRIT", `badge ${s.label}`, `sign of ${b.value} against estimate ${p.estimate}`)
      }
      // Precision: the same median read 25% apart on two surfaces (text `1 MB`, PR `0.785 MB`).
      for (const { c } of rows(s)) {
        const u = unitOf(c.metric)
        for (const side of ["base", "head"]) {
          // Both in the metric's unit (the PR block writes a second or longer in seconds).
          const read = (s: string) => parseFloat(s) * (/\d s$/.test(s) && u === "ms" ? 1000 : 1)
          const v = c[side], a = read(tValue(v, u)), b2 = read(pDisp(v, u))
          if (Number.isFinite(v) && v !== 0 && Number.isFinite(a) && Number.isFinite(b2) && Math.abs(a - b2) > 0.25 * Math.abs(v)) flagIt("MED", `text/pr ${s.label} ${c.metric}`, `${side} prints ${tValue(v, u)} in the text and ${pDisp(v, u)} in the PR block`)
        }
      }
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

// report.html's fmtVal / fmtDelta (Rust's rounding since cleanup/audit-surfaces).
function hFmt(v: number | null, u: string): string {
  if (v == null || !Number.isFinite(v)) return "—"
  return tValue(v, u)
}
function hDelta(c: J, u: string): string {
  return c.relative !== false ? tPct(c.estimate) : tSigned(c.estimate, absUnit(u))
}

// ---------- main ----------

const ids = argv.filter((a, i) => !a.startsWith("--") && !["--strobe", "--dir", "--since"].includes(argv[i - 1]))
const root = join(HOME, "experiments")
const list = flag("scan")
  ? readdirSync(root).filter((d) => existsSync(join(root, d, "result.json")) && (!opt("since") || statSync(join(root, d, "result.json")).mtime >= new Date(opt("since")!)))
  : ids
if (!list.length) {
  console.error("usage: bun scripts/dev/surfaces.ts <exp_…> … [--strobe <exe>] [--no-html] | --scan [--since <date>]")
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
for (const id of list) {
  current = id
  const file = existsSync(id) ? id : join(root, id, "result.json")
  let r: J
  try { r = JSON.parse(readFileSync(file, "utf8")) } catch (e) { flagIt("HIGH", "result.json", `unreadable: ${e}`); continue }
  if (!flag("scan")) checkSurfaces(r)
  checkResult(r)
}
const order: Sev[] = ["CRIT", "HIGH", "MED", "LOW"]
found.sort((a, b) => order.indexOf(a.sev) - order.indexOf(b.sev) || a.exp.localeCompare(b.exp))
for (const f of found) console.log(`${f.sev.padEnd(4)} ${f.exp} ${f.where}: ${f.what}`)
const bySev = order.map((s) => `${found.filter((f) => f.sev === s).length} ${s}`).join(" · ")
console.log(`\n${list.length} experiment${list.length === 1 ? "" : "s"} · ${bySev}`)
process.exit(found.some((f) => f.sev === "CRIT" || f.sev === "HIGH") ? 1 : 0)
