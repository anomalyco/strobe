// The verified/preview matrix (`SUPPORT_MATRIX` in src/support.rs, SPEC 21.1), printed for one host or checked.
//
//   bun scripts/dev/support-matrix.ts [--host windows-x64|windows-arm64|macos-arm64|macos-x64|linux-x64|linux-arm64] [--json]
//   bun scripts/dev/support-matrix.ts --check [--no-rust]
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
// metric's capability a capability, the parser on wrapped and escaped Rust. Then (without --no-rust) a small program
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
    for (const f of m[2].matchAll(new RegExp(`(\\w+)\\s*:\\s*${STR}`, "g"))) fields[f[1]] = str(f[2])
    for (const k of EVIDENCE_KEYS) if (!fields[k]?.trim()) evidenceProblems.push(`Evidence ${m[1]}: no ${k}`)
    for (const k of Object.keys(fields)) if (!EVIDENCE_KEYS.includes(k)) evidenceProblems.push(`Evidence ${m[1]}: unknown field ${k}`)
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
  }
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
  ["page from outside, frames", p(rm(one("ChromiumPage"), "frames.dropped.pct", "MacosArm64", th({ outside_page: true }))), `${PRE}capability,outside_page`],
  ["page from outside, page engine untouched", p(rm(one("ChromiumPage"), "main.work", "MacosArm64", th({ outside_page: true }))), `${PRE}capability`],
  ["outside applies to pages only", p(rm(one("NativeWindowless"), "cpu.work", "MacosArm64", th({ outside_page: true }))), `${PRE}capability`],
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
  ["cli memory.peak", p(rm(one("Cli"), "memory.peak", "WindowsX64")), `${PRE}capability`],
  ["native memory.peak", p(rm(one("NativeWindowless"), "memory.peak", "WindowsX64")), "none"],
  ["cli metric.loop_ms", p(rm(one("Cli"), "metric.loop_ms", "LinuxX64")), `${PRE}capability`],
  ["script metric.x", p(rm(one("Script"), "metric.x", "LinuxX64")), "none"],
  ["tty app.x", p(rm(one("Tty"), "app.x", "LinuxX64")), `${PRE}capability`],
  ["native app.x", p(rm(one("Native"), "app.x", "LinuxX64")), "none"],
  ["tty cpu.pty_host windows", p(rm(one("Tty"), "cpu.pty_host", "WindowsX64")), `${PRE}capability`],
  ["tty cpu.pty_host linux", p(rm(one("Tty"), "cpu.pty_host", "LinuxX64")), "none"],
  ["cli cli.launch", p(rm(one("Cli"), "cli.launch", "WindowsX64")), `${PRE}capability`],
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
  ["comparison", `resolve_comparison(${one("Cli")}, "cpu.work", Host::WindowsX64, ${T}, false).map(Support::name).unwrap_or("none").to_string()`, "preview"],
  ["target_support cli windows", `target_support(Runtime::Cli, Host::WindowsX64).map(Support::name).unwrap_or("none").to_string()`, "preview"],
  ["target_support webview macos", `target_support(Runtime::WebviewApp, Host::MacosArm64).map(Support::name).unwrap_or("none").to_string()`, "none"],
  // Phrases, as the surfaces will print them.
  ["phrase windowless cpu", phrase(`resolve(${one("NativeWindowless")}, Capability::ProcessCpu, Host::MacosArm64, ${T})`), "preview (windowless native · process CPU on macOS arm64: runtime checks held; not yet checked against a known-answer fixture)"],
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
    "preview (chromium page · frames (page measured from outside, OS input) on macOS arm64: runtime checks held; not yet checked against a known-answer fixture; a page measured from outside (the host's clock, the OS's frames) is not yet checked against a known answer; input through the OS is not yet checked against a known answer)",
  ],
  ["phrase verified", built(`tier: Support::Verified, runtime: Runtime::Cli, capability: Capability::ProcessCpu, host: Host::WindowsX64, preview_by: vec![]`), "verified (cli · process CPU on Windows x64)"],
  [
    "phrase secondary architecture",
    built(`tier: Support::Preview, runtime: Runtime::Cli, capability: Capability::ProcessCpu, host: Host::WindowsArm64, preview_by: vec![Cause::Architecture]`),
    "preview (cli · process CPU on Windows arm64: runtime checks held; verified on Windows x64 only)",
  ],
  ["status windows", `status_summary(Host::WindowsX64).line()`, "every path is preview on Windows x64: none is checked against a known answer here yet"],
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
  "input.rate": { only: [...PAGES_APPS, "android_chrome", ...WINDOWED] },
  "input.gap.p95": { only: [...PAGES_APPS, "android_chrome", ...WINDOWED] },
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

// ---------- main ----------

if (import.meta.main) {
  const args = process.argv.slice(2)
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
    if (s) {
      staticChecks(s, fail)
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
