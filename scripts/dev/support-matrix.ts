// The verified/preview matrix (`SUPPORT_MATRIX` in src/support.rs, SPEC 21.1), printed for one host or checked.
//
//   bun scripts/dev/support-matrix.ts [--host windows-x64|windows-arm64|macos-arm64|macos-x64|linux-x64|linux-arm64] [--json]
//   bun scripts/dev/support-matrix.ts --check [--no-rust] [--rot]
//
// Printing: every runtime × capability on the host, its label there and the evidence or what is missing, and
// which combinations strobe refuses (on that OS, or for that kind of target). It reads the source, so it needs no
// build. Off the OS's primary architecture every cell verified on that OS reads preview. Without --host: this
// machine's.
//
// --check holds the matrix to its invariants and exits 1 on any failure (strobe keeps no test suite, SPEC 1 rule 5:
// this is a dev check run by hand). From the source alone: one row per runtime and capability, every cell's text a
// table cell (no `|`), every evidence record complete (8 fields; date, version and machine one field each; its
// fixture a file under scripts/dev/), every name a round trip of its variant, the schema's enums the code's, every
// metric's capability a capability, the parser on wrapped and escaped Rust. The no-rot rule (`WATCH`, below): no
// verified cell's measuring code changed since its evidence or its review (git; a shallow clone fetches what it
// lacks); --rot prints each cell's spans, held or changed. Then (without --no-rust) a small program
// against the strobe library (cargo, offline, in the temp folder; CARGO_TARGET_DIR is reused when set) prints what
// the real `support` functions answer: every cell on every host, the lookups, refusals, modifiers and phrases, each
// held to what this file expects. `parseSupport` and `onHost` are what doc-check can import.
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync, copyFileSync } from "node:fs"
import { join, resolve as resolvePath } from "node:path"
import { tmpdir } from "node:os"

export type Cell = { kind: "verified"; evidence: Record<string, string> } | { kind: "preview"; why: string } | { kind: "refused"; why: string }
export type Row = { runtime: string; capability: string; windows: Cell; macos: Cell; linux: Cell }
export type Modifier = Record<"windows" | "macos" | "linux", Cell>
export type Support = {
  runtimes: string[]
  capabilities: string[]
  hosts: string[]
  core: string[]
  rows: Row[]
  osInput: Modifier
  outsidePage: Modifier
  names: Map<string, string>
  evidenceProblems: string[]
}

export const EVIDENCE_KEYS = ["date", "version", "machine", "fixture", "known", "aa", "planted", "boundary"]
// A later commit at which a person read every change to the cell's watched code since `version` and found that
// none can move its numbers, and the one sentence why (the no-rot rule, `rotProblems`).
export const REVIEW_KEYS = ["reviewed_at", "reviewed_why"]
// What `version` and `reviewed_at` hold: a commit id or a release tag.
const REVISION = /^[0-9a-f]{7,40}$|^v\d+\.\d+\.\d+$/

// A Rust string literal's value: its escapes, and a `\` at a line's end that drops the newline and the next
// line's leading whitespace.
function unescape(body: string): string {
  return body.replace(/\\(\r?\n\s*|u\{([0-9a-fA-F]+)\}|x([0-9a-fA-F]{2})|.)/g, (_, e: string, u?: string, x?: string) => {
    if (u) return String.fromCodePoint(parseInt(u, 16))
    if (x) return String.fromCharCode(parseInt(x, 16))
    if (e.startsWith("\n") || e.startsWith("\r")) return ""
    return ({ n: "\n", t: "\t", r: "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'" } as Record<string, string>)[e] ?? e
  })
}

// Every string literal out of the text, each replaced by `"@N@"`: what is left has no brace, quote or `};` of a
// string's, and comments are gone. Raw strings (r"…", r#"…"#) count as strings too.
function lift(text: string): { code: string; strings: string[] } {
  const strings: string[] = []
  let code = ""
  let i = 0
  const n = text.length
  while (i < n) {
    const c = text[i]
    if (c === "/" && text[i + 1] === "/") {
      const end = text.indexOf("\n", i)
      i = end < 0 ? n : end
    } else if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2)
      i = end < 0 ? n : end + 2
    } else if (c === "r" && /^r#*"/.test(text.slice(i, i + 10)) && !/\w/.test(text[i - 1] ?? "")) {
      const hashes = text.slice(i + 1).match(/^#*/)![0]
      const start = i + 2 + hashes.length
      const end = text.indexOf(`"${hashes}`, start)
      if (end < 0) throw new Error("unterminated raw string")
      code += `"@${strings.length}@"`
      strings.push(text.slice(start, end))
      i = end + 1 + hashes.length
    } else if (c === '"') {
      let j = i + 1
      while (j < n && text[j] !== '"') j += text[j] === "\\" ? 2 : 1
      if (j >= n) throw new Error("unterminated string")
      code += `"@${strings.length}@"`
      strings.push(unescape(text.slice(i + 1, j)))
      i = j + 1
    } else if (c === "'" && /^'(\\.|[^\\'])'/.test(text.slice(i, i + 4))) {
      const lit = text.slice(i, i + (text[i + 1] === "\\" ? 4 : 3))
      code += lit
      i += lit.length
    } else {
      code += c
      i++
    }
  }
  return { code, strings }
}

// `ChromiumPage` → `chromium_page`, as serde's snake_case names it.
const snake = (s: string) => s.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase()
const STR = `"@(\\d+)@"`
const CELL = `((?:Cell::)?(?:Verified\\(\\s*&\\s*\\w+\\s*\\)|(?:Preview|Refused)\\(\\s*(?:\\w+|${STR})\\s*\\)))`

export function parseSupport(text: string): Support {
  const { code, strings } = lift(text.replace(/\r\n/g, "\n"))
  const str = (i: string) => strings[Number(i)]
  const consts = new Map<string, string>()
  for (const m of code.matchAll(new RegExp(`const (\\w+)\\s*:\\s*&(?:'static )?str\\s*=\\s*${STR}\\s*;`, "g"))) consts.set(m[1], str(m[2]))
  const evidence = new Map<string, Record<string, string>>()
  const evidenceProblems: string[] = []
  for (const m of code.matchAll(/const (\w+)\s*:\s*Evidence\s*=\s*Evidence\s*\{([^{}]*)\}\s*;/g)) {
    const fields: Record<string, string> = {}
    const written: string[] = []
    // A field is a literal, Some(a literal or a &str const) or None.
    for (const f of m[2].matchAll(new RegExp(`(\\w+)\\s*:\\s*(?:${STR}|Some\\(\\s*(?:${STR}|([A-Z_][A-Z0-9_]*))\\s*,?\\s*\\)|(None))`, "g"))) {
      written.push(f[1])
      if (f[5] !== undefined) continue
      const v = f[4] !== undefined ? consts.get(f[4]) : str(f[2] ?? f[3])
      if (v === undefined) evidenceProblems.push(`Evidence ${m[1]}: ${f[1]} names no &str const ${f[4]}`)
      else fields[f[1]] = v
    }
    for (const k of EVIDENCE_KEYS) if (!fields[k]?.trim()) evidenceProblems.push(`Evidence ${m[1]}: no ${k}`)
    for (const k of written) if (!EVIDENCE_KEYS.includes(k) && !REVIEW_KEYS.includes(k)) evidenceProblems.push(`Evidence ${m[1]}: unknown field ${k}`)
    for (const k of REVIEW_KEYS) if (!written.includes(k)) evidenceProblems.push(`Evidence ${m[1]}: no ${k} (None until a person reviews a later change)`)
    if (("reviewed_at" in fields) !== ("reviewed_why" in fields)) evidenceProblems.push(`Evidence ${m[1]}: reviewed_at and reviewed_why go together (a review says why the numbers cannot have moved)`)
    for (const k of ["version", "reviewed_at"])
      if (fields[k] !== undefined && !REVISION.test(fields[k])) evidenceProblems.push(`Evidence ${m[1]}: ${k} ${fields[k]} is no commit id (7 to 40 hex digits) or release tag (vX.Y.Z)`)
    evidence.set(m[1], fields)
  }
  const cell = (src: string, where: string): Cell => {
    const m = src.match(/^(?:Cell::)?(Verified|Preview|Refused)\(\s*(.*?)\s*\)$/s)
    if (!m) throw new Error(`${where}: not a cell: ${src}`)
    if (m[1] === "Verified") {
      const name = m[2].replace(/^&\s*/, "")
      const ev = evidence.get(name)
      if (!ev) throw new Error(`${where}: no Evidence ${name}`)
      return { kind: "verified", evidence: ev }
    }
    const lit = m[2].match(/^"@(\d+)@"$/)
    const why = lit ? str(lit[1]) : consts.get(m[2])
    if (why === undefined) throw new Error(`${where}: no const ${m[2]}`)
    return { kind: m[1] === "Preview" ? "preview" : "refused", why }
  }
  const list = (ty: string, name: string) => {
    const body = code.match(new RegExp(`pub const ${name}\\s*:\\s*\\[${ty}\\s*;\\s*\\d+\\]\\s*=\\s*\\[([\\s\\S]*?)\\]\\s*;`))?.[1]
    if (body === undefined) throw new Error(`src/support.rs: no ${name} of ${ty}`)
    return [...body.matchAll(new RegExp(`${ty}::(\\w+)`, "g"))].map((m) => snake(m[1]))
  }
  const matrix = code.match(/pub const SUPPORT_MATRIX\s*:\s*&\[Row\]\s*=\s*&\[([\s\S]*?)\n\]\s*;/)?.[1]
  if (!matrix) throw new Error("src/support.rs: no SUPPORT_MATRIX")
  const rowRe = new RegExp(
    `Row\\s*\\{\\s*runtime:\\s*Runtime::(\\w+),\\s*capability:\\s*Capability::(\\w+),\\s*windows:\\s*${CELL},\\s*macos:\\s*${CELL},\\s*linux:\\s*${CELL},?\\s*\\}`,
    "g",
  )
  const rows: Row[] = []
  for (const m of matrix.matchAll(rowRe)) {
    const [runtime, capability] = [snake(m[1]), snake(m[2])]
    const where = `${runtime} · ${capability}`
    rows.push({ runtime, capability, windows: cell(m[3], where), macos: cell(m[5], where), linux: cell(m[7], where) })
  }
  const written = (matrix.match(/\bRow\s*\{/g) ?? []).length
  if (written !== rows.length) throw new Error(`src/support.rs: ${written} rows written, ${rows.length} read`)
  const modifier = (name: string): Modifier => {
    const m = code.match(new RegExp(`pub const ${name}\\s*:\\s*Modifier\\s*=\\s*Modifier\\s*\\{\\s*windows:\\s*${CELL},\\s*macos:\\s*${CELL},\\s*linux:\\s*${CELL},?\\s*\\}\\s*;`))
    if (!m) throw new Error(`src/support.rs: no ${name}`)
    return { windows: cell(m[1], name), macos: cell(m[3], name), linux: cell(m[5], name) }
  }
  // Each enum's `name()` arms (the first `Type::Variant => "…"` of each variant: name comes before label).
  const names = new Map<string, string>()
  for (const m of code.matchAll(new RegExp(`\\b(Runtime|Capability|Host)::(\\w+)\\s*=>\\s*${STR}`, "g"))) {
    const key = `${m[1]}::${m[2]}`
    if (!names.has(key)) names.set(key, str(m[3]))
  }
  return {
    runtimes: list("Runtime", "ALL"),
    capabilities: list("Capability", "ALL"),
    hosts: list("Host", "ALL").map((h) => h.replace(/_/g, "-")),
    core: list("Capability", "CORE"),
    rows,
    osInput: modifier("OS_INPUT"),
    outsidePage: modifier("OUTSIDE_PAGE"),
    names,
    evidenceProblems,
  }
}

const PRIMARY = new Set(["windows-x64", "macos-arm64", "linux-x64"])
const osOf = (host: string) => host.split("-")[0] as "windows" | "macos" | "linux"
const primaryOf = (host: string) => ({ windows: "windows-x64", macos: "macos-arm64", linux: "linux-x64" })[osOf(host)]

export const evidenceLine = (e: Record<string, string>) => `verified (${e.date}, ${e.version}, ${e.machine}, \`${e.fixture}\`, ${e.known})`
export const EVIDENCE_LINE = /^verified \(\d{4}-\d{2}-\d{2}, [^,]+, [^,]+, `(scripts\/dev\/[^`]+)`, .+\)$/

// A cell on a host, as `support::support` resolves it: verified only on the OS's primary architecture.
export function onHost(c: Cell, host: string): { label: "verified" | "preview" | "refused"; detail: string } {
  if (c.kind === "refused") return { label: "refused", detail: c.why }
  if (c.kind === "preview") return { label: "preview", detail: c.why }
  const e = c.evidence
  if (!PRIMARY.has(host)) return { label: "preview", detail: `verified on ${primaryOf(host)} only: ${evidenceLine(e)}` }
  return { label: "verified", detail: `${evidenceLine(e)}; A/A ${e.aa}; planted ${e.planted}; boundary ${e.boundary}` }
}

// A target's README cell (`support::target_support`): the least label of its core capabilities strobe runs on the
// host; null when it runs none there.
export function rollup(s: Support, runtime: string, host: string): "verified" | "preview" | null {
  const labels = s.rows.filter((r) => r.runtime === runtime && s.core.includes(r.capability)).map((r) => onHost(r[osOf(host)], host).label).filter((l) => l !== "refused")
  if (!labels.length) return null
  return labels.every((l) => l === "verified") ? "verified" : "preview"
}

const root = resolvePath(import.meta.dir, "..", "..")
const source = () => readFileSync(join(root, "src", "support.rs"), "utf8")

// ---------- --check ----------

function staticChecks(s: Support, fail: (m: string) => void) {
  const seen = new Set<string>()
  for (const r of s.rows) {
    const key = `${r.runtime} · ${r.capability}`
    if (seen.has(key)) fail(`row ${key} twice`)
    seen.add(key)
    if (!s.runtimes.includes(r.runtime)) fail(`row ${key}: ${r.runtime} is no Runtime::ALL`)
    if (!s.capabilities.includes(r.capability)) fail(`row ${key}: ${r.capability} is no Capability::ALL`)
    for (const os of ["windows", "macos", "linux"] as const) checkCell(r[os], `${key} on ${os}`, fail)
  }
  for (const [name, m] of [["OS_INPUT", s.osInput], ["OUTSIDE_PAGE", s.outsidePage]] as const) for (const os of ["windows", "macos", "linux"] as const) checkCell(m[os], `${name} on ${os}`, fail)
  for (const p of s.evidenceProblems) fail(p)
  for (const rt of s.runtimes) if (!s.rows.some((r) => r.runtime === rt && r.capability === "wall_and_counts")) fail(`${rt} has no wall_and_counts row`)
  for (const c of s.capabilities) if (!s.rows.some((r) => r.capability === c)) fail(`capability ${c} has no row`)
  for (const c of s.core) if (!s.capabilities.includes(c)) fail(`CORE names ${c}, no capability`)
  // Names: each variant's name() is its serde name (snake_case; a host's kebab-case), every variant has one.
  for (const [ty, all, conv] of [["Runtime", s.runtimes, (x: string) => x], ["Capability", s.capabilities, (x: string) => x], ["Host", s.hosts, (x: string) => x.replace(/-/g, "_")]] as const) {
    for (const v of all) {
      const variant = conv(v).replace(/(^|_)([a-z0-9])/g, (_, __, c: string) => c.toUpperCase())
      const named = s.names.get(`${ty}::${variant}`)
      if (named !== v) fail(`${ty}::${variant}.name() is ${named}, serde names it ${v}`)
    }
  }
  // The schema's enums are the code's, in order.
  const schema = JSON.parse(readFileSync(join(root, "schema", "result.v1.json"), "utf8"))
  for (const [def, all] of [["runtime", s.runtimes], ["capability", s.capabilities], ["host", s.hosts]] as const) {
    const e: string[] = schema.definitions?.[def]?.enum ?? []
    if (e.join(",") !== all.join(",")) fail(`schema definitions.${def} is [${e.join(", ")}], the code's [${all.join(", ")}]`)
  }
  const causes = schema.definitions?.pathSupport?.properties?.preview_by?.items?.enum ?? []
  const rustCauses = [...(lift(source()).code.match(/pub enum Cause\s*\{([\s\S]*?)\}/)?.[1] ?? "").matchAll(/^\s*(\w+),/gm)].map((m) => snake(m[1]))
  if (causes.join(",") !== rustCauses.join(",")) fail(`schema pathSupport.preview_by is [${causes.join(", ")}], Cause is [${rustCauses.join(", ")}]`)
  // Every capability a metric names is one.
  for (const m of readFileSync(join(root, "src", "model.rs"), "utf8").matchAll(/capability: Capability::(\w+)/g)) {
    if (!s.capabilities.includes(snake(m[1]))) fail(`model.rs names capability ${snake(m[1])}, which Capability::ALL lacks`)
  }
}

function checkCell(c: Cell, at: string, fail: (m: string) => void) {
  const texts = c.kind === "verified" ? Object.values(c.evidence) : [c.why]
  for (const t of texts) {
    if (!t.trim()) fail(`${at}: empty text`)
    if (t.includes("|") || t.includes("\n")) fail(`${at}: a table cell holds no | or line break: ${t}`)
  }
  if (c.kind !== "verified") return
  const e = c.evidence
  if (!/^\d{4}-\d{2}-\d{2}$/.test(e.date ?? "")) fail(`${at}: date ${e.date} is no YYYY-MM-DD`)
  for (const k of ["version", "machine", "fixture"]) if ((e[k] ?? "").includes(",")) fail(`${at}: ${k} holds a comma: ${e[k]}`)
  const f = e.fixture ?? ""
  if (!f.startsWith("scripts/dev/") || f.split(/[\\/]/).includes("..") || f.includes("\\")) fail(`${at}: fixture ${f} is no path under scripts/dev/`)
  else if (!statSync(join(root, f), { throwIfNoEntry: false })?.isFile()) fail(`${at}: fixture ${f} is no file`)
  if (!EVIDENCE_LINE.test(evidenceLine(e))) fail(`${at}: ${evidenceLine(e)} is no evidence line`)
}

// The parser on what rustfmt and a writer may produce: wrapped constants, braces, `};` and quotes in strings.
function parserChecks(fail: (m: string) => void) {
  const text = `
pub const ALL: [Runtime; 1] = [Runtime::Cli];
impl Runtime { fn name(self) -> &'static str { match self { Runtime::Cli => "cli" } } }
pub const ALL: [Capability; 1] = [Capability::WallAndCounts];
impl Capability { fn name(self) -> &'static str { match self { Capability::WallAndCounts => "wall_and_counts" } } }
pub const ALL: [Host; 1] = [Host::WindowsX64];
pub const CORE: [Capability; 1] = [Capability::WallAndCounts];
const WRAPPED_REASON: &str =
    "a reason with } and }; and \\"quotes\\" and it\\'s fine";
const MAC_CLI_WALL_AND_COUNTS_WITH_A_LONG_NAME: Evidence =
    Evidence {
        date: "2026-11-12", // a comment with "quotes" and };
        version: "964e825",
        machine: "Apple silicon laptop (macOS 27.0.1)",
        fixture: "scripts/dev/known/cli.bench.ts",
        known: "run.duration Δ 150.17 ms vs 150 ms { planted }; metric lines exact",
        aa: "8 pairs, +0.007% [−0.06 … +0.06]",
        planted: "+97.10% \\
            [+96.95 … +97.33]",
        boundary: "cmd: { exit 1 }; refused RunIncomplete",
        reviewed_at: Some(
            "0123abc",
        ),
        reviewed_why: Some("a rename; { braces } and \\"quotes\\""),
    };
pub const SUPPORT_MATRIX: &[Row] = &[
    Row {
        runtime: Runtime::Cli,
        capability: Capability::WallAndCounts,
        windows: Cell::Verified(&MAC_CLI_WALL_AND_COUNTS_WITH_A_LONG_NAME),
        macos: Preview(WRAPPED_REASON),
        linux: Refused("inline, with a } brace"),
    },
];
pub const OS_INPUT: Modifier = Modifier { windows: Preview("a"), macos: Preview("b"), linux: Preview("c") };
pub const OUTSIDE_PAGE: Modifier = Modifier { windows: Preview("a"), macos: Preview("b"), linux: Preview("c") };
`
  let s: Support
  try {
    s = parseSupport(text)
  } catch (e) {
    return fail(`parser: ${(e as Error).message}`)
  }
  const r = s.rows[0]
  const want = (what: string, got: unknown, exp: unknown) => got !== exp && fail(`parser: ${what} read ${JSON.stringify(got)}, not ${JSON.stringify(exp)}`)
  want("rows", s.rows.length, 1)
  want("verified", r?.windows.kind, "verified")
  if (r?.windows.kind === "verified") {
    want("boundary", r.windows.evidence.boundary, "cmd: { exit 1 }; refused RunIncomplete")
    want("planted", r.windows.evidence.planted, "+97.10% [+96.95 … +97.33]")
    want("date", r.windows.evidence.date, "2026-11-12")
    want("reviewed_at", r.windows.evidence.reviewed_at, "0123abc")
    want("reviewed_why", r.windows.evidence.reviewed_why, `a rename; { braces } and "quotes"`)
  }
  const unreviewed = parseSupport(text.replace(/reviewed_at: Some\([^)]*\)/, "reviewed_at: None").replace(/reviewed_why: Some\([^)]*\)/, "reviewed_why: None"))
  const ur = unreviewed.rows[0]?.windows
  want("unreviewed problems", unreviewed.evidenceProblems.length, 0)
  want("unreviewed reviewed_at", ur?.kind === "verified" ? ur.evidence.reviewed_at : "?", undefined)
  const half = parseSupport(text.replace(/reviewed_why: Some\([^)]*\)/, "reviewed_why: None"))
  want("a review without its why", half.evidenceProblems.length, 1)
  want("wrapped reason", r?.macos.kind === "preview" ? r.macos.why : null, `a reason with } and }; and "quotes" and it's fine`)
  want("inline reason", r?.linux.kind === "refused" ? r.linux.why : null, "inline, with a } brace")
  want("evidence problems", s.evidenceProblems.length, 0)
  const missing = parseSupport(text.replace(/\s+aa: "[^"]*",/, ""))
  want("a record without aa", missing.evidenceProblems.join(";"), "Evidence MAC_CLI_WALL_AND_COUNTS_WITH_A_LONG_NAME: no aa")
}

// What the real library answers, through a program cargo builds against it (offline).
type Case = [key: string, expr: string, want: string]
const p = (expr: string) => `p(${expr})`
const rt = (expr: string) => `${expr}.map(Runtime::name).unwrap_or("none").to_string()`
const T = "Through::default()"
const th = (o: Partial<Record<"base_pixels" | "head_pixels" | "os_input" | "outside_page", boolean>>) => `Through { ${Object.entries(o).map(([k, v]) => `${k}: ${v}`).join(", ")}, ..Through::default() }`
const rm = (sides: string, metric: string, host: string, through = T) => `resolve_metric(${sides}, ${JSON.stringify(metric)}, Host::${host}, ${through})`
const one = (r: string) => `Sides::One(Runtime::${r})`
const two = (b: string, h: string) => `Sides::Two { base: Runtime::${b}, head: Runtime::${h} }`
const target = (j: object, windowless: boolean, phone: boolean) => rt(`Runtime::of_target(&json!(${JSON.stringify(j)}), ${windowless}, ${phone})`)
const record = (t: string, engine: string | null, phone: boolean, native: boolean, webview: boolean, windowless: boolean) =>
  rt(`Runtime::of_record(${JSON.stringify(t)}, ${engine ? `Some(WebEngine::${engine})` : "None"}, ${phone}, ${native}, ${webview}, ${windowless})`)
const cap = (m: string) => `Capability::of(${JSON.stringify(m)}).map(Capability::name).unwrap_or("none").to_string()`
const pix = (j: object) => `launch_uses_pixels(&json!(${JSON.stringify(j)})).to_string()`
const phrase = (expr: string) => `${expr}.map(|p| p.phrase()).unwrap_or_else(|| "none".into())`
const built = (fields: string) => `PathSupport { ${fields}, os_input: false, pixels: false, outside_page: false, base_runtime: None, head_runtime: None }.phrase()`
const PRE = "preview|"
const CASES: Case[] = [
  // Finding 1: a terminal program's counters and idle wakes are measured.
  ["tty cpu.instructions linux", p(rm(one("Tty"), "cpu.instructions", "LinuxX64")), `${PRE}capability`],
  ["tty energy.cpu macos", p(rm(one("Tty"), "energy.cpu", "MacosArm64")), `${PRE}capability`],
  ["tty cpu.idle_wakes macos", p(rm(one("Tty"), "cpu.idle_wakes", "MacosArm64")), `${PRE}capability`],
  ["tty cpu.instructions windows (refused)", p(rm(one("Tty"), "cpu.instructions", "WindowsX64")), "none"],
  // Finding 2: pixels per side, a phone's pixels, a page from outside, OS input.
  ["windowless vs native, head read pixels", p(`resolve(${two("NativeWindowless", "Native")}, Capability::ProcessCpu, Host::WindowsX64, ${th({ head_pixels: true })})`), `${PRE}capability,pixels`],
  ["tty vs native, head read pixels", p(`resolve(${two("Tty", "Native")}, Capability::ProcessCpu, Host::WindowsX64, ${th({ head_pixels: true })})`), `${PRE}capability,pixels`],
  ["windowless side cannot read pixels", p(`resolve(${two("NativeWindowless", "Native")}, Capability::ProcessCpu, Host::WindowsX64, ${th({ base_pixels: true })})`), "none"],
  ["phone page with pixels", p(rm(one("AndroidChrome"), "main.work", "MacosArm64", th({ base_pixels: true }))), `${PRE}capability,pixels`],
  ["page from outside, frames", p(rm(one("ChromiumPage"), "frames.dropped.pct", "MacosArm64", th({ outside_page: true }))), `${PRE}outside_page`],
  ["page from outside, page engine untouched", p(rm(one("ChromiumPage"), "main.work", "MacosArm64", th({ outside_page: true }))), `${PRE}capability`],
  ["outside applies to pages only", p(rm(one("NativeWindowless"), "cpu.work", "MacosArm64", th({ outside_page: true }))), "verified|"],
  ["os input", p(rm(one("Native"), "interaction.p95", "WindowsX64", th({ os_input: true }))), `${PRE}capability,os_input`],
  // Finding 3: a phone measures nothing of this computer's processes; of_target is strict.
  ["phone process memory row", `support(Runtime::AndroidChrome, Capability::ProcessMemory, Host::MacosArm64).map(Support::name).unwrap_or("none").to_string()`, "none"],
  ["phone memory.app.private", p(rm(one("AndroidChrome"), "memory.app.private", "MacosArm64")), "none"],
  ["phone cpu.work", p(rm(one("AndroidChrome"), "cpu.work", "MacosArm64")), "none"],
  ["phone cpu.fixture (a process of this computer)", `${rm(one("AndroidChrome"), "cpu.fixture", "MacosArm64")}.map(|p| p.runtime.name()).unwrap_or("none").to_string()`, "native_windowless"],
  ["target firefox phone", target({ kind: "web", engine: "firefox", emulate: "mobile" }, false, true), "none"],
  ["target servo", target({ kind: "web", engine: "servo" }, false, false), "none"],
  ["target phone page", target({ kind: "web", emulate: "mobile" }, false, true), "android_chrome"],
  ["target phone page, chromium named", target({ kind: "url", engine: "chromium", emulate: "mobile" }, false, true), "android_chrome"],
  ["target desktop page under --mobile", target({ kind: "web" }, false, true), "none"],
  ["target touch page under --mobile", target({ kind: "web", emulate: "touch" }, false, true), "none"],
  ["target cli under --mobile", target({ kind: "cli" }, false, true), "none"],
  ["target emulated phone, no --mobile", target({ kind: "web", emulate: "mobile" }, false, false), "chromium_page"],
  ["target webkit", target({ kind: "url", engine: "webkit" }, false, false), "webkit_page"],
  ["target windowless native", target({ kind: "native" }, true, false), "native_windowless"],
  ["target native", target({ kind: "native" }, false, false), "native"],
  ["target command", target({ kind: "command" }, false, false), "chromium_app"],
  ["target unknown kind", target({ kind: "nope" }, false, false), "none"],
  // Finding 6: names and metrics only one kind of target reads.
  ["cli memory.app.private", p(rm(one("Cli"), "memory.app.private", "MacosArm64")), "none"],
  ["script main.work", p(rm(one("Script"), "main.work", "MacosArm64")), "none"],
  ["script js.heap.retained", p(rm(one("Script"), "js.heap.retained", "LinuxX64")), `${PRE}capability`],
  ["page js.heap.retained", p(rm(one("ChromiumPage"), "js.heap.retained", "LinuxX64")), "none"],
  ["cli memory.peak", p(rm(one("Cli"), "memory.peak", "WindowsX64")), "verified|"],
  ["native memory.peak", p(rm(one("NativeWindowless"), "memory.peak", "WindowsX64")), "none"],
  ["cli metric.loop_ms", p(rm(one("Cli"), "metric.loop_ms", "LinuxX64")), `${PRE}capability`],
  ["script metric.x", p(rm(one("Script"), "metric.x", "LinuxX64")), "none"],
  ["tty app.x", p(rm(one("Tty"), "app.x", "LinuxX64")), `${PRE}capability`],
  ["native app.x", p(rm(one("Native"), "app.x", "LinuxX64")), "none"],
  ["tty cpu.pty_host windows", p(rm(one("Tty"), "cpu.pty_host", "WindowsX64")), `${PRE}capability`],
  ["tty cpu.pty_host linux", p(rm(one("Tty"), "cpu.pty_host", "LinuxX64")), "none"],
  ["cli cli.launch", p(rm(one("Cli"), "cli.launch", "WindowsX64")), "verified|"],
  ["script cli.launch", p(rm(one("Script"), "cli.launch", "WindowsX64")), "none"],
  ["page cpu.main", p(rm(one("ChromiumPage"), "cpu.main", "WindowsX64")), "none"],
  ["windowless ui.cpu", p(rm(one("NativeWindowless"), "ui.cpu", "WindowsX64")), "none"],
  ["native ui.wakes macos", p(rm(one("Native"), "ui.wakes", "MacosArm64")), "none"],
  ["script energy.cpu linux", p(rm(one("Script"), "energy.cpu", "LinuxX64")), "none"],
  ["script cpu.instructions linux", p(rm(one("Script"), "cpu.instructions", "LinuxX64")), `${PRE}capability`],
  ["page memory.app.pss windows", p(rm(one("ChromiumPage"), "memory.app.pss", "WindowsX64")), "none"],
  ["page memory.app.pss linux", p(rm(one("ChromiumPage"), "memory.app.pss", "LinuxX64")), `${PRE}capability`],
  ["comparison of cli and script, memory.peak", p(rm(two("Cli", "Script"), "memory.peak", "WindowsX64")), "none"],
  ["capability of cpu.work{x}", cap("cpu.work{x}"), "none"],
  ["capability of net.bytes{/a}", cap("net.bytes{/a}"), "page_engine"],
  ["capability of net.bytes{}", cap("net.bytes{}"), "none"],
  ["capability of net.bytes{/a", cap("net.bytes{/a"), "none"],
  ["capability of launch.checkin", cap("launch.checkin"), "none"],
  ["capability of launch.boot", cap("launch.boot"), "launch"],
  ["capability of launch.node.x", cap("launch.node.x"), "launch"],
  ["capability of count.x", cap("count.x"), "wall_and_counts"],
  ["capability of nope", cap("nope"), "none"],
  // Finding 7: canvas metrics are their own path.
  ["capability of canvas.interval.p95", cap("canvas.interval.p95"), "canvas"],
  ["page canvas.interval.p95", p(rm(one("ChromiumPage"), "canvas.interval.p95", "MacosArm64")), `${PRE}capability`],
  // Pixels in a launch, record runtimes, roll-ups.
  ["launch painted", pix({ until: { window: "painted" } }), "true"],
  ["launch settled", pix({ until: { window: "settled" } }), "true"],
  ["launch visible", pix({ until: { window: "visible" } }), "false"],
  ["launch image", pix({ until: { image: "ready.png" } }), "true"],
  ["launch pixels", pix({ until: { pixels: { changed: {} } } }), "true"],
  ["launch selector", pix({ until: "#ready" }), "false"],
  ["launch milestone", pix({ until: { milestone: "up" } }), "false"],
  ["launch until list", pix({ until: ["#a", { image: "x.png" }] }), "true"],
  ["uses_pixels by snapshots", `uses_pixels(0.0, &json!(null), true).to_string()`, "true"],
  ["uses_pixels by reads", `uses_pixels(2.0, &json!({}), false).to_string()`, "true"],
  ["uses_pixels none", `uses_pixels(0.0, &json!({ "until": "#a" }), false).to_string()`, "false"],
  ["record wir", record("wir:", null, false, false, false, false), "ios_safari"],
  ["record pid", record("pid:42", null, false, false, false, false), "native"],
  ["record app", record("app:demo", null, false, false, false, false), "chromium_app"],
  ["record app native", record("app:demo", null, false, true, false, false), "native"],
  ["record app native windowless", record("exe:demo", null, false, true, false, true), "native_windowless"],
  ["record app webview", record("app:demo", null, false, false, true, false), "webview_app"],
  ["record cdp", record("cdp:9222", null, false, false, false, false), "chromium_app"],
  ["record page firefox", record("https://example.com", "Firefox", false, false, false, false), "firefox_page"],
  ["record page phone", record("https://example.com", null, true, false, false, false), "android_chrome"],
  ["record page phone firefox", record("https://example.com", "Firefox", true, false, false, false), "none"],
  ["record local page", record("file:///C:/site/page.html", null, false, false, false, false), "chromium_page"],
  ["record trace import", record("file:trace.json", null, false, false, false, false), "none"],
  ["overall none", `overall(Vec::new()).map(Support::name).unwrap_or("none").to_string()`, "none"],
  ["overall mixed", `overall([Support::Verified, Support::Preview]).map(Support::name).unwrap_or("none").to_string()`, "preview"],
  ["comparison refused", `resolve_comparison(${one("Cli")}, "cpu.work", Host::WindowsX64, ${T}, true).map(Support::name).unwrap_or("none").to_string()`, "none"],
  ["comparison", `resolve_comparison(${one("Cli")}, "cpu.work", Host::WindowsX64, ${T}, false).map(Support::name).unwrap_or("none").to_string()`, "verified"],
  ["target_support cli windows", `target_support(Runtime::Cli, Host::WindowsX64).map(Support::name).unwrap_or("none").to_string()`, "preview"],
  ["target_support webview macos", `target_support(Runtime::WebviewApp, Host::MacosArm64).map(Support::name).unwrap_or("none").to_string()`, "none"],
  // Phrases, as the surfaces will print them.
  ["phrase windowless wall", phrase(`resolve(${one("NativeWindowless")}, Capability::WallAndCounts, Host::MacosArm64, ${T})`), "preview (windowless native · wall time and counts on macOS arm64: runtime checks held; not yet checked against a known-answer fixture)"],
  ["phrase windowless cpu", phrase(`resolve(${one("NativeWindowless")}, Capability::ProcessCpu, Host::MacosArm64, ${T})`), "verified (windowless native · process CPU on macOS arm64)"],
  ["phrase script heap", phrase(rm(one("Script"), "js.heap.retained", "LinuxX64")), "preview (script · JS heap on Linux x64: runtime checks held; not yet checked against a known-answer fixture)"],
  ["phrase phone", phrase(rm(one("AndroidChrome"), "main.work", "MacosArm64")), "preview (chrome on a phone · page engine (from macOS arm64): runtime checks held; not yet checked against a known-answer fixture)"],
  [
    "phrase comparison with pixels",
    phrase(`resolve(${two("Native", "ChromiumPage")}, Capability::Launch, Host::WindowsX64, ${th({ base_pixels: true, head_pixels: true })})`),
    "preview (native vs chromium page · launch (window pixels) on Windows x64: runtime checks held; the native side is not yet checked against a known-answer fixture; its reads of the window's pixels are not yet checked against a known answer)",
  ],
  [
    "phrase outside page with OS input",
    phrase(rm(one("ChromiumPage"), "frames.dropped.pct", "MacosArm64", th({ outside_page: true, os_input: true }))),
    "preview (chromium page · frames (page measured from outside, OS input) on macOS arm64: runtime checks held; a page measured from outside (the host's clock, the OS's frames) is not yet checked against a known answer; input through the OS is not yet checked against a known answer)",
  ],
  [
    "phrase outside page with OS input, preview frames",
    phrase(rm(one("ChromiumPage"), "frames.dropped.pct", "WindowsX64", th({ outside_page: true, os_input: true }))),
    "preview (chromium page · frames (page measured from outside, OS input) on Windows x64: runtime checks held; not yet checked against a known-answer fixture; a page measured from outside (the host's clock, the OS's frames) is not yet checked against a known answer; input through the OS is not yet checked against a known answer)",
  ],
  ["phrase verified", built(`tier: Support::Verified, runtime: Runtime::Cli, capability: Capability::ProcessCpu, host: Host::WindowsX64, preview_by: vec![]`), "verified (cli · process CPU on Windows x64)"],
  [
    "phrase secondary architecture",
    built(`tier: Support::Preview, runtime: Runtime::Cli, capability: Capability::ProcessCpu, host: Host::WindowsArm64, preview_by: vec![Cause::Architecture]`),
    "preview (cli · process CPU on Windows arm64: runtime checks held; verified on Windows x64 only)",
  ],
  ["status windows", `status_summary(Host::WindowsX64).line()`, "verified on Windows x64: windowless native (process CPU, process memory), cli (process CPU, process memory) · 82 more paths preview"],
  ["status linux arm64", `status_summary(Host::LinuxArm64).line()`, "every path is preview on Linux arm64: known answers are checked on Linux x64 only"],
]

// Where each catalog metric is read beyond its capability's row (`support::measures`): the runtimes that read
// it (`only`, else every runtime with the row; `not` takes some out) and the OSes (`os`, else every one). Every
// metric of the catalog (src/model.rs) must have an entry here, an empty one when its row says it all, so a new
// metric is placed by someone and `runtime_reads` is held to it for every runtime and OS. A family is read
// through one name of it (`count.x`).
type Where = { only?: string[]; not?: string[]; os?: ("windows" | "macos" | "linux")[] }
const PAGES_APPS = ["chromium_page", "firefox_page", "webkit_page", "chromium_app", "webview_app"]
const WINDOWED = ["native"]
const NATIVES = ["native", "native_windowless"]
const NOT_MAC: Where["os"] = ["windows", "linux"]
const WHERE: Record<string, Where> = {
  // Families.
  count: {},
  phase: {},
  span: { not: ["script"] },
  metric: { only: ["cli"] },
  app: { only: ["tty"] },
  "launch.milestone": {},
  // Wall time and strobe's own looks.
  "run.duration": {},
  // WebKit (WebDriver Classic) batches an input list inside the browser: no input times (engine_refusal).
  "input.rate": { only: [...PAGES_APPS.filter((r) => r !== "webkit_page"), "android_chrome", ...WINDOWED] },
  "input.gap.p95": { only: [...PAGES_APPS.filter((r) => r !== "webkit_page"), "android_chrome", ...WINDOWED] },
  "driver.reads": { only: [...PAGES_APPS, "android_chrome", ...WINDOWED] },
  "driver.read_ms": { only: [...PAGES_APPS, "android_chrome", ...WINDOWED] },
  // The OS's accounting.
  "cpu.pty_host": { only: ["tty"], os: ["windows"] },
  "cpu.app.total": {},
  "cpu.foreign": { os: NOT_MAC },
  "cpu.process.total": {},
  "cpu.renderer": { only: PAGES_APPS },
  "cpu.gpu": { only: PAGES_APPS },
  "cpu.browser": { only: PAGES_APPS },
  "cpu.fixture": {},
  "cpu.work": {},
  "cpu.main": { only: ["script", ...NATIVES] },
  "cpu.main.busy": { only: ["script", ...NATIVES] },
  "ui.cpu": { only: WINDOWED },
  "ui.wakes": { only: WINDOWED, os: NOT_MAC },
  "cpu.wakes": { only: [...NATIVES, "cli", "tty"] },
  "cli.launch": { only: ["cli"] },
  "io.read": { only: [...NATIVES, "cli", "tty"] },
  "io.write": { only: [...NATIVES, "cli", "tty"] },
  "memory.app.private": { not: ["cli"] },
  "memory.app.host": { only: NATIVES, os: NOT_MAC },
  "memory.app.pss": { not: ["cli", "tty"], os: ["linux"] },
  "memory.peak": { only: ["cli"] },
  "energy.cpu": { os: ["macos"] },
  "cpu.instructions": { os: ["macos", "linux"] },
  "cpu.idle_wakes": { os: ["macos"] },
  "gpu.process.3d": {},
  "gpu.vram": { os: NOT_MAC },
  "gpu.shared": { os: NOT_MAC },
  "js.heap.retained": { only: ["script"] },
}
// The metrics read wherever their capability's row is (a page engine's not on a script): each named, so a new
// metric of the catalog is placed here or in WHERE by someone.
const ROW_ONLY = new Set(
  [
    "canvas.frames canvas.interval.p50 canvas.interval.p95 canvas.interval.p99 canvas.interval.max",
    "frames.presented.p50 frames.presented.p90 frames.presented.p95 frames.presented.p99 frames.presented.max frames.presented.count",
    "frames.interval.p50 frames.interval.p95 frames.interval.p99 frames.interval.max frames.dropped.pct frames.dropped frames.discarded frames.partial",
    "frames.renderer.p95 frames.viz.p95",
    "http.latency.p50 http.latency.p95 http.latency.max http.timeouts http.errors http.missed_sends http.probes http.idle.p50",
    "interaction.p50 interaction.p95 scroll.latency.p50 scroll.latency.p95",
    "launch.ready",
    "main.busy main.work main.script main.gc main.layout main.style main.paint layout.count style.count main.tasks.long gpu.compile",
    "js.heap.used dom.nodes js.heap.growth dom.nodes.growth leak.suspects.retained layout.shift net.requests net.bytes",
    "driver.pixel_reads driver.pixel_ms driver.pixel_cpu_ms launch.painted launch.settled",
    "tty.latency.p50 tty.latency.p95 tty.settle.p50 tty.settle.p95 tty.updates tty.frames tty.frame.p50 tty.frame.p95 tty.bytes tty.cells",
  ].flatMap((l) => l.split(" ")),
)
// The page engine's metrics are a script's only where said (js.heap.retained); everything else of the row.
const PAGE_ENGINE_NOT_SCRIPT = { not: ["script"] }
const PROBE: Record<string, string> = { count: "count.x", phase: "phase.x", span: "span.x", metric: "metric.x", app: "app.x", "launch.milestone": "launch.x" }

// The catalog's metrics and their capabilities, read from src/model.rs.
function catalog(): [name: string, capability: string][] {
  const text = readFileSync(join(root, "src", "model.rs"), "utf8")
  const snake = (c: string) => c.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase()
  const seen = new Map<string, string>()
  for (const m of text.matchAll(/name: "([^"]+)",\s*capability: Capability::(\w+)/g)) seen.set(m[1], snake(m[2]))
  return [...seen]
}

// What `runtime_reads` must answer for a metric on a runtime and OS, by WHERE and the matrix's rows.
function expectedReads(s: Support, name: string, capability: string, runtime: string, os: "windows" | "macos" | "linux"): string {
  const w = WHERE[name] ?? (capability === "page_engine" ? PAGE_ENGINE_NOT_SCRIPT : {})
  const where = capability === "page_engine" && WHERE[name] && !WHERE[name].only && !WHERE[name].not ? { ...WHERE[name], ...PAGE_ENGINE_NOT_SCRIPT } : w
  const row = s.rows.find((r) => r.runtime === runtime && r.capability === capability)
  const reads = (o: "windows" | "macos" | "linux") =>
    !!row && row[o].kind !== "refused" && (!where.only || where.only.includes(runtime)) && !(where.not ?? []).includes(runtime) && (!where.os || where.os.includes(o))
  if (reads(os)) return "yes"
  return (["windows", "macos", "linux"] as const).some(reads) ? "not_here" : "never"
}

function rustChecks(s: Support, fail: (m: string) => void) {
  const metrics = catalog()
  for (const [name] of metrics) if (!(name in WHERE) && !ROW_ONLY.has(name)) fail(`reads: ${name} is in the catalog and not in WHERE or ROW_ONLY (scripts/dev/support-matrix.ts): say where it is read`)
  for (const name of [...Object.keys(WHERE), ...ROW_ONLY]) if (!metrics.some(([n]) => n === name)) fail(`reads: ${name} is in this file's table and not in the catalog`)
  const dir = join(tmpdir(), "strobe-support-check")
  mkdirSync(join(dir, "src"), { recursive: true })
  const repo = root.replace(/\\/g, "/")
  writeFileSync(
    join(dir, "Cargo.toml"),
    `[package]\nname = "support-check"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n[dependencies]\nstrobe = { path = "${repo}" }\nserde_json = "1"\n\n[workspace]\n`,
  )
  copyFileSync(join(root, "Cargo.lock"), join(dir, "Cargo.lock"))
  const hosts = s.hosts.map((h) => `Host::${h.replace(/(^|-)([a-z0-9])/g, (_, __, c: string) => c.toUpperCase())}`)
  const main = `// Written by scripts/dev/support-matrix.ts --check; not part of strobe.
use serde_json::json;
use strobe::model::WebEngine;
use strobe::support::*;
fn p(x: Option<PathSupport>) -> String {
    match x {
        None => "none".into(),
        Some(p) => format!("{}|{}", p.tier.name(), p.preview_by.iter().map(|c| serde_json::to_value(c).unwrap().as_str().unwrap().to_string()).collect::<Vec<_>>().join(",")),
    }
}
fn main() {
    let _ = (WebEngine::Chromium, json!(null), p(None));
    for host in [${hosts.join(", ")}] {
        for r in SUPPORT_MATRIX {
            println!("cell\\t{}\\t{}\\t{}\\t{}", host.name(), r.runtime.name(), r.capability.name(), support(r.runtime, r.capability, host).map(Support::name).unwrap_or("refused"));
        }
        let st = status_summary(host);
        println!("status\\t{}\\t{}\\t{}\\t{}", host.name(), st.verified.len(), st.preview, st.refused);
    }
    for r in Runtime::ALL { let j = serde_json::to_value(r).unwrap(); assert_eq!(j, json!(r.name())); assert_eq!(serde_json::from_value::<Runtime>(j).unwrap(), r); assert_eq!(Runtime::parse(r.name()), Some(r)); }
    for c in Capability::ALL { let j = serde_json::to_value(c).unwrap(); assert_eq!(j, json!(c.name())); assert_eq!(serde_json::from_value::<Capability>(j).unwrap(), c); assert_eq!(Capability::parse(c.name()), Some(c)); }
    for h in Host::ALL { let j = serde_json::to_value(h).unwrap(); assert_eq!(j, json!(h.name())); assert_eq!(serde_json::from_value::<Host>(j).unwrap(), h); assert_eq!(Host::parse(h.name()), Some(h)); }
    let ps = resolve(Sides::Two { base: Runtime::Native, head: Runtime::ChromiumPage }, Capability::Launch, Host::WindowsX64, Through { base_pixels: true, ..Through::default() }).unwrap();
    assert_eq!(serde_json::from_value::<PathSupport>(serde_json::to_value(&ps).unwrap()).unwrap(), ps);
    println!("roundtrip\\tok");
${CASES.map(([k, e]) => `    println!("case\\t{}\\t{}", ${JSON.stringify(k)}, ${e});`).join("\n")}
    for (name, probe) in [${metrics.map(([n]) => `(${JSON.stringify(n)}, ${JSON.stringify(PROBE[n] ?? n)})`).join(", ")}] {
        for r in Runtime::ALL {
            for os in Os::ALL {
                let got = match runtime_reads(r, probe, os) { Reads::Yes => "yes", Reads::NotHere(_) => "not_here", Reads::Never => "never" };
                println!("reads\\t{}\\t{}\\t{}\\t{}", name, r.name(), os.label().to_lowercase(), got);
            }
        }
    }
}
`
  writeFileSync(join(dir, "src", "main.rs"), main)
  const target = process.env.CARGO_TARGET_DIR ?? join(tmpdir(), "strobe-support-check-target")
  const run = Bun.spawnSync(["cargo", "run", "--offline", "--quiet", "--manifest-path", join(dir, "Cargo.toml")], { env: { ...process.env, CARGO_TARGET_DIR: target }, stdout: "pipe", stderr: "pipe" })
  if (run.exitCode !== 0) return fail(`rust: cargo run exited ${run.exitCode}:\n${run.stderr.toString().split("\n").slice(-30).join("\n")}`)
  const lines = run.stdout.toString().split(/\r?\n/).filter(Boolean)
  if (!lines.includes("roundtrip\tok")) fail("rust: no round trip line")
  let cells = 0
  for (const l of lines.filter((l) => l.startsWith("cell\t"))) {
    const [, host, runtime, capability, got] = l.split("\t")
    const row = s.rows.find((r) => r.runtime === runtime && r.capability === capability)
    const want = row ? onHost(row[osOf(host)], host).label : "missing"
    if (want !== got) fail(`rust: ${runtime} · ${capability} on ${host} is ${got}; the source reads ${want}`)
    cells++
  }
  if (cells !== s.rows.length * s.hosts.length) fail(`rust: ${cells} cells printed for ${s.rows.length} rows × ${s.hosts.length} hosts`)
  for (const l of lines.filter((l) => l.startsWith("status\t"))) {
    const [, host, verified, preview, refused] = l.split("\t")
    const labels = s.rows.map((r) => onHost(r[osOf(host)], host).label)
    const want = [new Set(s.rows.filter((r) => onHost(r[osOf(host)], host).label === "verified").map((r) => r.runtime)).size, labels.filter((x) => x === "preview").length, labels.filter((x) => x === "refused").length].join(",")
    if ([verified, preview, refused].join(",") !== want) fail(`rust: status on ${host} counts ${verified},${preview},${refused}; the source ${want}`)
  }
  const got = new Map(lines.filter((l) => l.startsWith("case\t")).map((l) => { const [, k, ...v] = l.split("\t"); return [k, v.join("\t")] as const }))
  for (const [k, , want] of CASES) {
    if (!got.has(k)) fail(`rust: case ${k} printed nothing`)
    else if (got.get(k) !== want) fail(`rust: ${k}: ${got.get(k)}, expected ${want}`)
  }
  let reads = 0
  const capOf = new Map(metrics)
  for (const l of lines.filter((l) => l.startsWith("reads\t"))) {
    const [, name, runtime, os, got] = l.split("\t")
    const want = expectedReads(s, name, capOf.get(name)!, runtime, os as "windows" | "macos" | "linux")
    if (want !== got) fail(`rust: runtime_reads(${runtime}, ${PROBE[name] ?? name}, ${os}) is ${got}; this file's table and the matrix say ${want}`)
    reads++
  }
  if (reads !== metrics.length * s.runtimes.length * 3) fail(`rust: ${reads} runtime_reads lines for ${metrics.length} metrics × ${s.runtimes.length} runtimes × 3 OSes`)
  console.log(`rust: ${cells} cells, ${CASES.length} cases, ${reads} runtime_reads`)
}

// ---------- the no-rot rule (SPEC 21.1) ----------
//
// A verified cell's evidence holds for the code that produced it. `WATCH` names, for each cell, the spans of code
// that compute its numbers; `--check` compares each span at the evidence's revision (`Evidence.version`, or
// `reviewed_at` when a person has read a later change and found it structural) with the working tree, after
// dropping comments, whitespace and a trailing comma before a closing bracket (what rustfmt changes), and fails on
// any other difference. Spans are functions (`file#name`, every fn of that name: one per OS under cfg;
// `file#Type::name`, `file#impl Type`, `file#mod name`, `file#const NAME`), a JavaScript block opened at an
// anchor's first `{` (`file#@anchor text`), or a whole small file (`file`).

type Os = "windows" | "macos" | "linux"
const OSES: Os[] = ["windows", "macos", "linux"]

// The text's tokens: comments and whitespace gone, a string or char literal one token (its quotes and escapes as
// written), every other punctuation character its own token. `js`: JavaScript quotes and template literals.
export function tokens(text: string, js = false): string[] {
  const out: string[] = []
  const n = text.length
  let i = 0
  const quoted = (q: string) => {
    let j = i + 1
    while (j < n && text[j] !== q) j += text[j] === "\\" ? 2 : 1
    out.push(text.slice(i, j + 1))
    i = j + 1
  }
  while (i < n) {
    const c = text[i]
    if (c === " " || c === "\t" || c === "\n" || c === "\r") {
      i++
    } else if (c === "/" && text[i + 1] === "/") {
      const e = text.indexOf("\n", i)
      i = e < 0 ? n : e
    } else if (c === "/" && text[i + 1] === "*") {
      // Rust's block comments nest.
      let depth = 1
      i += 2
      while (i < n && depth > 0) {
        if (!js && text.startsWith("/*", i)) (depth++, (i += 2))
        else if (text.startsWith("*/", i)) (depth--, (i += 2))
        else i++
      }
    } else if (!js && /^b?r#*"/.test(text.slice(i, i + 12)) && !/\w/.test(text[i - 1] ?? "")) {
      const m = text.slice(i).match(/^b?r(#*)"/)!
      const end = text.indexOf(`"${m[1]}`, i + m[0].length)
      const stop = end < 0 ? n : end + 1 + m[1].length
      out.push(text.slice(i, stop))
      i = stop
    } else if (js && c === "/" && /^$|^[(,=:[!&|?{};+\-*%<>~^]$|^(return|typeof|instanceof|in|of|new|delete|void|throw|case|do|else)$/.test(out[out.length - 1] ?? "")) {
      // A regular expression literal (where an operand starts), its class brackets and flags with it.
      let j = i + 1
      let cls = false
      while (j < n && (cls || text[j] !== "/") && text[j] !== "\n") {
        if (text[j] === "\\") j++
        else if (text[j] === "[") cls = true
        else if (text[j] === "]") cls = false
        j++
      }
      j++
      while (j < n && /\w/.test(text[j])) j++
      out.push(text.slice(i, j))
      i = j
    } else if (c === '"' || (js && c === "'")) {
      quoted(c)
    } else if (js && c === "`") {
      // A template literal, `${…}` parts and all, as one token.
      let j = i + 1
      let depth = 0
      while (j < n && (depth > 0 || text[j] !== "`")) {
        if (text[j] === "\\") j++
        else if (text[j] === "$" && text[j + 1] === "{") (depth++, j++)
        else if (text[j] === "}" && depth > 0) depth--
        j++
      }
      out.push(text.slice(i, j + 1))
      i = j + 1
    } else if (!js && c === "'" && /^'(\\(u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'])'/.test(text.slice(i, i + 12))) {
      quoted("'")
    } else if (/\w/.test(c)) {
      let j = i + 1
      while (j < n && /\w/.test(text[j])) j++
      out.push(text.slice(i, j))
      i = j
    } else {
      out.push(c)
      i++
    }
  }
  return out
}

// One canonical text for a token run: rustfmt adds and drops a trailing comma before a closing bracket.
const canon = (t: string[]) => t.filter((x, k) => !(x === "," && [")", "]", "}"].includes(t[k + 1] ?? ""))).join(" ")

type Item = { key: string; from: number; to: number }

// The index of the bracket that closes the one at `at`.
function closing(t: string[], at: number): number {
  const open = t[at]
  const close = ({ "{": "}", "(": ")", "[": "]" } as Record<string, string>)[open]
  let depth = 0
  for (let k = at; k < t.length; k++) {
    if (t[k] === open) depth++
    else if (t[k] === close && --depth === 0) return k
  }
  return t.length - 1
}

// Where an item starts: back from its keyword over its outer attributes (`#[cfg(…)]`, `#[inline]`) and its
// modifiers (`pub`, `pub(crate)`, `unsafe`, `async`, `const`, `extern "C"`, `default`), so a changed cfg or
// visibility is a change of the item.
const MODIFIERS = new Set(["pub", "unsafe", "async", "const", "extern", "default", "safe"])
function lead(t: string[], at: number): number {
  let j = at - 1
  for (;;) {
    if (j >= 0 && MODIFIERS.has(t[j])) j--
    else if (j >= 1 && t[j].startsWith('"') && t[j - 1] === "extern") j -= 2
    else if (j >= 0 && t[j] === ")") {
      let k = j
      for (let d = 0; k >= 0; k--) if (t[k] === ")") d++
      else if (t[k] === "(" && --d === 0) break
      if (t[k - 1] !== "pub") break
      j = k - 2
    } else if (j >= 0 && t[j] === "]") {
      let k = j
      for (let d = 0; k >= 0; k--) if (t[k] === "]") d++
      else if (t[k] === "[" && --d === 0) break
      if (t[k - 1] !== "#") break
      j = k - 2
    } else break
  }
  return j + 1
}
// The items of a Rust token stream: fns (`name`, and `Type::name` inside `impl Type`), `impl Type`, `mod name`,
// `const NAME` and `static NAME`, each with its token range.
export function items(t: string[]): Item[] {
  const out: Item[] = []
  const walk = (from: number, to: number, ty: string | null) => {
    let i = from
    while (i < to) {
      const w = t[i]
      if (w === "fn" && /^\w+$/.test(t[i + 1] ?? "")) {
        let j = i + 2
        let depth = 0
        while (j < to && !(depth === 0 && (t[j] === "{" || t[j] === ";"))) {
          if (t[j] === "(" || t[j] === "[") depth++
          else if (t[j] === ")" || t[j] === "]") depth--
          j++
        }
        if (t[j] !== "{") {
          i = j + 1
          continue
        }
        const end = closing(t, j)
        out.push({ key: ty ? `${ty}::${t[i + 1]}` : t[i + 1], from: lead(t, i), to: end })
        i = end + 1
      } else if (w === "impl" && (i === 0 || ["}", ";", "]"].includes(t[i - 1]) || /^(pub|unsafe)$/.test(t[i - 1]))) {
        let j = i + 1
        while (j < to && t[j] !== "{" && t[j] !== ";") j++
        if (t[j] !== "{") {
          i = j + 1
          continue
        }
        // The type: after `for` when there is one, generics left out, the last name of its path.
        let head = t.slice(i + 1, j)
        const angle: string[] = []
        let d = 0
        for (const x of head) {
          if (x === "<") d++
          else if (x === ">" && d > 0) d--
          else if (d === 0) angle.push(x)
        }
        head = angle
        const where = head.indexOf("where")
        if (where >= 0) head = head.slice(0, where)
        const f = head.lastIndexOf("for")
        const name = [...(f >= 0 ? head.slice(f + 1) : head)].reverse().find((x) => /^[A-Za-z_]\w*$/.test(x) && x !== "dyn" && x !== "mut") ?? "?"
        const end = closing(t, j)
        out.push({ key: `impl ${name}`, from: lead(t, i), to: end })
        walk(j + 1, end, name)
        i = end + 1
      } else if (w === "mod" && /^\w+$/.test(t[i + 1] ?? "") && t[i + 2] === "{") {
        const end = closing(t, i + 2)
        out.push({ key: `mod ${t[i + 1]}`, from: lead(t, i), to: end })
        walk(i + 3, end, null)
        i = end + 1
      } else if ((w === "const" || w === "static") && /^[A-Z_][A-Z0-9_]*$/.test(t[i + 1] ?? "")) {
        let j = i + 2
        while (j < to && t[j] !== ";") j = ["{", "(", "["].includes(t[j]) ? closing(t, j) + 1 : j + 1
        out.push({ key: `${w} ${t[i + 1]}`, from: lead(t, i), to: j })
        i = j + 1
      } else if (w === "{" || w === "(" || w === "[") {
        // A struct, an enum, an extern block or a macro's body: nothing watched is inside.
        i = closing(t, i) + 1
      } else {
        i++
      }
    }
  }
  walk(0, t.length, null)
  return out
}

// The canonical texts of a span in a file's text, one per item it names (several: a fn per OS under cfg); null
// when it names none.
// A file's tokens and items, kept per text: a check reads each span of a file at a few revisions, many times.
const parsedTexts = new Map<string, { t: string[]; its?: Item[] }>()
export function spanTexts(text: string, sel: string | null, js: boolean): string[] | null {
  let p = parsedTexts.get(text)
  if (!p) parsedTexts.set(text, (p = { t: tokens(text.replace(/\r\n/g, "\n"), js) }))
  const t = p.t
  if (sel === null) return [canon(t)]
  if (sel.startsWith("@")) {
    const anchor = tokens(sel.slice(1), js)
    for (let k = 0; k + anchor.length <= t.length; k++) {
      if (anchor.every((a, j) => t[k + j] === a)) {
        // The block that opens at the anchor's first `{` (or the first after it).
        let b = k
        while (b < t.length && t[b] !== "{") b++
        return [canon(t.slice(k, closing(t, b) + 1))]
      }
    }
    return null
  }
  const found = (p.its ??= items(t)).filter((it) => it.key === sel || (!sel.includes("::") && !sel.includes(" ") && it.key.endsWith(`::${sel}`)))
  return found.length ? found.map((it) => canon(t.slice(it.from, it.to + 1))) : null
}

// Where two canonical texts first differ, a few tokens of each around it.
function near(a: string, b: string): string {
  const x = a.split(" ")
  const y = b.split(" ")
  const limit = Math.min(x.length, y.length)
  let k = 0
  for (; k < limit; k++) if (x[k] !== y[k]) break
  const ctx = (v: string[]) => v.slice(Math.max(0, k - 6), k + 8).join(" ")
  return `then: …${ctx(x)}… now: …${ctx(y)}…`
}

export type Watch = { cells: string[]; os?: Os[]; spans: string[]; why: string }

// What computes each verified cell's numbers. A cell is `runtime · capability` (`runtime · *`: each of the
// runtime's; `*`: every verified cell, which alone maps none). Each group says why those spans are the ones: what
// in them makes the number, and why the rest of their files does not.
export const WATCH: Watch[] = [
  // ----- every cell -----
  {
    cells: ["*"],
    spans: [
      "src/stats.rs#compare",
      "src/stats.rs#diffs",
      "src/stats.rs#hodges_lehmann",
      "src/stats.rs#wilcoxon_p",
      "src/stats.rs#signed_rank_counts",
      "src/stats.rs#normal_quantile",
      "src/stats.rs#erfc",
      "src/stats.rs#two_tail",
      "src/metrics.rs#median",
    ],
    why: "every quoted A/B and A/A is compare's estimate and interval: the per-pair log ratio or difference (diffs), its Hodges-Lehmann shift and signed-rank bounds; the verdict words, Holm, the noworse and planning code beside them move no number",
  },
  {
    cells: ["*"],
    spans: [
      "src/app/bench/block.rs#impl Warmup",
      "src/app/bench/block.rs#steady",
      "src/metrics.rs#impl Timed",
      "src/metrics.rs#block_values",
      "src/app/bench.rs#untimed_values",
      "src/app/bench/block.rs#untimed_state",
    ],
    why: "a block's runs to its values: which runs are measured (warmup ends once two runs cost the same), a run's span less its untimed stretches (Timed), the per-run values reduced to the block's (block_values, its median) and the untimed share said and refused (untimed_values, untimed_state)",
  },
  {
    cells: ["*"],
    spans: ["sdk/bench/src/runner.ts#@const request =", "sdk/bench/src/runner.ts#@const context =", 'sdk/bench/src/runner.ts#@case "run": {'],
    why: "the SDK runner's side of a run: the run handler strobe times, and the context the case calls (ctx.exec, ctx.call, ctx.phase, ctx.untimed, ctx.count, ctx.waitForLine) and the request each is; the rest of runner.ts validates the bench file and drives Playwright",
  },
  {
    cells: ["*"],
    spans: ["src/host/mod.rs#now_ns", "src/app/bench.rs#lopsided_background", "src/validity.rs#impl CpuBusy", "src/validity.rs#impl Busy", "src/app/bench.rs#refuse_not_on_this_os", "src/app/outside.rs#not_on_this_os"],
    why: "the host clock a run's edges, phases and Clock are read on; the other work on the target's CPUs over a run (cpu.foreign, Background); and the refusals of what this OS cannot give (NotOnThisOs), which several cells cite as their boundary",
  },
  { cells: ["*"], os: ["windows"], spans: ["src/host/win.rs#qpc_ns", "src/host/win.rs#qpc_ticks_ns"], why: "Windows: the host clock (QueryPerformanceCounter in ns)" },
  { cells: ["*"], os: ["linux"], spans: ["src/host/linux.rs#clock", "src/host/linux.rs#now_ns"], why: "Linux: the host clock (CLOCK_MONOTONIC in ns)" },
  { cells: ["*"], os: ["macos"], spans: ["src/host/macos.rs#now_ns", "src/host/macos.rs#mach_ns", "src/host/macos.rs#timebase"], why: "macOS: the host clock (mach time in ns)" },
  // ----- a command (cli) -----
  {
    cells: ["cli · *"],
    spans: [
      "src/app/bench/cli.rs#line_for",
      "src/app/bench/cli.rs#simple",
      "src/app/bench/cli.rs#command",
      "src/app/bench/cli.rs#exec",
      "src/app/bench/cli.rs#block",
      "src/app/bench/cli.rs#metric_line",
      "src/app/bench/cli.rs#sum_of",
      "src/app/bench/script.rs#cpu_ms",
      "src/host/spawn.rs#each_line",
      "src/host/spawn.rs#spawn_group",
      "src/host/spawn.rs#windows",
      "src/host/spawn.rs#new_job",
      "src/host/spawn.rs#assign",
      "src/host/spawn.rs#nt_resume",
      "src/host/spawn.rs#impl Spawned",
      "src/host/spawn.rs#Owned::spawn_span",
      "src/host/spawn.rs#Owned::run_to_exit",
      "src/host/mod.rs#tsc",
      "src/host/mod.rs#thread_cycles",
      "src/host/mod.rs#cycles_refused",
    ],
    why: "a command's run: the line as spawned (line_for, simple, command: whether a shell stays its parent), its two edges and launch CPU (exec; Windows' CreateProcessW, its job (new_job, assign: what breaks away stays in its count) and resume in windows, nt_resume and Spawned::around, spawn_span; cpu_ms on the cycle clock), its output lines (each_line), its job or group and what the OS counted at its exit (spawn_group, run_to_exit: job cycles and PeakProcessMemoryUsed on Windows, wait4's rusage on Unix), and the block's values from them (block, metric_line, sum_of); strobe's own cost (SelfCost) and the stamps' writing are not the command's numbers",
  },
  {
    cells: ["cli · *"],
    spans: [
      "src/host/spawn.rs#still_left",
      "src/host/spawn.rs#group_members",
      "src/host/spawn.rs#job_pids",
      "src/host/spawn.rs#stamped_with",
      "src/host/spawn.rs#stamped",
      "src/host/spawn.rs#stamped_in",
      "src/host/spawn.rs#stamp_of",
      "src/host/spawn.rs#stamp_in",
      "src/host/spawn.rs#parse_owner",
      "src/host/spawn.rs#home_key",
      "src/host/spawn.rs#owner_stamp",
      "src/host/mod.rs#process_list",
    ],
    why: "the cli cells' boundary: what a command left running is found by its group (Unix), its job (Windows) or the stamp it inherited (a process that broke away), and exec refuses the run RunIncomplete; how leftovers are then ended (end_left) refuses nothing",
  },
  { cells: ["cli · process_cpu", "cli · wall_and_counts"], os: ["windows"], spans: ["src/host/win.rs#thread_cycles"], why: "Windows: the launching thread's cycles, cli.launch's CPU" },
  { cells: ["cli · process_cpu", "cli · wall_and_counts"], os: ["linux"], spans: ["src/host/linux.rs#thread_cycles"], why: "Linux: the launching thread's CPU, cli.launch's" },
  { cells: ["cli · process_cpu", "cli · wall_and_counts"], os: ["macos"], spans: ["src/host/macos.rs#thread_cycles"], why: "macOS: the launching thread's CPU, cli.launch's" },
  {
    cells: ["cli · process_memory"],
    os: ["linux"],
    spans: ["src/host/spawn.rs#peak_floor", "src/host/spawn.rs#into_cgroup"],
    why: "Linux: the resident set a child inherits from strobe before its exec (peak_floor, larger when a pre_exec makes Rust fork, as into_cgroup's does), under which run_to_exit masks a peak as not the command's: the cell's 4 MiB boundary",
  },
  // ----- counts, phases and metric lines (wall_and_counts) -----
  {
    cells: ["cli · wall_and_counts", "script · wall_and_counts"],
    spans: ["src/app/bench/block.rs#impl Hooks", "src/app/bench/block.rs#per_run_hooks", "src/app/bench.rs#phase_ms", "src/metrics.rs#phase_stretches"],
    why: "the counts, phases and untimed stretches a run reports, kept per run and reduced (Hooks, per_run_hooks, phase_ms on the host clock, phase_stretches)",
  },
  // ----- a script (script) -----
  {
    cells: ["script · *"],
    spans: [
      "src/app/bench/script.rs#chunk",
      "src/app/bench/script.rs#Host::spawn",
      "src/app/bench/script.rs#Host::request",
      "sdk/bench/src/host.mjs#@try { const fn",
      "src/app/bench.rs#self_state",
      "src/app/bench.rs#impl SelfCost",
      "src/app/bench.rs#script_unread",
      "src/app/bench.rs#script_unread_refusal",
    ],
    why: "a script run: chunk holds its edges (the request's send and reply on the host clock), its CPU windows and the block's values; Host::spawn starts the host and reads which runtime it is; request is the round trip timed; host.mjs's call block is what runs inside it; the cells' boundaries are OverheadBudget for sub-millisecond runs (self_state, from strobe's and the runner's share of a core that SelfCost measures) and NotForTarget for cpu.wakes and io.* (script_unread_refusal); the heap, clock and exit ops beside the call are no part of a run's wall or CPU",
  },
  // ----- the processes a run's CPU, memory and counters are of -----
  {
    cells: ["script · process_cpu", "native_windowless · *"],
    spans: [
      "src/app/record.rs#per_process",
      "src/app/record.rs#read_known",
      "src/app/record.rs#read_at",
      "src/app/record.rs#impl Clock",
      "src/app/record.rs#cycles_used",
      "src/app/record.rs#cycles_timed",
      "src/app/record.rs#read_set",
      "src/app/record.rs#set_used",
      "src/app/record.rs#gone",
      "src/app/record.rs#timed_cpu",
      "src/app/record.rs#cpu_lost",
      "src/app/bench/script.rs#cycles",
      "src/host/mod.rs#process_cycles",
      "src/host/mod.rs#process_cycles_at",
      "src/host/mod.rs#process_tree",
      "src/host/mod.rs#process_trees",
      "src/host/mod.rs#trees",
      "src/host/mod.rs#process_set",
      "src/host/mod.rs#tree_pids",
      "src/host/mod.rs#start_time",
      "src/host/mod.rs#ended_marks",
      "src/host/mod.rs#impl EndedMarks",
      "src/host/mod.rs#launch_ended_cpu",
    ],
    why: "which processes a run's CPU, memory and counters are of (the tree and set from the root, those that ended inside the run, their CPU from the launch's cgroup or job) and the CPU of each between the run's edges, on the cycle clock (Clock, timed_cpu); the rest of record.rs writes recordings",
  },
  {
    cells: ["script · process_cpu", "native_windowless · *"],
    spans: [
      "src/host/mod.rs#impl MainThread",
      "src/host/mod.rs#macos_lost",
      "src/host/mod.rs#main_share",
      "src/host/mod.rs#threads",
      "src/host/mod.rs#thread_cycles_of",
      "src/host/mod.rs#switches_since",
      "src/host/spawn.rs#Owned::main_thread",
    ],
    why: "the main thread's CPU and share (cpu.main, cpu.main.busy) and the threads a process's wakes and switches are summed over",
  },
  {
    cells: ["script · process_cpu", "native_windowless · *"],
    os: ["windows"],
    spans: [
      "src/host/win.rs#process_trees",
      "src/host/win.rs#tree_of",
      "src/host/win.rs#tree_pids_of",
      "src/host/win.rs#tree_pids",
      "src/host/win.rs#process_list",
      "src/host/win.rs#process_cycles",
      "src/host/win.rs#proc_times",
      "src/host/win.rs#each_process",
      "src/host/win.rs#system_processes",
      "src/host/win.rs#procs",
      "src/host/win.rs#start_time",
      "src/host/win.rs#tree_threads",
      "src/host/win.rs#threads",
      "src/host/win.rs#thread_cycles_of",
      "src/host/win.rs#impl ThreadHandle",
      "src/host/win.rs#memory_map",
      "src/host/jobs.rs",
    ],
    why: "Windows' readers: the process list and the tree walked from the root, each process's cycles and private bytes (pagefile commit), its threads' cycles (the main thread's by its handle), its memory map, and the job objects whose accounting holds the CPU of processes that ended inside a run",
  },
  {
    cells: ["script · process_cpu", "native_windowless · *"],
    os: ["linux"],
    spans: [
      "src/host/linux.rs#process_trees",
      "src/host/linux.rs#tree_of",
      "src/host/linux.rs#tree_pids_of",
      "src/host/linux.rs#tree_pids",
      "src/host/linux.rs#process_list",
      "src/host/linux.rs#zombies_of",
      "src/host/linux.rs#process_cycles",
      "src/host/linux.rs#process_cpu_ns",
      "src/host/linux.rs#read",
      "src/host/linux.rs#tck",
      "src/host/linux.rs#stat",
      "src/host/linux.rs#all_stats",
      "src/host/linux.rs#start_time",
      "src/host/linux.rs#schedstat",
      "src/host/linux.rs#threads",
      "src/host/linux.rs#thread_cycles_of",
      "src/host/linux.rs#tids",
      "src/host/linux.rs#private_bytes",
      "src/host/linux.rs#anonymous_kb",
      "src/host/linux.rs#memory_map",
      "src/host/cgroup.rs#read",
      "src/host/cgroup.rs#impl Reading",
      "src/host/cgroup.rs#create",
      "src/host/cgroup.rs#impl Launch",
      "src/host/spawn.rs#spawn_group",
      "src/host/spawn.rs#into_cgroup",
    ],
    why: "Linux's readers: /proc's stat (tree, start times, zombies), each process's and thread's CPU ns from schedstat, private bytes from its anonymous memory, its memory map, and the launch cgroup a launch moves into (spawn_group, into_cgroup), whose CPU counts the processes that ended",
  },
  {
    cells: ["script · process_cpu", "native_windowless · *"],
    os: ["macos"],
    spans: [
      "src/host/macos.rs#process_trees",
      "src/host/macos.rs#tree_of",
      "src/host/macos.rs#tree_pids",
      "src/host/macos.rs#process_list",
      "src/host/macos.rs#zombies_of",
      "src/host/macos.rs#responsible_for",
      "src/host/macos.rs#responsible_pids",
      "src/host/macos.rs#all_bsd",
      "src/host/macos.rs#bsd",
      "src/host/macos.rs#process_cycles",
      "src/host/macos.rs#rusage",
      "src/host/macos.rs#start_time",
      "src/host/macos.rs#task_info",
      "src/host/macos.rs#thread_ids",
      "src/host/macos.rs#thread_info",
      "src/host/macos.rs#threads",
      "src/host/macos.rs#thread_cpu",
      "src/host/macos.rs#thread_cycles_of",
      "src/host/macos.rs#first_thread_id",
      "src/host/macos.rs#known_thread",
    ],
    why: "macOS's readers: the process list and tree (with the processes an app is responsible for), each process's CPU and phys_footprint from proc_pid_rusage, its threads' CPU (the main thread is the first), on the mach clock",
  },
  // ----- a windowless native app (native_windowless) -----
  {
    cells: ["native_windowless · *"],
    spans: [
      "src/app/bench/native.rs#runs",
      "src/app/bench/native.rs#cpu_ms",
      "src/target/native.rs#Native::processes",
      "src/target/native.rs#Native::pids",
      "src/target/native.rs#Native::main_thread",
      "src/app/bench/native.rs#missing",
      "src/app/bench/launch.rs#app_windowless",
      "src/app/record.rs#exited",
      "src/app/record.rs#exited_refusal",
      "src/app/record.rs#app_gone",
    ],
    why: "a native block: runs holds each run's edges (and whether the app has a window to wait for, app_windowless), CPU, counters and end-of-run memory, cpu_ms turns cycles into ms, the app's processes and main thread come from Native; the cells' boundary is an app that exits before or in a run, refused RunIncomplete (exited, exited_refusal, app_gone), and missing says why a declared metric has no value (memory.app.host NotOnThisOs on macOS); the launch and window code elsewhere in native.rs is for launch cells and windowed apps",
  },
  {
    cells: ["native_windowless · process_memory"],
    spans: ["src/probe/memory.rs", "src/host/mod.rs#memory_map", "src/host/mod.rs#pss_bytes"],
    why: "the memory reading at a run's end over the app's processes (memory.rs: its parts and totals, memory.app.host from the memory map) and the proportional set size (pss_bytes)",
  },
  {
    cells: ["native_windowless · process_cpu", "native_windowless · hardware_counters"],
    spans: [
      "src/probe/wakes.rs#Snapshot::take",
      "src/probe/wakes.rs#Snapshot::take_since",
      "src/probe/wakes.rs#span",
      "src/probe/wakes.rs#impl Span",
      "src/probe/wakes.rs#wall_us",
      "src/probe/wakes.rs#impl Counters",
      "src/probe/wakes.rs#counted",
      "src/probe/wakes.rs#counter_values",
      "src/probe/wakes.rs#counter_metrics",
      "src/probe/wakes.rs#impl Uncounted",
    ],
    why: "wakes and the OS's per-process counters between a run's edges (instructions, cycles, energy), the metrics named from them, and NoCounter when a process the counts need is gone (Uncounted); the timeline and thread views in wakes.rs are for recordings",
  },
  {
    cells: ["native_windowless · hardware_counters"],
    os: ["macos"],
    spans: ["src/host/macos.rs#counts", "src/host/macos.rs#energy_counters"],
    why: "macOS: instructions, cycles and energy from proc_pid_rusage's v6 record",
  },
  // ----- a headed page (chromium_page frames and input latency) -----
  {
    cells: ["chromium_page · frames", "chromium_page · input_latency"],
    spans: [
      "src/app/bench.rs#run_chunk",
      "src/probe/trace.rs",
      "src/metrics.rs#read",
      "src/metrics.rs#vsyncs",
      "src/metrics.rs#intervals",
      "src/metrics.rs#vsync_ms",
      "src/metrics.rs#covered",
      "src/metrics.rs#outside",
      "src/metrics.rs#stretches",
      "src/metrics.rs#run_frames",
      "src/metrics.rs#run_intervals",
      "src/metrics.rs#phase_frames",
      "src/metrics.rs#windows_sql",
      "src/metrics.rs#doc_mark_script",
      "src/metrics.rs#mark_windows",
      "src/metrics.rs#discrete_samples",
      "src/metrics.rs#percentile",
      "src/metrics.rs#impl BlockData",
      "src/app/bench.rs#tail_of",
      "src/app/bench.rs#tail_refusal",
      "src/app/bench.rs#tail_state",
      "src/app/bench.rs#headless_state",
      "src/app/bench.rs#headless_of",
      "src/app/bench.rs#side_windowless",
      "src/app/bench.rs#windowless_as",
      "src/app/bench.rs#windowless_target",
      "src/app/bench/launch.rs#app_windowless",
      "src/metrics.rs#split_metric",
      "src/metrics.rs#split_refusal",
      "src/validity.rs#display_metric",
    ],
    why: "a headed page block: run_chunk sets up the trace and the run's marks and reads the block's values; trace.rs is the trace's categories; metrics.rs reads frames (vsyncs, intervals) and EventLatency from the trace within each run's windows (covered, outside, stretches) and reduces them to a block's p50/p95; the cells' boundaries are a tail too short for its percentile (tail_of, tail_refusal, tail_state: TailTooShort) and a headless page's (headless_state, display_metric: Headless); whether a page is headless or windowless decides its trace (headless_of, side_windowless) and split_metric which percentile a name reads; the series, report and launch code in bench.rs and the recording views in metrics.rs move none of it",
  },
  {
    cells: ["chromium_page · input_latency"],
    spans: [
      "sdk/bench/src/runner.ts#@const page = {",
      "src/driver.rs#Driver::dispatch",
      "src/driver.rs#Driver::mouse",
      "src/driver.rs#Driver::input_at",
      "src/driver.rs#Driver::spot",
      "src/driver.rs#Driver::aim",
      "src/driver.rs#Driver::aim_for",
      "src/driver.rs#Driver::locate",
      "src/driver.rs#Driver::pointer_move",
      "src/driver.rs#Driver::pointer_button",
      "src/driver.rs#Driver::stroke",
      "src/driver.rs#Driver::key_event",
      "src/driver.rs#Driver::key",
      "src/driver.rs#Driver::settle",
      "src/driver.rs#Driver::settle_key",
      "src/driver.rs#sent",
      "src/driver.rs#take_sending",
      "src/driver.rs#cdp_button",
      "src/driver.rs#cdp_mods",
      "src/driver.rs#input_pacing",
      "src/driver.rs#impl Pacer",
    ],
    why: "how an input reaches the page and when it was sent: the click (and a key's) path from the SDK's page object through dispatch, aiming and CDP's mouse and key events, its pacing and settling, whose latency the trace times; the driver's waits, queries and screenshots are no part of it",
  },
]

// The groups a cell on an OS is held to.
export function watchFor(runtime: string, capability: string, os: Os): Watch[] {
  const cell = `${runtime} · ${capability}`
  return WATCH.filter((w) => (w.cells.includes("*") || w.cells.includes(cell) || w.cells.includes(`${runtime} · *`)) && (!w.os || w.os.includes(os)))
}

const git = (args: string[]) => {
  const r = Bun.spawnSync(["git", ...args], { cwd: root, stdout: "pipe", stderr: "pipe", timeout: 120_000 })
  return r.exitCode === 0 ? r.stdout.toString() : null
}
let fetched = false
// A revision's commit, or null when this repository has none by that name. A shallow clone fetches its history
// once (CI checks out the whole history; this is for a local shallow clone).
const commits = new Map<string, string | null>()
function commitOf(rev: string): string | null {
  if (commits.has(rev)) return commits.get(rev)!
  const id = () => git(["rev-parse", "--verify", "-q", `${rev}^{commit}`])?.trim() || null
  if (id() === null && !fetched) {
    fetched = true
    if (git(["rev-parse", "--is-shallow-repository"])?.trim() === "true") git(["fetch", "-q", "--unshallow", "--tags", "origin"])
    else git(["fetch", "-q", "--tags", "origin"])
  }
  const c = id()
  commits.set(rev, c)
  return c
}
// `a` is `b` or one of its ancestors: in its history, so no landing of `b` can lose it.
const ancestry = new Map<string, boolean>()
const isAncestor = (a: string, b: string) => {
  const k = `${a} ${b}`
  if (!ancestry.has(k)) ancestry.set(k, git(["merge-base", "--is-ancestor", `${a}^{commit}`, `${b}^{commit}`]) !== null)
  return ancestry.get(k)!
}
const shown = new Map<string, string | null>()
function fileAt(rev: string, path: string): string | null {
  const key = `${rev}:${path}`
  if (!shown.has(key)) shown.set(key, commitOf(rev) === null ? null : git(["show", key]))
  return shown.get(key)!
}

// The code checked: the working tree (its history HEAD's), or the tree a landing would leave (`--landing`), whose
// history is its tips'.
export type Tree = { what: string; read: (path: string) => string | null; tips: string[] }
const cached = (read: (path: string) => string | null) => {
  const seen = new Map<string, string | null>()
  return (p: string) => (seen.has(p) ? seen.get(p)! : (seen.set(p, read(p)), seen.get(p)!))
}
export const WORKING: Tree = { what: "the working tree", read: cached((p) => (existsSync(join(root, p)) ? readFileSync(join(root, p), "utf8") : null)), tips: ["HEAD"] }

// What a watched span reads in a file source: null when the file or item is not there.
function spanIn(read: (path: string) => string | null, spec: string): string[] | null {
  const [path, sel] = spec.includes("#") ? [spec.slice(0, spec.indexOf("#")), spec.slice(spec.indexOf("#") + 1)] : [spec, null]
  const text = read(path)
  return text === null ? null : spanTexts(text, sel, /\.(m?js|ts)$/.test(path))
}
const spanAt = (rev: string, spec: string) => spanIn((p) => fileAt(rev, p), spec)
const same = (a: string[] | null, b: string[] | null) => (a === null ? b === null : b !== null && a.join(" ¦ ") === b.join(" ¦ "))

// The name a review sentence uses for a span: its item's last name (`run_to_exit`, `Hooks`), else its file's.
export function spanName(spec: string): string {
  const [path, sel] = spec.includes("#") ? [spec.slice(0, spec.indexOf("#")), spec.slice(spec.indexOf("#") + 1)] : [spec, ""]
  if (!sel || sel.startsWith("@")) return path.split("/").pop()!.replace(/\.(rs|ts|mjs|js)$/, "")
  return sel.split(/::| /).pop()!
}
const names = (text: string, name: string) => new RegExp(`(^|[^\\w])${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}($|[^\\w])`).test(text)

const OS_LABEL: Record<Os, string> = { windows: "Windows x64", macos: "macOS arm64", linux: "Linux x64" }
const LANDING = "A branch that sets a stamp lands by fast-forward or a merge commit, never by squash or cherry-pick (both give its commits new ids); rebased, it re-stamps reviewed_at to the rebased code commit in its second commit. Before landing: bun scripts/dev/support-matrix.ts --landing <branch>"

// Every verified cell whose evidence or review is not in the tree's history, whose review says nothing of what it
// reviewed, or whose watched spans changed since: what is wrong and the three ways out. `report` gets one line per
// cell and span, held or not.
export function rotProblems(s: Support, fail: (m: string) => void, report?: (m: string) => void, tree: Tree = WORKING) {
  const head = git(["rev-parse", "--short=10", tree.tips[tree.tips.length - 1]])?.trim() ?? tree.tips[0]
  const inHistory = (rev: string) => tree.tips.some((t) => isAncestor(rev, t))
  for (const r of s.rows) {
    for (const os of OSES) {
      const c = r[os]
      if (c.kind !== "verified") continue
      const e = c.evidence
      const cell = `${r.runtime} · ${r.capability} on ${OS_LABEL[os]}`
      const groups = watchFor(r.runtime, r.capability, os)
      if (!groups.some((g) => !g.cells.includes("*"))) {
        fail(`${cell} is verified, and WATCH (scripts/dev/support-matrix.ts) names no code that computes it: map its spans`)
        continue
      }
      // The evidence build and the review are commits of the tree's history, the review after the build.
      let ok = true
      for (const [what, rev] of [["evidence version", e.version], ["reviewed_at", e.reviewed_at]] as const) {
        if (rev === undefined) continue
        if (commitOf(rev) === null) fail(`${cell}: its ${what} ${rev} is no commit of this repository`)
        else if (!inHistory(rev))
          fail(`${cell}: its ${what} ${rev} is not in the history of ${tree.what} (${tree.tips.join(", ")}): a commit of another branch, or one a squash, cherry-pick or rebase replaced. ${LANDING}`)
        else continue
        ok = false
      }
      if (ok && e.reviewed_at && !isAncestor(e.version, e.reviewed_at)) {
        fail(`${cell}: its reviewed_at ${e.reviewed_at} is not after its evidence version ${e.version} (the version must be an ancestor of the review)`)
        ok = false
      }
      if (!ok) continue
      const specs = groups.flatMap((g) => g.spans.map((spec) => ({ spec, why: g.why })))
      // A review says what it reviewed: some watched span differs between the evidence and it, the sentence names
      // every one that does and a commit.
      if (e.reviewed_at) {
        const reviewed = specs.filter(({ spec }) => !same(spanAt(e.version, spec), spanAt(e.reviewed_at!, spec))).map(({ spec }) => spec)
        const why = e.reviewed_why ?? ""
        if (!reviewed.length) fail(`${cell}: reviewed_at ${e.reviewed_at} reviews nothing: no watched span differs between ${e.version} and it. Drop reviewed_at and reviewed_why`)
        const unnamed = reviewed.filter((spec) => !names(why, spanName(spec)))
        if (unnamed.length) fail(`${cell}: reviewed_why names no ${unnamed.map((x) => `${spanName(x)} (${x})`).join(", ")}, which changed between ${e.version} and ${e.reviewed_at}: say why each cannot move the numbers`)
        if (!/(^|[^\w])[0-9a-f]{7,40}($|[^\w])/.test(why)) fail(`${cell}: reviewed_why names no commit: name the commits whose changes it reviewed`)
      }
      const rev = e.reviewed_at || e.version
      const changed: string[] = []
      for (const { spec, why } of specs) {
        const then = spanAt(rev, spec)
        const now = spanIn(tree.read, spec)
        if (now === null) changed.push(`${spec}: not in ${tree.what} (renamed or moved? map the code that computes it now) [${why}]`)
        else if (then === null) changed.push(`${spec}: not at ${rev} (new since the evidence, or renamed) [${why}]`)
        else if (!same(then, now)) changed.push(`${spec} changed [${why}]\n      ${near(then.join(" ¦ "), now.join(" ¦ "))}`)
        else report?.(`held    ${cell} since ${rev}: ${spec}`)
      }
      if (!changed.length) continue
      for (const x of changed) report?.(`CHANGED ${cell} since ${rev}: ${x.split("\n")[0]}`)
      const since = e.reviewed_at ? `${e.version}'s runs, reviewed at ${e.reviewed_at}` : `${e.version}'s runs`
      fail(
        `${cell} is verified from ${since} (\`${e.fixture}\`), and the code that computes it changed since:\n    ${changed.join("\n    ")}\n` +
          `  Three ways out, in the cell's Evidence in src/support.rs (and its line in SPEC 21.1):\n` +
          `    1. run ${e.fixture} again on the evidence's machine (${e.machine}): its known answer, A/A, planted A/B and boundary case; write the new numbers, date and version;\n` +
          `    2. or flip the cell to Preview("re-verify ${e.fixture} after <the commit>: <what changed>");\n` +
          `    3. or, when the change moves no timestamp, attribution or arithmetic (a rename, a moved block, a refusal beside the reading), commit it, then in a second commit set reviewed_at: Some("<that commit>") and reviewed_why: Some("<one sentence naming each changed span and the commits: why none can move the numbers>"). ${LANDING}. Now ${head}.`,
      )
    }
  }
}

// `--landing <branch> [--onto <main>]`: the check on the tree landing the branch leaves (a fast-forward when main
// is in its history, else a merge), and each stamp that holds only if the branch lands by fast-forward or merge.
function landing(args: string[]): number {
  const ref = args[args.indexOf("--landing") + 1]
  const onto = args.includes("--onto") ? args[args.indexOf("--onto") + 1] : "origin/main"
  if (!ref || commitOf(ref) === null || commitOf(onto) === null) {
    console.error(`--landing <branch> [--onto <main>]: ${ref ?? "no branch"} or ${onto} is no commit (git fetch first)`)
    return 2
  }
  let tree: Tree
  if (isAncestor(onto, ref)) {
    const id = commitOf(ref)!
    tree = { what: `${ref} fast-forwarded onto ${onto}`, read: cached((p) => fileAt(id, p)), tips: [ref] }
  } else {
    const merged = Bun.spawnSync(["git", "merge-tree", "--write-tree", onto, ref], { cwd: root, stdout: "pipe", stderr: "pipe" })
    const id = merged.stdout.toString().split("\n")[0].trim()
    if (merged.exitCode !== 0 || !/^[0-9a-f]{40}$/.test(id)) {
      console.error(`${ref} does not merge into ${onto} cleanly:\n${merged.stdout.toString()}${merged.stderr.toString()}`)
      return 1
    }
    tree = { what: `${ref} merged into ${onto}`, read: cached((p) => git(["show", `${id}:${p}`])), tips: [onto, ref] }
  }
  const text = tree.read("src/support.rs")
  if (text === null) {
    console.error(`${tree.what} has no src/support.rs`)
    return 1
  }
  const s = parseSupport(text)
  const problems: string[] = [...s.evidenceProblems]
  watchChecks(s, (m) => problems.push(m), tree)
  rotProblems(s, (m) => problems.push(m), undefined, tree)
  const branchOnly = new Set<string>()
  for (const r of s.rows) for (const os of OSES) {
    const c = r[os]
    if (c.kind !== "verified") continue
    for (const rev of [c.evidence.version, c.evidence.reviewed_at]) if (rev && commitOf(rev) !== null && !isAncestor(rev, onto)) branchOnly.add(rev)
  }
  for (const m of problems) console.error(`FAIL ${m}`)
  if (problems.length) {
    console.log(`landing ${ref} onto ${onto}: ${problems.length} failures; main would fail support-matrix --check`)
    return 1
  }
  console.log(
    branchOnly.size
      ? `landing ${ref} onto ${onto}: every stamp holds if it lands by ${isAncestor(onto, ref) ? "fast-forward" : "merge commit"}; ${[...branchOnly].join(", ")} ${branchOnly.size > 1 ? "are" : "is"} only on the branch, so a squash, cherry-pick or rebase would orphan ${branchOnly.size > 1 ? "them" : "it"}`
      : `landing ${ref} onto ${onto}: every stamp is on ${onto} already; any landing holds`,
  )
  return 0
}

// WATCH itself: every cell it names is a row (or a runtime's), every span names code in the tree checked (the working tree, or a landing's), a
// group gives its why.
function watchChecks(s: Support, fail: (m: string) => void, tree: Tree = WORKING) {
  for (const g of WATCH) {
    if (!g.why.trim()) fail(`WATCH ${g.spans[0]}: a group says why its spans are the ones`)
    for (const c of g.cells) {
      const [rt, cap] = c.split(" · ")
      if (c !== "*" && !(s.runtimes.includes(rt) && (cap === "*" || s.rows.some((r) => r.runtime === rt && r.capability === cap)))) fail(`WATCH names ${c}, which is no row of SUPPORT_MATRIX`)
    }
    for (const spec of g.spans) if (spanIn(tree.read, spec) === null) fail(`WATCH: ${spec} names no code in ${tree.what} (renamed or moved? map the code that computes it now)`)
  }
}

// The span machinery on text it must judge: a harmless reformat of a watched fn holds, a changed constant inside
// it fails and names it, a change outside every span holds.
function rotSelfChecks(fail: (m: string) => void) {
  const before = `
impl Owned {
    /// Reads the cycles.
    pub fn read(&self) -> u64 {
        let rate = 10; // per tick
        self.cycles(rate, Some(2),)
    }
}
fn other() -> u32 { 1 }
#[cfg(windows)]
fn per_os() -> u32 { 1 }
#[cfg(unix)]
fn per_os() -> u32 { 2 }
`
  const reformatted = `
impl Owned {
    pub fn read(&self) -> u64 {
        // a new comment, and rustfmt's own layout
        let rate = 10;
        self.cycles(
            rate,
            Some(2),
        )
    }
}
fn other() -> u32 {
    1
}
#[cfg(windows)]
fn per_os() -> u32 { 1 }
#[cfg(unix)]
fn per_os() -> u32 { 2 }
`
  const constant = before.replace("let rate = 10;", "let rate = 11;")
  const outside = before.replace("fn other() -> u32 { 1 }", "fn other() -> u32 { 7 }")
  const per = before.replace("fn per_os() -> u32 { 2 }", "fn per_os() -> u32 { 3 }")
  const same = (a: string, b: string, sel: string) => (spanTexts(a, sel, false) ?? ["missing"]).join(" ¦ ") === (spanTexts(b, sel, false) ?? ["missing a"]).join(" ¦ ")
  if (!same(before, reformatted, "Owned::read")) fail("rot: a reformat of a watched fn (comments, line breaks, a trailing comma) reads as a change")
  if (same(before, constant, "Owned::read")) fail("rot: a changed constant inside a watched fn reads as no change")
  if (!same(before, outside, "Owned::read")) fail("rot: a change outside every watched span reads as a change")
  if (same(before, per, "per_os")) fail("rot: a change to one OS's copy of a watched fn reads as no change")
  if (spanTexts(before, "nothing", false) !== null) fail("rot: a span naming no item found one")
  for (const [spec, want] of [["src/host/spawn.rs#Owned::run_to_exit", "run_to_exit"], ["src/app/bench/block.rs#impl Hooks", "Hooks"], ["sdk/bench/src/host.mjs#@try {", "host"], ["src/host/jobs.rs", "jobs"]])
    if (spanName(spec) !== want) fail(`rot: a review names ${spec} as ${spanName(spec)}, not ${want}`)
  const cfg = before.replace("#[cfg(unix)]\nfn per_os", '#[cfg(target_os = "linux")]\nfn per_os')
  const swapped = before.replace("#[cfg(windows)]\nfn per_os() -> u32 { 1 }\n#[cfg(unix)]", "#[cfg(unix)]\nfn per_os() -> u32 { 1 }\n#[cfg(windows)]")
  const visible = before.replace("    pub fn read(", "    pub(crate) fn read(")
  if (same(before, cfg, "per_os")) fail("rot: a changed #[cfg(…)] on a watched fn reads as no change")
  if (same(before, swapped, "per_os")) fail("rot: two copies of a watched fn with their #[cfg(…)]s swapped read as no change")
  if (same(before, visible, "Owned::read")) fail("rot: a watched fn's changed visibility reads as no change")
  if (!same(before, before.replace("fn other() -> u32 { 1 }", "#[inline]\nfn other() -> u32 { 1 }"), "per_os")) fail("rot: an attribute on the item before a watched fn reads as the watched fn's")
  const js = "const a = 1\ntry {\n  const v = await fn(...args) // the call\n  write({ id, ok: v })\n} catch (e) {}\n"
  const jsLater = "const a = 2\ntry {\n  const v = await fn(\n    ...args,\n  )\n  write({ id, ok: v })\n} catch (e) {}\n"
  if ((spanTexts(js, "@try {", true) ?? []).join() !== (spanTexts(jsLater, "@try {", true) ?? ["x"]).join()) fail("rot: a JavaScript block reformatted reads as a change")
  const re = 'const r = /["`\']\\/[}]/g\nconst run = { a() { return 1 } }\n'
  if ((spanTexts(re, "@const run =", true) ?? []).join() !== "const run = { a ( ) { return 1 } }") fail("rot: a JavaScript regular expression with quotes and braces in it is read as code")
}

// ---------- main ----------

if (import.meta.main) {
  const args = process.argv.slice(2)
  if (args.includes("--landing")) process.exit(landing(args))
  if (args.includes("--check")) {
    const problems: string[] = []
    const fail = (m: string) => problems.push(m)
    let s: Support | undefined
    try {
      s = parseSupport(source())
    } catch (e) {
      fail(`parse: ${(e as Error).message}`)
    }
    parserChecks(fail)
    rotSelfChecks(fail)
    if (s) {
      staticChecks(s, fail)
      watchChecks(s, fail)
      rotProblems(s, fail, args.includes("--rot") ? (m) => console.log(m) : undefined)
      if (!args.includes("--no-rust")) rustChecks(s, fail)
    }
    for (const m of problems) console.error(`FAIL ${m}`)
    console.log(problems.length ? `support-matrix --check: ${problems.length} failures` : `support-matrix --check: ${s!.rows.length} rows, ${s!.hosts.length} hosts, every invariant holds`)
    process.exit(problems.length ? 1 : 0)
  }
  const json = args.includes("--json")
  const at = args.indexOf("--host")
  const arch = process.arch === "arm64" ? "arm64" : "x64"
  const plat = process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : "linux"
  const host = at >= 0 ? args[at + 1] : `${plat}-${arch}`
  const s = parseSupport(source())
  if (!s.hosts.includes(host)) {
    console.error(`--host is one of ${s.hosts.join(", ")}, not ${host}`)
    process.exit(2)
  }
  const os = osOf(host)
  const grid = s.runtimes.flatMap((runtime) =>
    s.capabilities.map((capability) => {
      const row = s.rows.find((r) => r.runtime === runtime && r.capability === capability)
      if (!row) return { runtime, capability, label: "not for target", detail: "strobe measures no metric of it for this kind of target (NotForTarget)" }
      return { runtime, capability, ...onHost(row[os], host) }
    }),
  )
  const modifiers = { os_input: onHost(s.osInput[os], host), outside_page: onHost(s.outsidePage[os], host) }
  const targets = Object.fromEntries(s.runtimes.map((r) => [r, rollup(s, r, host)]))
  if (json) {
    console.log(JSON.stringify({ host, cells: grid, modifiers, targets }, null, 2))
  } else {
    const count = (l: string) => grid.filter((g) => g.label === l).length
    console.log(`strobe support on ${host}: ${count("verified")} verified · ${count("preview")} preview · ${count("refused")} refused on this OS · ${count("not for target")} not for target`)
    if (!PRIMARY.has(host)) console.log(`${host} is no primary architecture: every path strobe runs here is preview`)
    for (const runtime of s.runtimes) {
      console.log(`\n${runtime}  (summary: ${targets[runtime] ?? "refused here"})`)
      for (const g of grid.filter((g) => g.runtime === runtime && g.label !== "not for target")) console.log(`  ${g.capability.padEnd(18)} ${g.label.padEnd(9)} ${g.detail}`)
      const nft = grid.filter((g) => g.runtime === runtime && g.label === "not for target").map((g) => g.capability)
      if (nft.length) console.log(`  ${"(not for target)".padEnd(28)} ${nft.join(", ")}`)
    }
    console.log(`\nOS input (a side driven through the OS)  ${modifiers.os_input.label}  ${modifiers.os_input.detail}`)
    console.log(`a page measured from outside            ${modifiers.outside_page.label}  ${modifiers.outside_page.detail}`)
  }
}
