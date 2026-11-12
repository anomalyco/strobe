// Every command `strobe query <exp> insights` prints must run (dev, not shipped). For each experiment given (and a set
// of synthetic results built here for the forms that are hard to make for real: `--mobile`, a `strobe run`, params with
// a comma and a `$`, a case name with `*`, a path with spaces, leak runs with no heap snapshot, engines strobe cannot
// profile, an implementation check, several targets), it takes each printed command (`next` and every hit's handles)
// and passes it through `sh` and PowerShell as a person would paste it, into a program that prints the argv each shell
// produced. Each argv must then:
//   - parse with the real binary's parser: `strobe <argv> --help` (Args::parse runs before `--help` is read, and refuses
//     an unknown flag or a value a flag cannot take, exit 2), which starts no bench;
//   - name only flags its command reads (`cli::FLAGS`, read from src/cli.rs, which the binary was built from; what
//     `Args::check_command` refuses at run time) and the positional words its command takes;
//   - name recordings and experiments that exist (`rec_…`, `exp_…`), unless a synthetic result names a missing one.
// There is no dry run of a bench, so this is the cheapest honest stand-in: it does not prove a bench would succeed,
// only that strobe would start it as written.
//
//   bun scripts/dev/next-commands.ts [--strobe <exe>] [exp_… …]
//
// Exit 1 when a command fails.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "fs"
import { homedir, platform, tmpdir } from "os"
import { join } from "path"

const argv = process.argv.slice(2)
const opt = (n: string) => (argv.includes(`--${n}`) ? argv[argv.indexOf(`--${n}`) + 1] : undefined)
const STROBE = opt("strobe") ?? "strobe"
const ids = argv.filter((a, i) => !a.startsWith("--") && argv[i - 1] !== "--strobe")
const win = platform() === "win32"
const realState = win ? join(process.env.LOCALAPPDATA ?? join(homedir(), "AppData/Local"), "strobe") : platform() === "darwin" ? join(homedir(), "Library/Application Support/strobe") : join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local/share"), "strobe")

// cli::FLAGS from the source: name → (kind, commands).
const cliSrc = readFileSync(new URL("../../src/cli.rs", import.meta.url), "utf8")
const FLAGS = new Map<string, { kind: string; commands: string[] }>()
for (const m of cliSrc.matchAll(/\("([a-z0-9-]+)", (Switch|Value|Number|Optional), "([^"]*)"\)/g)) FLAGS.set(m[1], { kind: m[2], commands: m[3].split(" ").filter(Boolean) })

// A program that prints its argv as JSON: what a shell handed it.
const work = mkdtempSync(join(tmpdir(), "strobe-next-"))
const echo = join(work, "argv.js")
writeFileSync(echo, "console.log(JSON.stringify(process.argv.slice(2)))\n")
const bun = process.execPath
const sh = win ? ["C:\\Program Files\\Git\\bin\\sh.exe"].find(existsSync) : "/bin/sh"
const pwsh = Bun.which("pwsh") ?? (win ? Bun.which("powershell") : null)

function argvVia(shell: "sh" | "pwsh", rest: string): string[] | string {
  const p =
    shell === "sh"
      ? Bun.spawnSync([sh!, "-c", `"$0" "$1" ${rest}`, bun.replace(/\\/g, "/"), echo.replace(/\\/g, "/")], { stdout: "pipe", stderr: "pipe", env: { ...process.env, MSYS_NO_PATHCONV: "1" } })
      : Bun.spawnSync([pwsh!, "-NoProfile", "-NonInteractive", "-Command", `& '${bun}' '${echo}' ${rest}`], { stdout: "pipe", stderr: "pipe" })
  if (p.exitCode !== 0) return `${shell} could not run it: ${p.stderr.toString().trim().split("\n")[0]}`
  try {
    return JSON.parse(p.stdout.toString())
  } catch {
    return `${shell} printed no argv: ${p.stdout.toString().trim()}`
  }
}

// What a command must look like after its flags are taken out: its positional words.
function positional(cmd: string, words: string[], flags: Map<string, string>): string | undefined {
  const n = words.length
  switch (cmd) {
    case "bench":
      return flags.has("resume") || flags.has("stop") ? (n === 0 ? undefined : `bench --resume takes no file, got ${n}`) : n === 1 ? undefined : `bench takes one file, got ${n}: ${JSON.stringify(words)}`
    case "record":
      return flags.has("bench") ? (n === 0 ? undefined : `record --bench takes no target, got ${JSON.stringify(words)}`) : n === 1 ? undefined : `record takes one target, got ${n}`
    case "run":
      return n === 0 ? undefined : `run --base --head takes no words, got ${JSON.stringify(words)}`
    case "query":
      return n >= 2 ? undefined : `query takes a handle and a view, got ${JSON.stringify(words)}`
    case "report":
    case "open":
      return n >= 1 && n <= 2 ? undefined : `${cmd} takes a handle, got ${JSON.stringify(words)}`
    default:
      return `strobe ${cmd}: not a command insights should print`
  }
}

function check(a: string[], state: string, missingOk: Set<string>): string | undefined {
  const [cmd, ...rest] = a
  const flags = new Map<string, string>()
  const words: string[] = []
  for (let i = 0; i < rest.length; i++) {
    const t = rest[i]
    if (!t.startsWith("--")) {
      words.push(t)
      continue
    }
    const [name, eq] = t.slice(2).split(/=(.*)/s)
    const f = FLAGS.get(name)
    if (!f) return `unknown flag --${name}`
    if (!f.commands.includes("*") && !f.commands.includes(cmd)) return `--${name} is not a flag of strobe ${cmd}`
    if (f.kind === "Switch" || f.kind === "Optional") flags.set(name, eq ?? "")
    else if (eq !== undefined) flags.set(name, eq)
    else flags.set(name, rest[++i] ?? "")
  }
  const bad = positional(cmd, words, flags)
  if (bad) return bad
  for (const h of [...words, ...flags.values()].flatMap((w) => w.match(/\b(?:rec|exp)_[0-9a-z_]+/g) ?? [])) {
    const kind = h.startsWith("rec_") ? "recordings" : "experiments"
    if (!missingOk.has(h) && !existsSync(join(state, kind, h.split("..")[0]))) return `${h} does not exist`
  }
  const file = cmd === "bench" ? words[0] : flags.get("bench")?.split("#")[0]
  if (file && !existsSync(file)) return `no file ${file}`
  // The real parser: refuses what it cannot read, before `--help` stops it.
  const p = Bun.spawnSync([STROBE, ...a, "--help"], { stdout: "pipe", stderr: "pipe" })
  if (p.exitCode !== 0) return `strobe refused it: ${p.stderr.toString().trim().split("\n")[0]}`
}

type J = any
function commandsOf(out: J): string[] {
  const all: string[] = []
  for (const s of out.series ?? []) {
    for (const n of s.next ?? []) if (n.command) all.push(n.command)
    for (const g of s.suspects ?? []) for (const h of g.hits ?? []) for (const c of h.handles ?? []) all.push(c)
  }
  for (const n of out.next ?? []) if (n.command) all.push(n.command)
  return [...new Set(all)]
}

function insightsOf(id: string, env: Record<string, string>): J {
  const p = Bun.spawnSync([STROBE, "query", id, "insights", "--json"], { stdout: "pipe", stderr: "pipe", env: { ...process.env, ...env } })
  if (p.exitCode !== 0) throw new Error(`query ${id} insights: exit ${p.exitCode}: ${p.stderr.toString().trim()}`)
  return JSON.parse(p.stdout.toString())
}

// ---------- synthetic results, in a state folder of their own ----------

// strobe's state folder is <LOCALAPPDATA or XDG_DATA_HOME>/strobe: one of this run's own (macOS: not supported here).
const fakeRoot = join(work, "local")
const fakeEnv: Record<string, string> = win ? { LOCALAPPDATA: fakeRoot } : { XDG_DATA_HOME: fakeRoot }
const fakeState = join(fakeRoot, "strobe")
const benchDir = join(work, "my bench $dir")
mkdirSync(benchDir, { recursive: true })
const benchFile = join(benchDir, "app.bench.ts")
writeFileSync(benchFile, "// a placeholder: the commands name it, nothing runs it\n")
const comparison = (s: J, flags: J, extra: J = {}) => ({ kind: "experiment", comparison: "builds", plan: { runs_per_block: 5 }, args: { command: "bench", pos: [benchFile], flags }, file: benchFile, variants: [{ label: "c0", spec: "c0" }, { label: "c1", spec: "c1" }], warnings: [], ...extra, series: s })
const underpowered = { metric: "cpu.work", verdict: "underpowered: ~12 pairs needed", verdict_kind: "underpowered", decided: false, needed_pairs: 12, n: 3, base: 10, head: 10.2, estimate: 0.02, lo: -0.05, hi: 0.08, relative: true }
const worse = { metric: "cpu.work", verdict: "more", verdict_kind: "worse", decided: true, n: 8, base: 10, head: 13, estimate: 0.3, lo: 0.2, hi: 0.4, relative: true, better: "lower" }
const series = (case_: string, target: string, primary: J, extra: J = {}) => ({ kind: "runs", label: `${case_} · ${target}`, case: case_, target, target_kind: "web", engine: "chromium", primary, pairs: primary.n, attribution: {}, warnings: [], ...extra })
const synthetic: [string, J][] = [
  // --mobile bare, params with a comma and a `$`, a case with `*`, several targets, a path with spaces and `$`.
  ["exp_syn_mobile", comparison([series("load*", "phone", underpowered, { params: { a: 1, b: "x$y" } }), series("load*", "desk", worse)], { head: "c1", base: "c0", mobile: "", why: "never", seed: "7" })],
  ["exp_syn_serial", comparison([series("tap", "phone", worse)], { head: "c1", base: "c0", mobile: "emulator-5554", why: "never" })],
  // A `strobe run` of two commands.
  ["exp_syn_run", comparison([{ ...series("run", "command", underpowered), target_kind: "cli" }], { head: "cmd:bun b.ts --n 1", base: "cmd:bun a.ts", metrics: "metric.a,metric.b" }, { file: join(realState, "run", "cmd-abc", "run.bench.ts") })],
  // Engines strobe cannot profile, a decided change, no attribution: no `record --bench`.
  ["exp_syn_webkit", comparison([{ ...series("open", "page", worse), engine: "webkit" }], { head: "c1", base: "c0", engine: "webkit" })],
  ["exp_syn_fxlaunch", comparison([{ ...series("cold", "page", worse), kind: "launch", engine: "firefox" }], { head: "c1", base: "c0", engine: "firefox" })],
  // Leak runs: a native app's host memory, a Firefox page's about:memory (no heap snapshots), an unclear growth.
  ["exp_syn_leak", comparison([{ kind: "leak", label: "grow · app", case: "grow", target: "app", runs_per_half: 5, leak: { base: { recording: "rec_syn_b", native_host: { verdict: "no growth (within ±16 KB/run)", verdict_kind: "same" } }, head: { recording: "rec_syn_h", native_host: { verdict: "leaks +1 MB/run", verdict_kind: "worse", lo: 9e5, hi: 1.1e6 } } } }, { kind: "leak", label: "page · fx", case: "page", target: "fx", runs_per_half: 5, leak: { head: { recording: "rec_syn_f", js_heap: { verdict: "unclear: -1 KB … +40 KB/run; more runs (--repeat) narrow it", verdict_kind: "unclear", lo: -1e3, hi: 4e4 }, suspects: { refused: "Firefox gives strobe no heap snapshot" } } } }], { head: "c1", base: "c0", probes: "memory", repeat: "5" })],
  // An implementation check (no flags kept: its variants), and a failed case with a seed.
  ["exp_syn_impl", { kind: "check", comparison: "implementations", file: benchFile, plan: { seed: 3, used_random: true }, variants: [{ name: "Old", target: "old", spec: "old" }, { name: "New", target: "new", spec: "new" }], warnings: [], series: [{ kind: "check", label: "a · new", case: "a", target: "new", target_kind: "web", engine: "chromium", primary: { metric: "run.duration" }, passed: true, values: { "run.duration": 5 }, attribution: null, recording: null }, { kind: "check", label: "b · new", case: "b", target: "new", target_kind: "web", primary: { metric: "run.duration" }, passed: false, error: "timed out", hint: "raise the timeout" }] }],
  // --blocks without attribution, and with --why always that failed.
  ["exp_syn_blocks", comparison([{ kind: "blocks", label: "b · app", case: "b", target: "app", target_kind: "script", primary: { metric: "cpu.work" }, blocks: [{ values: { "cpu.work": 3 } }, { values: { "cpu.work": 3.2 } }], attribution: {}, timing_recordings: [{ recording: "rec_syn_t", variant: "head" }] }], { head: ".", blocks: "2" })],
  // An attributed comparison whose recordings were pruned.
  ["exp_syn_pruned", comparison([series("load", "page", worse, { attribution: { base: "rec_syn_gone_b", head: "rec_syn_gone_h" } })], { head: "c1", base: "c0" })],
]
const missingOk = new Set(["rec_syn_b", "rec_syn_h", "rec_syn_f", "rec_syn_t", "rec_syn_gone_b", "rec_syn_gone_h"])
for (const [id, r] of synthetic) {
  mkdirSync(join(fakeState, "experiments", id), { recursive: true })
  writeFileSync(join(fakeState, "experiments", id, "result.json"), JSON.stringify({ id, ...r }))
}

// ---------- run ----------

let failed = 0
let checked = 0
const runs: [string, Record<string, string>, string][] = [...ids.map((id) => [id, {}, realState] as [string, Record<string, string>, string]), ...synthetic.map(([id]) => [id, fakeEnv, fakeState] as [string, Record<string, string>, string])]
for (const [id, env, state] of runs) {
  let out: J
  try {
    out = insightsOf(id, env)
  } catch (e) {
    console.log(`FAIL ${id}: ${e}`)
    failed++
    continue
  }
  for (const c of commandsOf(out)) {
    if (!c.startsWith("strobe ")) {
      console.log(`FAIL ${id}: not a strobe command: ${c}`)
      failed++
      continue
    }
    const rest = c.slice("strobe ".length)
    for (const shell of ["sh", "pwsh"] as const) {
      if ((shell === "sh" && !sh) || (shell === "pwsh" && !pwsh)) continue
      checked++
      const a = argvVia(shell, rest)
      const why = typeof a === "string" ? a : check(a, state, missingOk)
      if (why) {
        console.log(`FAIL ${id} [${shell}] ${c}\n     ${why}`)
        failed++
      }
    }
  }
  if (Object.keys(env).length) console.log(`${id}: ${commandsOf(out).length} commands`)
  // `--show`: each synthetic result's insights as text.
  if (argv.includes("--show") && Object.keys(env).length) {
    console.log(Bun.spawnSync([STROBE, "query", id, "insights"], { stdout: "pipe", env: { ...process.env, ...env } }).stdout.toString())
  }
}
rmSync(work, { recursive: true, force: true })
console.log(`${checked} command parses (sh${pwsh ? " and PowerShell" : ""}): ${failed ? `${failed} failed` : "all run as written"}`)
process.exit(failed ? 1 : 0)
