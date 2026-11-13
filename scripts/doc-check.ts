// The docs say what the code does (SPEC 18, SPEC 24): a static lint of the code's catalogs against SPEC.md,
// the skill and the bench SDK. It reads files only (no build, no strobe run) and fails on any drift:
//
//   (a) every `ValidatorId` has a row in SPEC 10.1 and every row is one; an id nothing raises says Reserved
//   (b) every warning id the code makes (`validity::warn` / `validity::info`, the only constructors) and its
//       levels are a row of SPEC 10.2, and every row is one
//   (c) every user-facing `cli::FLAGS` entry appears as `--name` in SPEC.md; every MCP tool's inputs
//       (`app/mcp.rs` TOOLS) are its SPEC 17 row's, and each of its flags is one FLAGS lists for its command
//   (d) the runner's key lists (`KEYS`, `TARGET_KEYS` in sdk/bench/src/runner.ts) are the SDK's option types
//       (sdk/bench/src/index.ts)
//   (e) the skill's lines are at most 110 columns (its frontmatter's one-line description aside)
//   (f) SPEC 21.1's support table is `SUPPORT_MATRIX` (src/support.rs, read by scripts/dev/support-matrix.ts):
//       its rows in order and the two modifiers, each verified cell's evidence line exact
//
//   bun scripts/doc-check.ts
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { type Cell, EVIDENCE_LINE, evidenceLine, parseSupport } from "./dev/support-matrix.ts"

const root = join(import.meta.dir, "..")
const read = (p: string) => readFileSync(join(root, p), "utf8").replace(/\r\n/g, "\n")
const walk = (d: string): string[] =>
  readdirSync(d).flatMap((n) => {
    const f = join(d, n)
    return statSync(f).isDirectory() ? walk(f) : f.endsWith(".rs") ? [f] : []
  })
const sources = walk(join(root, "src")).map((f) => ({ file: relative(root, f).replace(/\\/g, "/"), text: readFileSync(f, "utf8") }))
const spec = read("SPEC.md")
const problems: string[] = []
const fail = (what: string) => problems.push(what)
const diff = (what: string, code: Set<string>, doc: Set<string>, codeName: string, docName: string) => {
  for (const x of code) if (!doc.has(x)) fail(`${what}: ${x} is in ${codeName}, not in ${docName}`)
  for (const x of doc) if (!code.has(x)) fail(`${what}: ${x} is in ${docName}, not in ${codeName}`)
}

// A SPEC section by its heading's number ("10.1"): its lines up to the next heading of its level or above.
function section(num: string): string {
  const lines = spec.split("\n")
  const at = lines.findIndex((l) => new RegExp(`^#{2,3} ${num.replace(".", "\\.")}\\.? `).test(l))
  if (at < 0) {
    fail(`SPEC.md has no section ${num}`)
    return ""
  }
  const level = lines[at].indexOf(" ")
  const end = lines.findIndex((l, i) => i > at && /^#{2,3} /.test(l) && l.indexOf(" ") <= level)
  return lines.slice(at, end < 0 ? undefined : end).join("\n")
}
// A table's rows as their cells (the first column's backticked ids, the rest as text).
const rows = (text: string) =>
  text
    .split("\n")
    .filter((l) => l.startsWith("| `"))
    .map((l) => {
      const cells = l.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim())
      return { ids: [...cells[0].matchAll(/`([A-Za-z][\w.-]*)`/g)].map((m) => m[1]), cells, line: l }
    })

// (a) validators.
const model = read("src/model.rs")
const validators = new Set(
  (model.match(/validators!\(([\s\S]*?)\);/)?.[1] ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean),
)
const v101 = rows(section("10.1"))
diff("validators", validators, new Set(v101.flatMap((r) => r.ids)), "model::ValidatorId", "SPEC 10.1")
// Raised: `ValidatorId::X`, `Error::refused("X", …)`, a refusal line `"X: …"` (`Refusal::from_line` types it),
// or `Refusal::window_hidden` (WindowHidden).
const raisedIds = new Set<string>()
for (const { file, text } of sources) {
  if (file === "src/model.rs") continue
  for (const m of text.matchAll(/ValidatorId::(\w+)|refused\(\s*"(\w+)"|"([A-Z]\w+): /g)) raisedIds.add(m[1] ?? m[2] ?? m[3])
  if (text.includes("Refusal::window_hidden(")) raisedIds.add("WindowHidden")
}
for (const id of validators) {
  const raised = raisedIds.has(id)
  const row = v101.find((r) => r.ids.includes(id))
  if (!raised && row && !/\bReserved\b/.test(row.line)) fail(`validators: nothing raises ${id}; its SPEC 10.1 row must say Reserved`)
  if (raised && row && /\bReserved\b/.test(row.line)) fail(`validators: ${id} is raised (src), yet its SPEC 10.1 row says Reserved`)
}

// (b) warnings. The id is a literal at every call, so this list is the code's; one id may be built from a
// validator (`r.validator.name()`: the refusing validator's own id, said as info; SPEC 10.2's intro).
const DYNAMIC = [/validity::info\(r\.validator\.name\(\),/]
const levels = new Map<string, Set<string>>()
for (const { file, text } of sources) {
  for (const m of text.matchAll(/\bvalidity::(warn|info)\(\s*"([A-Z]\w+)"/g)) {
    if (!levels.has(m[2])) levels.set(m[2], new Set())
    levels.get(m[2])!.add(m[1])
  }
  for (const m of text.matchAll(/\bvalidity::(warn|info)\(\s*(?=[^\s"])([^\n]{0,40})/g)) {
    if (!DYNAMIC.some((d) => d.test(text.slice(m.index!, m.index! + 80)))) fail(`warnings: ${file}: validity::${m[1]}(${m[2]}…) has no literal id`)
  }
  if (file !== "src/validity.rs" && /\bWarning\s*\{\s*id\b/.test(text)) fail(`warnings: ${file} builds a Warning itself; use validity::warn or validity::info`)
  if (file !== "src/validity.rs" && /^use crate::validity::\{[^}]*\b(warn|info)\b/m.test(text)) fail(`warnings: ${file} imports warn/info bare; call validity::warn/info`)
}
const w102 = new Map<string, Set<string>>()
for (const r of rows(section("10.2"))) {
  const lv = new Set((r.cells[1] ?? "").split(",").map((s) => s.trim()).filter((s) => s === "warn" || s === "info"))
  for (const id of r.ids) w102.set(id, lv)
}
diff("warnings", new Set(levels.keys()), new Set(w102.keys()), "validity::warn/info", "SPEC 10.2")
for (const [id, lv] of levels) {
  const doc = w102.get(id)
  if (doc && [...lv].sort().join() !== [...doc].sort().join()) fail(`warnings: ${id} is ${[...lv].sort().join(", ")} in the code, ${[...doc].sort().join(", ")} in SPEC 10.2`)
}

// (c) flags and MCP tools.
const cli = read("src/cli.rs")
const specFlags = new Set([...spec.matchAll(/--([a-z][a-z-]*)/g)].map((m) => m[1]))
let flagCount = 0
// Each flag's commands (`cli::FLAGS`; `*`: every command), for `cli::takes`.
const takers = new Map<string, string[]>()
for (const m of cli.matchAll(/^\s+\("([a-z-]+)",\s*(?:Switch|Value|Number|Optional),\s*"([^"]*)"\)/gm)) {
  flagCount++
  takers.set(m[1], m[2].split(" ").filter(Boolean))
  const users = m[2].split(" ").filter((c) => c && c !== "helper")
  if (users.length && !specFlags.has(m[1])) fail(`flags: --${m[1]} (${m[2]}) appears nowhere in SPEC.md`)
}
const takes = (command: string, flag: string) => takers.get(flag)?.some((c) => c === "*" || c === command) ?? false
if (!flagCount) fail("flags: no cli::FLAGS entry read")
if (!validators.size || !levels.size) fail("catalogs: no validator or warning read")
const mcp = read("src/app/mcp.rs")
const tools = mcp.split(/\n\s*Tool \{/).slice(1)
if (!tools.length) fail("mcp: no tool read")
const s17 = rows(section("17"))
for (const t of tools) {
  const name = t.match(/name: "(\w+)"/)?.[1] ?? "?"
  const part = (key: string) => t.match(new RegExp(`${key}: &\\[([\\s\\S]*?)\\n\\s*\\],?\\n`))?.[1] ?? t.match(new RegExp(`${key}: &\\[([^\\n]*?)\\],`))?.[1] ?? ""
  const names = (s: string) => [...s.matchAll(/\(\s*"([a-z][a-z-]*)",\s*"/g)].map((m) => m[1])
  // A tool's flags go through its command's check (`Args::check_command`): each must be one the command reads.
  for (const f of names(part("flags"))) if (!takes(name, f)) fail(`mcp ${name}: flag ${f} is not one cli::FLAGS lists for ${name}`)
  const code = new Set([...names(part("args")), ...names(part("flags")), ...(/long: true/.test(t) ? ["timeout"] : [])])
  const row = s17.find((r) => r.ids[0] === name)
  if (!row) {
    fail(`mcp: tool ${name} has no SPEC 17 row`)
    continue
  }
  const doc = new Set([...row.cells[1].split(";")[0].matchAll(/`([a-z][a-z-]*)\??`/g)].map((m) => m[1]))
  diff(`mcp ${name}`, code, doc, "app/mcp.rs", "SPEC 17")
}

// (d) the runner's key lists against the SDK's types.
const sdk = read("sdk/bench/src/index.ts")
const runner = read("sdk/bench/src/runner.ts")
function keysOf(name: string, seen = new Set<string>()): Set<string> {
  if (seen.has(name)) return new Set()
  seen.add(name)
  const m = sdk.match(new RegExp(`^(?:export )?interface ${name}\\b[^\\n]*?(?:extends ([\\w<>, ]+?))?\\s*\\{\\n([\\s\\S]*?)^\\}`, "m"))
  if (!m) {
    fail(`sdk: no interface ${name} in index.ts`)
    return new Set()
  }
  const keys = new Set([...m[2].matchAll(/^ {2}(\w+)\??\s*[:(]/gm)].map((k) => k[1]))
  for (const base of (m[1] ?? "").split(",").map((b) => b.trim().replace(/<.*$/, "")).filter(Boolean)) {
    if (base !== "Seeded" && base !== "Options") for (const k of keysOf(base, seen)) keys.add(k)
  }
  return keys
}
const list = (src: string) => [...src.matchAll(/"(\w+)"/g)].map((m) => m[1])
const constant = (name: string) => list(runner.match(new RegExp(`const ${name} = \\[([^\\]]*)\\]`))?.[1] ?? "")
const sideKeys = keysOf("Side")
const union = sdk.match(/export type Target<P, K = any> = Side &\n([\s\S]*?)\n\n/)?.[1] ?? ""
const targetDoc = runner.match(/const TARGET_KEYS[^{]*\{([\s\S]*?)\n\}/)?.[1] ?? ""
const kinds = [...union.matchAll(/\| \(\{ kind: "(\w+)";?([^}]*)\}((?: & \w+(?:<[^>]*>)?)*)\)/g)]
const runnerKinds = [...targetDoc.matchAll(/^\s*(\w+): \[/gm)].map((m) => m[1])
if (!kinds.length || kinds.length !== runnerKinds.length) fail(`runner: index.ts's Target has ${kinds.length} kinds, runner.ts TARGET_KEYS ${runnerKinds.length}`)
for (const m of kinds) {
  const kind = m[1]
  const expected = new Set(["kind", ...sideKeys, ...[...m[2].matchAll(/(\w+)\??:/g)].map((k) => k[1])])
  for (const t of m[3].matchAll(/& (\w+)/g)) for (const k of keysOf(t[1])) expected.add(k)
  const line = targetDoc.match(new RegExp(`^\\s*${kind}: \\[([^\\]]*)\\]`, "m"))?.[1]
  if (line === undefined) {
    fail(`runner: TARGET_KEYS has no ${kind}`)
    continue
  }
  const got = new Set(["kind", ...line.split(",").flatMap((p) => (p.trim() === "...SIDE" ? [...constant("SIDE")] : p.trim() === "...PAGE" ? constant("PAGE") : p.trim() === "...APP" ? constant("APP") : list(p)))])
  diff(`runner target ${kind}`, expected, got, "index.ts", "runner.ts TARGET_KEYS")
}
const runnerKeys = (kind: string) => new Set(list(runner.match(new RegExp(`^\\s*${kind}: \\[([^\\]]*)\\]`, "m"))?.[1] ?? ""))
diff("runner bench", keysOf("Bench"), runnerKeys("bench"), "index.ts Bench", "runner.ts KEYS.bench")
diff("runner case", new Set([...keysOf("RunCase"), ...keysOf("LaunchCase")]), runnerKeys("case"), "index.ts RunCase | LaunchCase", "runner.ts KEYS.case")
diff("runner launch", keysOf("Launch"), runnerKeys("launch"), "index.ts Launch", "runner.ts KEYS.launch")

// (e) the skill's width.
read("skill/SKILL.md")
  .split("\n")
  .forEach((l, i) => {
    if ([...l].length > 110 && !l.startsWith("description: ")) fail(`skill/SKILL.md:${i + 1} is ${[...l].length} columns (at most 110)`)
  })

// (f) SPEC 21.1's table is the support matrix: one row per row in its order, then the two modifiers; a verified
// cell its evidence line exact, a preview `preview`, a refusal `—`.
const support = parseSupport(read("src/support.rs"))
for (const p of support.evidenceProblems) fail(`support: ${p}`)
for (const r of support.rows)
  for (const os of ["windows", "macos", "linux"] as const) {
    const c = r[os]
    if (c.kind === "verified" && !statSync(join(root, c.evidence.fixture), { throwIfNoEntry: false })?.isFile())
      fail(`support: ${r.runtime} · ${r.capability} on ${os} cites ${c.evidence.fixture}, which is no file`)
  }
const supportText = (c: Cell) => (c.kind === "verified" ? evidenceLine(c.evidence) : c.kind === "preview" ? "preview" : "—")
const wantRows = [
  ...support.rows.map((r) => [`\`${r.runtime}\` · \`${r.capability}\``, r.windows, r.macos, r.linux] as const),
  ["OS input (`OS_INPUT`)", support.osInput.windows, support.osInput.macos, support.osInput.linux] as const,
  ["a page measured from outside (`OUTSIDE_PAGE`)", support.outsidePage.windows, support.outsidePage.macos, support.outsidePage.linux] as const,
]
const s211 = section("21.1")
  .split("\n")
  .filter((l) => l.startsWith("| ") && !l.startsWith("| Runtime · capability |"))
  .map((l) => l.split("|").slice(1, -1).map((c) => c.trim()))
if (s211.length !== wantRows.length) fail(`support: SPEC 21.1 has ${s211.length} rows, src/support.rs ${wantRows.length} (SUPPORT_MATRIX's ${support.rows.length} and the 2 modifiers)`)
wantRows.forEach(([label, ...cells], i) => {
  const got = s211[i]
  if (!got) return
  if (got[0] !== label) return fail(`support: SPEC 21.1 row ${i + 1} is ${got[0]}, src/support.rs has ${label} there`)
  ;(["Windows x64", "macOS arm64", "Linux x64"] as const).forEach((os, j) => {
    const want = supportText(cells[j])
    if (got[j + 1] !== want) fail(`support: SPEC 21.1 ${label} on ${os} reads "${got[j + 1]}", src/support.rs "${want}"`)
    if (cells[j].kind === "verified" && !EVIDENCE_LINE.test(want)) fail(`support: ${label} on ${os}: ${want} is no evidence line`)
  })
})

if (problems.length) {
  console.error(problems.join("\n"))
  console.error(`doc-check: ${problems.length} problem${problems.length === 1 ? "" : "s"}`)
  process.exit(1)
}
console.log(
  `doc-check: ${validators.size} validators, ${levels.size} warnings, ${flagCount} flags, ${tools.length} MCP tools, ${kinds.length} target kinds, ${support.rows.length} support rows and the skill agree with the docs`,
)
