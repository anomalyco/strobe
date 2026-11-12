// Prints the verified/preview matrix (`SUPPORT_MATRIX` in src/support.rs, SPEC 21.1) for one host: every
// runtime × capability, its label there and the evidence or what is missing, and which combinations strobe
// refuses (on that OS, or for that kind of target). It reads the source, as scripts/doc-check.ts does, so it
// needs no build. Off the OS's primary architecture (Windows and Linux on arm64, Intel Macs) every cell
// verified on that OS reads preview.
//
//   bun scripts/dev/support-matrix.ts [--host windows-x64|windows-arm64|macos-arm64|macos-x64|linux-x64|linux-arm64] [--json]
//
// Without --host: this machine's. `parseSupport` is what doc-check can import to compare SPEC 21.1's table.
import { readFileSync } from "node:fs"
import { join } from "node:path"

export type Cell = { kind: "verified"; evidence: Record<string, string> } | { kind: "preview"; why: string } | { kind: "refused"; why: string }
export type Row = { runtime: string; capability: string; windows: Cell; macos: Cell; linux: Cell }
export type Support = { runtimes: string[]; capabilities: string[]; rows: Row[]; osInput: Record<"windows" | "macos" | "linux", Cell> }

const STR = `"((?:[^"\\\\]|\\\\.)*)"`
const unquote = (s: string) => JSON.parse(`"${s}"`) as string
// `ChromiumPage` → `chromium_page`, as serde's snake_case names it.
const snake = (s: string) => s.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase()

export function parseSupport(text: string): Support {
  text = text.replace(/\r\n/g, "\n")
  const strs = new Map<string, string>()
  for (const m of text.matchAll(new RegExp(`const (\\w+): &str =\\s*${STR};`, "g"))) strs.set(m[1], unquote(m[2]))
  const evidence = new Map<string, Record<string, string>>()
  for (const m of text.matchAll(/const (\w+): Evidence = Evidence \{([\s\S]*?)\};/g)) {
    const fields: Record<string, string> = {}
    for (const f of m[2].matchAll(new RegExp(`(\\w+): ${STR}`, "g"))) fields[f[1]] = unquote(f[2])
    evidence.set(m[1], fields)
  }
  const text_of = (arg: string, where: string) => {
    if (arg.startsWith('"')) return unquote(arg.slice(1, -1))
    const s = strs.get(arg)
    if (s === undefined) throw new Error(`${where}: no const ${arg}`)
    return s
  }
  const cell = (src: string, where: string): Cell => {
    const m = src.match(/^(Verified|Preview|Refused)\((.*)\)$/s)
    if (!m) throw new Error(`${where}: not a cell: ${src}`)
    if (m[1] === "Verified") {
      const ev = evidence.get(m[2].replace(/^&/, ""))
      if (!ev) throw new Error(`${where}: no Evidence ${m[2]}`)
      return { kind: "verified", evidence: ev }
    }
    return { kind: m[1] === "Preview" ? "preview" : "refused", why: text_of(m[2].trim(), where) }
  }
  const CELL = `((?:Verified\\(&\\w+\\))|(?:(?:Preview|Refused)\\((?:\\w+|${STR})\\)))`
  const all = (ty: string) =>
    [...(text.match(new RegExp(`pub const ALL: \\[${ty}; \\d+\\] = \\[([\\s\\S]*?)\\];`))?.[1] ?? "").matchAll(new RegExp(`${ty}::(\\w+)`, "g"))].map((m) => snake(m[1]))
  const matrix = text.match(/pub const SUPPORT_MATRIX: &\[Row\] = &\[([\s\S]*?)\n\];/)?.[1]
  if (!matrix) throw new Error("src/support.rs: no SUPPORT_MATRIX")
  const rowRe = new RegExp(
    `Row \\{\\s*runtime: Runtime::(\\w+),\\s*capability: Capability::(\\w+),\\s*windows: ${CELL},\\s*macos: ${CELL},\\s*linux: ${CELL},?\\s*\\}`,
    "g",
  )
  const rows: Row[] = []
  for (const m of matrix.matchAll(rowRe)) {
    const [runtime, capability] = [snake(m[1]), snake(m[2])]
    const where = `${runtime} · ${capability}`
    rows.push({ runtime, capability, windows: cell(m[3], where), macos: cell(m[5], where), linux: cell(m[7], where) })
  }
  const written = (matrix.match(/Row \{/g) ?? []).length
  if (written !== rows.length) throw new Error(`src/support.rs: ${written} rows written, ${rows.length} read`)
  const os = text.match(new RegExp(`pub const OS_INPUT: Modifier = Modifier \\{\\s*windows: ${CELL},\\s*macos: ${CELL},\\s*linux: ${CELL},?\\s*\\};`))
  if (!os) throw new Error("src/support.rs: no OS_INPUT")
  return {
    runtimes: all("Runtime"),
    capabilities: all("Capability"),
    rows,
    osInput: { windows: cell(os[1], "OS_INPUT"), macos: cell(os[3], "OS_INPUT"), linux: cell(os[5], "OS_INPUT") },
  }
}

const HOSTS = ["windows-x64", "windows-arm64", "macos-arm64", "macos-x64", "linux-x64", "linux-arm64"]
const PRIMARY = new Set(["windows-x64", "macos-arm64", "linux-x64"])
const osOf = (host: string) => host.split("-")[0] as "windows" | "macos" | "linux"

// A cell on a host, as `support::support` resolves it: verified only on the OS's primary architecture.
export function onHost(c: Cell, host: string): { label: "verified" | "preview" | "refused"; detail: string } {
  if (c.kind === "refused") return { label: "refused", detail: c.why }
  if (c.kind === "preview") return { label: "preview", detail: c.why }
  const e = c.evidence
  const line = `verified (${e.date}, ${e.version}, ${e.machine}, \`${e.fixture}\`, ${e.known})`
  if (!PRIMARY.has(host)) return { label: "preview", detail: `verified on the OS's primary architecture only: ${line}` }
  return { label: "verified", detail: `${line}; A/A ${e.aa}; planted ${e.planted}; boundary ${e.boundary}` }
}

if (import.meta.main) {
  const args = process.argv.slice(2)
  const json = args.includes("--json")
  const at = args.indexOf("--host")
  const arch = process.arch === "arm64" ? "arm64" : "x64"
  const plat = process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : "linux"
  const host = at >= 0 ? args[at + 1] : `${plat}-${arch}`
  if (!HOSTS.includes(host)) {
    console.error(`--host is one of ${HOSTS.join(", ")}, not ${host}`)
    process.exit(2)
  }
  const s = parseSupport(readFileSync(join(import.meta.dir, "..", "..", "src", "support.rs"), "utf8"))
  const os = osOf(host)
  const grid = s.runtimes.flatMap((runtime) =>
    s.capabilities.map((capability) => {
      const row = s.rows.find((r) => r.runtime === runtime && r.capability === capability)
      if (!row) return { runtime, capability, label: "not for target", detail: "strobe measures no metric of it for this kind of target (NotForTarget)" }
      return { runtime, capability, ...onHost(row[os], host) }
    }),
  )
  const input = onHost(s.osInput[os], host)
  if (json) {
    console.log(JSON.stringify({ host, cells: grid, os_input: input }, null, 2))
  } else {
    const count = (l: string) => grid.filter((g) => g.label === l).length
    console.log(`strobe support on ${host}: ${count("verified")} verified · ${count("preview")} preview · ${count("refused")} refused on this OS · ${count("not for target")} not for target`)
    if (!PRIMARY.has(host)) console.log(`${host} is no primary architecture: every path strobe runs here is preview`)
    for (const runtime of s.runtimes) {
      console.log(`\n${runtime}`)
      for (const g of grid.filter((g) => g.runtime === runtime && g.label !== "not for target")) console.log(`  ${g.capability.padEnd(18)} ${g.label.padEnd(9)} ${g.detail}`)
      const nft = grid.filter((g) => g.runtime === runtime && g.label === "not for target").map((g) => g.capability)
      if (nft.length) console.log(`  ${"(not for target)".padEnd(28)} ${nft.join(", ")}`)
    }
    console.log(`\nOS input (a side driven through the OS)  ${input.label}  ${input.detail}`)
  }
}
