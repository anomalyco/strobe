// Every command `strobe query <exp> insights` prints must run (dev, not shipped). For each experiment given (and a set
// of synthetic results built here for the forms that are hard to make for real: `--mobile`, a `strobe run`, params with
// a comma and a `$`, a case name with `*`, a path with spaces, leak runs with no heap snapshot, engines strobe cannot
// profile, a native comparison with off-CPU waits, several targets), it takes each printed command (`next` and every hit's handles)
// and passes it through `sh` and PowerShell as a person would paste it, into a program that prints the argv each shell
// produced. Each argv must then:
//   - parse with the real binary's parser: `strobe <argv> --help` (Args::parse runs before `--help` is read, and refuses
//     an unknown flag or a value a flag cannot take, exit 2), which starts no bench;
//   - name only flags its command reads (`cli::FLAGS`, read from src/cli.rs, which the binary was built from; what
//     `Args::check_command` refuses at run time) and the positional words its command takes;
//   - name recordings and experiments that exist (`rec_…`, `exp_…`), unless a synthetic result names a missing one.
// There is no dry run of a bench, so this is the cheapest honest stand-in: it does not prove a bench would succeed,
// only that strobe would start it as written. Then each command of a series must still be about that series and
// measure what it measured:
//   - `record --bench <file>#<case>` names the series' case, its target (a two-target check: the target that case ran,
//     never the other side's) and its own params; a bench rerun with `--filter` names its case and its own params,
//     whatever the experiment swept (`--params all`);
//   - every flag that chose how the experiment ran or what it measured (`--runs`, `--pin`, `--power`, `--engine`,
//     `--headless`, `--mobile`, `--seed` when the hooks drew on it, `--probes waits` for a profile) stays, with its
//     value, wherever the printed command takes it.
//   - a `record` names a build a side ran (`--head <folder or ref>`, a target side's variant, `.` without one);
//   - a `bench` rerun of one series runs that series' target alone (`--target <its target>`), or none where the
//     sides name the targets;
//   - every series is checked against its own commands (one command printed under two series fails on the one
//     it is not about);
//   - no command names a recording, experiment, bench file or folder that is not here (a synthetic result from
//     another machine, pruned recordings), and one read from its path names that path, never the id strobe
//     keeps for another result; it keeps its own ledger;
//   - a word no spelling passes to both shells (a `'` beside `$`) is quoted for this OS's shell and said so
//     (`quoted_for`): only that shell's reading is checked;
//   - no word zsh reads differently is left bare: one starting with `=` (EQUALS) or holding a `^` (extendedglob);
//   - what a check's insights must say: base's error and its hint once when base failed (the series gives the same
//     hint as its own), a `--noworse` failure once;
//   - a hint outside insights (`query <folder> threads` of a recording with no trace) names a recording read by its
//     path by that path, quoted alike, never its id.
// `--make` first builds scripts/dev/insights-repo.ts's repository in a folder named `O'Connor repo` (a `'` and a space
// in every path) and runs cheap checks there, in a state folder of this run's own, for the shapes a real result has
// (`MAKE` below: two targets of one case on one build named by its folder or a ref, two targets checked against each
// other, one target's variant, a param swept with `--params all`, a seeded workload with `--runs`, `--pin` and
// `--power`, attribution blocks of a script and of a page, a failed case, `strobe run --check` of one command and of
// two); a few seconds each, each takes strobe's bench slot. Each made result is then also read by its path, from a copy
// strobe does not keep (`query <path>/result.json insights`), and one of its recordings from a copied folder
// (`query <folder> insights`): their commands name those paths.
//
//   bun scripts/dev/next-commands.ts [--strobe <exe>] [--make] [--show] [exp_… …]
//
// Exit 1 when a command fails.
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "fs"
import { homedir, platform, tmpdir } from "os"
import { join } from "path"
import { fileURLToPath } from "url"
import { gzipSync } from "zlib"

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

// What zsh, a third shell people paste into, reads differently from sh in a word left bare: a leading `=`
// (`=word` is the program word's path, EQUALS) and a `^` (`extendedglob` negation, `HEAD^`). Quoted words pass.
function zshReads(c: string): string | undefined {
  let quote: string | null = null
  let start = true
  for (const ch of c) {
    if (quote) {
      if (ch === quote) quote = null
      continue
    }
    if (ch === "'" || ch === '"') {
      quote = ch
      start = false
      continue
    }
    if (ch === " ") {
      start = true
      continue
    }
    if (ch === "^") return "a bare ^, which zsh's extendedglob reads as a negation"
    if (ch === "=" && start) return "a bare word starting with =, which zsh expands to a program's path"
    start = false
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
  for (const h of [...words, ...flags.values()].filter((w) => !/[\\/]/.test(w)).flatMap((w) => w.match(/\b(?:rec|exp)_[0-9a-z_]+/g) ?? [])) {
    const kind = h.startsWith("rec_") ? "recordings" : "experiments"
    if (!missingOk.has(h) && !existsSync(join(state, kind, h.split("..")[0]))) return `${h} does not exist`
  }
  // A handle given as a path (a result or a recording strobe does not keep) names one that is there.
  if ((cmd === "query" || cmd === "report") && words.length) {
    const handle = cmd === "report" && ["experiment", "recording"].includes(words[0]) ? words[1] : words[0]
    if (handle && /[\\/]/.test(handle) && !existsSync(handle)) return `no file or folder ${handle}`
  }
  const file = cmd === "bench" ? words[0] : flags.get("bench")?.split("#")[0]
  if (file && !existsSync(file)) return `no file ${file}`
  // The real parser: refuses what it cannot read, before `--help` stops it.
  const p = Bun.spawnSync([STROBE, ...a, "--help"], { stdout: "pipe", stderr: "pipe" })
  if (p.exitCode !== 0) return `strobe refused it: ${p.stderr.toString().trim().split("\n")[0]}`
}

type J = any
// Each command with the result's series it was printed under (null: the experiment's own): the same
// command under two series is checked against each of them (two series that print one command, where each
// should have its own, fail on one), once per series.
function commandsOf(out: J): [string, number | null][] {
  const all: [string, number | null][] = []
  const add = (c: string, i: number | null) => {
    if (!all.some(([x, j]) => x === c && j === i)) all.push([c, i])
  }
  for (const [i, s] of (out.series ?? []).entries()) {
    for (const n of s.next ?? []) if (n.command) add(n.command, i)
    for (const g of s.suspects ?? []) for (const h of g.hits ?? []) for (const c of h.handles ?? []) add(c, i)
  }
  for (const n of out.next ?? []) if (n.command) add(n.command, null)
  // A recording's insights: each hit's handles.
  for (const h of out.insights ?? []) for (const c of h.handles ?? []) add(c, null)
  return all
}

// What a rerun or a profile must keep of its series and its experiment (argv as a shell gave it).
const MEASURED = ["runs", "pin", "power", "engine", "headless", "mobile", "wayland", "style"]
function semantic(a: string[], s: J, r: J): string | undefined {
  const [cmd, ...rest] = a
  if (cmd !== "bench" && cmd !== "record") return
  const flags = new Map<string, string>()
  for (let i = 0; i < rest.length; i++) {
    const t = rest[i]
    if (!t.startsWith("--")) continue
    const [name, eq] = t.slice(2).split(/=(.*)/s)
    const kind = FLAGS.get(name)?.kind
    flags.set(name, kind === "Switch" || kind === "Optional" ? (eq ?? "") : (eq ?? rest[++i] ?? ""))
  }
  const takes = (k: string) => FLAGS.get(k)?.commands.some((c) => c === "*" || c === cmd)
  const own = (p: J) => (p && typeof p === "object" ? Object.entries(p).map(([k, v]) => `${k}=${v}`).sort().join(",") : undefined)
  const params = (v: string | undefined) => v?.split(",").sort().join(",")
  const given = r.args?.flags ?? {}
  if (s) {
    const filtered = cmd === "record" ? flags.get("bench")?.split("#").pop() : flags.get("filter")
    if (filtered !== undefined && filtered !== s.case) return `names case ${filtered}, under series ${s.label}`
    if (filtered !== undefined && own(s.params) && params(flags.get("params")) !== own(s.params)) return `--params ${flags.get("params") ?? "(none)"}, under series ${s.label} of params ${own(s.params)}`
    if (cmd === "record" && s.target && flags.get("target") !== s.target) return `--target ${flags.get("target") ?? "(none)"}, under series ${s.label} of target ${s.target}`
    // The file's targets the result names, and whether its sides name them (then a rerun keeps the sides
    // and adds no --target): an implementation comparison, one target's variant, a check of two targets
    // (its one variant is the bench file's own folder).
    const targets = new Set<string>([...(r.series ?? []).map((x: J) => x.target), ...(r.variants ?? []).map((v: J) => v.target)].filter(Boolean))
    const namesTarget = (v: string) => v.startsWith("target:") || targets.has(v.split("=")[0])
    const sidesNameTargets = r.comparison === "implementations" || (r.variants ?? []).some((v: J) => typeof v.target === "string") || (r.kind === "check" && given.base !== undefined && (r.variants ?? []).length === 1)
    // A bench rerun of this series alone runs its own target, never the others `--target` picked.
    if (cmd === "bench" && filtered !== undefined && s.target) {
      if (sidesNameTargets && flags.has("target")) return `--target ${flags.get("target")} beside sides that name targets, under series ${s.label}`
      if (!sidesNameTargets && flags.get("target") !== s.target) return `--target ${flags.get("target") ?? "(none)"}, under series ${s.label} of target ${s.target}`
    }
    // A profile is of a build a side ran: its folder or ref as given, or a target side's variant (`.` without one).
    if (cmd === "record") {
      const norm = (p: string) => {
        const x = p.replace(/^\\\\\?\\UNC\\/, "//").replace(/^\\\\\?\\/, "").replace(/\\/g, "/").replace(/\/$/, "")
        return win ? x.toLowerCase() : x
      }
      // The sides as given, or (a result that kept no flags) its variants' specs.
      const sides = given.head !== undefined ? ["base", "head"].map((k) => given[k]) : (r.variants ?? []).map((v: J) => v.spec)
      const builds = sides.filter((v: J): v is string => typeof v === "string" && !v.startsWith("cmd:")).map((v: string) => (namesTarget(v) ? (v.includes("=") ? v.split("=").slice(1).join("=") : ".") : v))
      const head = flags.get("head") ?? "."
      if (!(builds.length ? builds : ["."]).some((b) => norm(b) === norm(head))) return `--head ${head}, a build no side of the experiment ran (${builds.join(", ") || "."})`
    }
  }
  for (const k of MEASURED.filter((k) => given[k] !== undefined && takes(k))) {
    const want = String(given[k])
    if (!flags.has(k)) return `drops --${k} ${want}`
    if (FLAGS.get(k)?.kind !== "Switch" && flags.get(k) !== want && !(want === "true" && flags.get(k) === "")) return `--${k} ${flags.get(k)}, the experiment ran --${k} ${want}`
  }
  if (r.plan?.used_random === true && takes("seed") && flags.get("seed") !== String(r.plan.seed)) return `--seed ${flags.get("seed") ?? "(none)"}: the hooks drew on seed ${r.plan.seed}, the workload`
  if (cmd === "record" && given.probes === "waits" && flags.get("probes") !== "waits") return "drops --probes waits from the profile"
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
// Bench files in folders a shell reads differently: a typographic apostrophe, and a `'` beside a `$`.
const placeholder = (dir: string) => (mkdirSync(join(work, dir), { recursive: true }), writeFileSync(join(work, dir, "app.bench.ts"), "// a placeholder\n"), join(work, dir, "app.bench.ts"))
const curlyFile = placeholder("Luke\u2019s repo")
const dollarFile = placeholder("O'Connor $x")
// Another machine's checkout: no such folder here.
const foreignDir = win ? "Z:\\nowhere-strobe\\alice\\app" : "/nowhere-strobe/alice/app"
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
  // A native comparison with off-CPU waits, pinned, a seeded workload: its profiles keep all of it. (Two-target
  // checks, params and seeds of a check: `--make` runs them for real.)
  ["exp_syn_waits", comparison([{ ...series("open", "app", worse), target_kind: "native", engine: null }], { head: "c1", base: "c0", probes: "waits", runs: "3", pin: "none", power: "keep" }, { plan: { runs_per_block: 3, seed: 11, used_random: true } })],
  // --blocks without attribution, and with --why always that failed.
  ["exp_syn_blocks", comparison([{ kind: "blocks", label: "b · app", case: "b", target: "app", target_kind: "script", primary: { metric: "cpu.work" }, blocks: [{ values: { "cpu.work": 3 } }, { values: { "cpu.work": 3.2 } }], attribution: {}, timing_recordings: [{ recording: "rec_syn_t", variant: "head" }] }], { head: ".", blocks: "2" })],
  // An attributed comparison whose recordings were pruned.
  ["exp_syn_pruned", comparison([series("load", "page", worse, { attribution: { base: "rec_syn_gone_b", head: "rec_syn_gone_h" } })], { head: "c1", base: "c0" })],
  // A result from another machine: its bench file and folder are not here, its recordings neither. Nothing it
  // prints may name them.
  ["exp_syn_foreign", comparison([series("load", "page", worse)], { head: foreignDir, base: "c0" }, { file: join(foreignDir, "perf", "x.bench.ts"), args: { command: "bench", pos: [join(foreignDir, "perf", "x.bench.ts")], flags: { head: foreignDir, base: "c0" } } })],
  // Paths a shell reads differently: a typographic apostrophe; a `'` beside a `$` (no spelling both shells read
  // alike: quoted for this OS's shell, and said so); a case named `--%` (PowerShell's stop-parsing token).
  ["exp_syn_curly", comparison([series("load", "page", worse)], { head: "c1", base: "c0" }, { file: curlyFile, args: { command: "bench", pos: [curlyFile], flags: { head: "c1", base: "c0" } } })],
  ["exp_syn_dollar", comparison([series("load", "page", worse)], { head: "c1", base: "c0" }, { file: dollarFile, args: { command: "bench", pos: [dollarFile], flags: { head: "c1", base: "c0" } } })],
  ["exp_syn_stop", comparison([series("--%", "page", underpowered)], { head: "c1", base: "c0" })],
  // A `strobe run` whose commands hold both quotes (`bun -e 'console.log("a")'`).
  ["exp_syn_runq", comparison([{ ...series("run", "command", underpowered), target_kind: "cli" }], { head: `cmd:bun -e 'console.log("b")'`, base: `cmd:bun -e 'console.log("a")'` }, { file: join(realState, "run", "cmd-abd", "run.bench.ts") })],
  // A comparison of one target's two variants that kept no flags: its variants name the target.
  ["exp_syn_onetarget", { ...comparison([series("port", "lib2", worse)], {}), args: { command: "bench", pos: [benchFile] }, variants: [{ label: "c0", spec: "c0", target: "lib2" }, { label: "c1", spec: "c1", target: "lib2" }] }],
  // Checks as the bench writes them with a base: one whose base failed (its error and hint are base's), one
  // whose `--noworse` counter failed (said once).
  // (The bench also gives the series base's hint as its own, `hint`: said once.)
  ["exp_syn_basefail", { ...comparison([{ kind: "check", label: "load · page", case: "load", target: "page", target_kind: "web", passed: false, state: "fail", primary: { metric: "cpu.work" }, error: "base failed, so head was checked against nothing (base's error is on its line)", hint: "export lib from base too, or --filter this case out until it lands", base: { passed: false, error: "the script host failed to load:\nerror: Cannot find module './lib' from 'perf/x.bench.ts'\n    at resolve", hint: "export lib from base too, or --filter this case out until it lands" } }], { head: "c1", base: "c0", check: "true" }), kind: "check" }],
  // Words zsh reads differently: a case starting with `=` (`=fast` is the program fast's path) and a ref with `^`
  // (`extendedglob` negation).
  ["exp_syn_zsh", comparison([series("=fast", "page", underpowered)], { head: "git:HEAD^", base: "c0", filter: "=fast" }, { variants: [{ label: "c0", spec: "c0" }, { label: "HEAD^", spec: "git:HEAD^" }] })],
  ["exp_syn_noworse", { ...comparison([{ kind: "check", label: "load · page", case: "load", target: "page", target_kind: "web", passed: false, state: "fail", primary: { metric: "cpu.work" }, error: "--noworse: count.items 1 → 2 · worse", counters: [{ metric: "count.items", passed: false, state: "worse", text: "count.items 1 → 2 · worse" }], base: { passed: true, values: {} } }], { head: "c1", base: "c0", check: "true", noworse: "count.items" }), kind: "check" }],
]
// What each result's insights must say beyond its commands (series index → a test of its result lines).
const sayings: Record<string, (out: J) => string | undefined> = {
  exp_syn_basefail: (o) => {
    const lines: string[] = o.series?.[0]?.result ?? []
    if (!lines.some((l) => l.includes("base FAILED: the script host failed to load:"))) return "base's error is not shown"
    if (!lines.some((l) => l.includes("Cannot find module './lib'"))) return "base's error's later lines are not shown"
    const hints = (o.series?.[0]?.next ?? []).filter((n: J) => String(n.why).includes("export lib from base")).length
    if (hints === 0) return "base's hint is not shown"
    if (hints > 1) return `base's hint, the series' too, is said ${hints} times`
  },
  exp_syn_zsh: (o) => (commandsOf(o).length ? undefined : "no command printed to check its = and ^ words"),
  exp_syn_noworse: (o) => {
    const lines: string[] = o.series?.[0]?.result ?? []
    const n = lines.filter((l) => l.includes("count.items 1 → 2")).length
    if (n !== 1) return `the --noworse failure is said ${n} times`
    if (lines.some((l) => l.includes("count.items: count.items"))) return "the counter's name is doubled"
  },
  exp_syn_foreign: (o) => (o.unrunnable_here ? undefined : "a result from another machine is not said to be unrunnable here"),
  exp_syn_dollar: (o) => (o.quoted_for ? undefined : "a word no spelling passes to both shells, and no quoting note"),
}
// No recording or experiment a result names that is not here may be printed (each synthetic one is missing).
const missingOk = new Set<string>()
for (const [id, r] of synthetic) {
  mkdirSync(join(fakeState, "experiments", id), { recursive: true })
  writeFileSync(join(fakeState, "experiments", id, "result.json"), JSON.stringify({ id, ...r }))
}

// ---------- real results, made here (`--make`) ----------

let failed = 0
// Each made result, by id (in this run's state folder), and read again by its path from a copy strobe does not keep.
const made: string[] = []
const byPath: string[] = []
if (argv.includes("--make")) {
  // A `'` and a space in every path: PowerShell and sh must both read the bench file's path whole.
  const repo = join(work, "O'Connor repo")
  const mk = Bun.spawnSync([bun, fileURLToPath(new URL("./insights-repo.ts", import.meta.url)), repo], { stdout: "pipe", stderr: "pipe" })
  if (mk.exitCode !== 0) throw new Error(`insights-repo.ts: ${mk.stderr.toString()}`)
  // The trace engine the real state folder holds, copied (else each run downloads it into its own).
  for (const v of existsSync(join(realState, "engine")) ? readdirSync(join(realState, "engine")) : []) {
    for (const exe of ["trace_processor_shell", "trace_processor_shell.exe"].filter((e) => existsSync(join(realState, "engine", v, e)))) {
      mkdirSync(join(fakeState, "engine", v), { recursive: true })
      cpSync(join(realState, "engine", v, exe), join(fakeState, "engine", v, exe))
    }
  }
  // A case that fails (its hint and the rerun of it alone).
  writeFileSync(
    join(repo, "fails.bench.ts"),
    `import { bench, script } from "@hona/strobe"\n\nexport default bench({\n  targets: { lib: script("./harness.ts") },\n  cases: {\n    boom: { primary: "cpu.work", run: () => { throw new Error("planted failure") } },\n  },\n})\n`,
  )
  // The shapes a real result has, each a check (no timed pairs), from the repository's folder.
  const MAKE: [string, string[]][] = [
    // Two targets of one case on one build, named by its folder or by a ref: a parity matrix, whose sides are builds.
    ["bench", ["more.bench.ts", "--head", repo, "--check", "--filter", "port"]],
    ["bench", ["more.bench.ts", "--head", "c1", "--check", "--filter", "port"]],
    // Two targets of one file, checked against each other: each case's line runs one of them.
    ["bench", ["more.bench.ts", "--base", "lib", "--head", "lib2", "--check", "--filter", "port"]],
    // The same with the sides' `target:` prefix, and a two-target check whose filter keeps a case one target has.
    ["bench", ["more.bench.ts", "--base", "target:lib", "--head", "target:lib2", "--check", "--filter", "port"]],
    ["bench", ["more.bench.ts", "--base", "lib", "--head", "lib2", "--check", "--filter", "sized"]],
    // Two builds of two targets picked by `--target a,b`: each series' rerun runs its own target alone.
    ["bench", ["more.bench.ts", "--base", "c0", "--head", "c1", "--target", "lib,lib2", "--check", "--filter", "port"]],
    // One target's variant (`--head <target>=<variant>`).
    ["bench", ["more.bench.ts", "--head", "lib2=c1", "--check", "--filter", "port"]],
    // A param swept (`--params all`): each series names its own value.
    ["bench", ["more.bench.ts", "--head", "c1", "--check", "--filter", "sized", "--params", "all"]],
    // A workload drawn from the seed, with how it ran: kept in every rerun and profile.
    ["bench", ["more.bench.ts", "--base", "c0", "--head", "c1", "--check", "--filter", "rand", "--runs", "2", "--pin", "none", "--power", "keep"]],
    // Attribution blocks of a script (`--why always`): their recordings' hits and handles.
    ["bench", ["insights.bench.ts", "--head", "c1", "--check", "--why", "always", "--filter", "regress,waits"]],
    // A page whose time is restyling, headless: a lean check recording and an attribution one.
    ["bench", ["insights.bench.ts", "--head", "c1", "--check", "--why", "always", "--filter", "restyle", "--headless"]],
    // A failed case.
    ["bench", ["fails.bench.ts", "--head", ".", "--check"]],
    // `strobe run --check`: one command, and two.
    ["run", ["--check", "--head", "bun -e 0"]],
    ["run", ["--check", "--base", "bun -e 0", "--head", "bun -e 1"]],
  ]
  for (const [command, args] of MAKE) {
    const p = Bun.spawnSync([STROBE, command, ...args, "--progress", "stdout"], { cwd: repo, stdout: "pipe", stderr: "pipe", env: { ...process.env, ...fakeEnv } })
    const id = (p.stdout.toString() + p.stderr.toString()).match(/\bexp_[0-9a-f]{6,}\b/g)?.pop()
    const failing = command === "bench" && args[0] === "fails.bench.ts"
    console.log(`made ${id ?? "(none)"}: strobe ${command} ${args.join(" ")} (exit ${p.exitCode})`)
    if (p.exitCode !== 0 && !failing) console.log(`     ${(p.stderr.toString() + p.stdout.toString()).trim().split("\n").filter((l) => /^strobe:|error/i.test(l)).slice(0, 3).join("\n     ")}`)
    if (id && existsSync(join(fakeState, "experiments", id, "result.json")) && (p.exitCode === 0 || (failing && p.exitCode === 1))) made.push(id)
    else failed++
  }
  // Each made result copied where strobe does not keep it, under an id it does not know: its commands name the copy.
  // The first also as an edited copy that keeps its id (strobe keeps a result of that id: not this one).
  for (const [n, id] of made.entries()) {
    const r = JSON.parse(readFileSync(join(fakeState, "experiments", id, "result.json"), "utf8"))
    const dir = join(work, "copied results", id)
    mkdirSync(dir, { recursive: true })
    writeFileSync(join(dir, "result.json"), JSON.stringify({ ...r, id: `exp_copied_${id.slice(4)}` }))
    byPath.push(join(dir, "result.json"))
    if (n === 0) {
      mkdirSync(join(dir, "same id"), { recursive: true })
      writeFileSync(join(dir, "same id", "result.json"), JSON.stringify(r))
      byPath.push(join(dir, "same id", "result.json"))
    }
  }
  // A recording folder copied the same way: its hits' handles name the folder.
  const recs = made.flatMap((id) => {
    const r = JSON.parse(readFileSync(join(fakeState, "experiments", id, "result.json"), "utf8"))
    return (r.series ?? []).flatMap((s: J) => [s.recording, typeof s.attribution === "string" ? s.attribution : undefined]).filter(Boolean)
  })
  for (const rec of recs.slice(0, 4)) {
    const to = join(work, "copied recordings", `${rec} copy`)
    Bun.spawnSync(win ? ["robocopy", join(fakeState, "recordings", rec), to, "/E", "/NFL", "/NDL", "/NJH", "/NJS"] : ["cp", "-R", join(fakeState, "recordings", rec), to])
    if (existsSync(join(to, "meta.json"))) byPath.push(to)
  }
}

// A result read from its path keeps its own rerun ledger (this machine's index is not its history).
const ledgered = join(work, "copied results", "ledger", "result.json")
mkdirSync(join(work, "copied results", "ledger"), { recursive: true })
const ownLedger = { first: false, text: "3rd run of the same variants (its own ledger)", same: ["exp_aaaaaa", "exp_bbbbbb"] }
writeFileSync(ledgered, JSON.stringify({ id: "exp_ledgered", ...synthetic[0][1], identity: { file: "app.bench.ts", base: "c0", head: "c1" }, ledger: ownLedger }))
byPath.push(ledgered)
// A recording folder strobe does not keep, made here: one CPU profile in a minified bundle (two hits, whose
// handles name the folder), in a folder with a `'` and a space.
const handmade = join(work, "hand made 'rec'")
mkdirSync(handmade, { recursive: true })
{
  const cf = (functionName: string, columnNumber: number) => ({ functionName, url: "http://127.0.0.1/vendor-8f3a.js", lineNumber: 0, columnNumber, scriptId: "1" })
  const samples = Array.from({ length: 40 }, (_, i) => 2 + (i % 2))
  const nodes = [{ id: 1, callFrame: { functionName: "(root)", url: "", lineNumber: -1, columnNumber: -1 }, children: [2, 3] }, { id: 2, callFrame: cf("a", 1204), children: [] }, { id: 3, callFrame: cf("b", 88011), children: [] }]
  writeFileSync(join(handmade, "cpu-0.cpuprofile"), JSON.stringify({ nodes, startTime: 0, endTime: 40000, samples, timeDeltas: samples.map(() => 1000) }))
  writeFileSync(join(handmade, "meta.json"), JSON.stringify({ id: "rec_handmade", kind: "record", probes: "attribution", values: {}, duration_ms: 40 }))
}
byPath.push(handmade)
// The same recording named by a canonical UNC path (`\\?\UNC\host\share\…`), where Windows serves this drive as
// a share to its own user: its handles must name `//host/share/…`, never `UNC/…`.
if (win && /^[A-Za-z]:\\/.test(handmade) && existsSync(`\\\\localhost\\${handmade[0]}$\\${handmade.slice(3)}`)) byPath.push(`\\\\?\\UNC\\localhost\\${handmade[0]}$\\${handmade.slice(3)}`)

// ---------- run ----------

let checked = 0
type Run = [string, Record<string, string>, string]
const runs: Run[] = [
  ...ids.map((id) => [id, {}, realState] as Run),
  ...made.map((id) => [id, fakeEnv, fakeState] as Run),
  ...byPath.map((p) => [p, fakeEnv, fakeState] as Run),
  ...synthetic.map(([id]) => [id, fakeEnv, fakeState] as Run),
]
for (const [id, env, state] of runs) {
  let out: J
  try {
    out = insightsOf(id, env)
  } catch (e) {
    console.log(`FAIL ${id}: ${e}`)
    failed++
    continue
  }
  // The result each series' commands are judged against (a recording by its path: none).
  const resultFile = /[\\/]/.test(id) ? (id.endsWith("result.json") ? id : null) : join(state, "experiments", id, "result.json")
  const result: J = resultFile ? JSON.parse(readFileSync(resultFile, "utf8")) : {}
  const fail = (why: string) => {
    console.log(`FAIL ${id}: ${why}`)
    failed++
  }
  // Quoted for one shell (a word no spelling passes to both): only that shell's reading is checked, and it says so.
  const shells = (["sh", "pwsh"] as const).filter((s) => !out.quoted_for || (out.quoted_for === "powershell") === (s === "pwsh"))
  for (const [c, i] of commandsOf(out)) {
    if (!c.startsWith("strobe ")) {
      fail(`not a strobe command: ${c}`)
      continue
    }
    // A result read from its path is named by that path: never by an id strobe keeps for another result.
    if (resultFile && /[\\/]/.test(id) && result.id && c.split(/\s+/).some((w) => w.replace(/^['"]|['"]$/g, "") === result.id)) fail(`${c} names ${result.id}, which strobe keeps for another result; this one was read from its path`)
    const z = zshReads(c)
    if (z) fail(`${c}: ${z}`)
    const rest = c.slice("strobe ".length)
    for (const shell of shells) {
      if ((shell === "sh" && !sh) || (shell === "pwsh" && !pwsh)) continue
      checked++
      const a = argvVia(shell, rest)
      const why = typeof a === "string" ? a : (check(a, state, missingOk) ?? semantic(a, i === null ? null : result.series?.[i], result))
      if (why) {
        console.log(`FAIL ${id} [${shell}] ${c}\n     ${why}`)
        failed++
      }
    }
  }
  const said = sayings[id]?.(out)
  if (said) fail(said)
  // A result read from its path keeps the ledger it holds.
  if (id === ledgered) {
    const p = Bun.spawnSync([STROBE, "query", id, "summary", "--json"], { stdout: "pipe", stderr: "pipe", env: { ...process.env, ...env } })
    const got = p.exitCode === 0 ? JSON.parse(p.stdout.toString()).ledger : `exit ${p.exitCode}`
    const canon = (v: J): string => (v && typeof v === "object" && !Array.isArray(v) ? `{${Object.keys(v).sort().map((k) => `${k}:${canon(v[k])}`).join(",")}}` : JSON.stringify(v))
    if (canon(got) !== canon(ownLedger)) fail(`query <path> summary replaced the result's own ledger with ${JSON.stringify(got)}`)
  }
  const ours = Object.keys(env).length > 0 || made.includes(id)
  if (ours) console.log(`${id}: ${commandsOf(out).length} commands`)
  // `--show`: each synthetic or made result's insights as text.
  if (argv.includes("--show") && ours) {
    console.log(Bun.spawnSync([STROBE, "query", id, "insights"], { stdout: "pipe", env: { ...process.env, ...env } }).stdout.toString())
  }
}
// A hint outside insights names a recording read by its path by that path, quoted as insights quote it: the
// thread view of the hand-made folder (no trace, a thread timeline) points at `--timeline`.
writeFileSync(join(handmade, "timeline.json.gz"), gzipSync(JSON.stringify({ threads: [] })))
{
  const p = Bun.spawnSync([STROBE, "query", handmade, "threads"], { stdout: "pipe", stderr: "pipe", env: { ...process.env, ...fakeEnv } })
  const said = p.stderr.toString() + p.stdout.toString()
  const c = said.match(/strobe query .* threads --timeline/)?.[0]
  if (!c) {
    console.log(`FAIL ${handmade} threads: no --timeline hint in ${JSON.stringify(said.trim())}`)
    failed++
  } else {
    const z = zshReads(c)
    if (z) {
      console.log(`FAIL ${handmade} threads: ${c}: ${z}`)
      failed++
    }
    for (const shell of (["sh", "pwsh"] as const).filter((s) => (s === "sh" ? sh : pwsh))) {
      checked++
      const a = argvVia(shell, c.slice("strobe ".length))
      const why = typeof a === "string" ? a : (check(a, fakeState, missingOk) ?? (a[1].replace(/\\/g, "/") === handmade.replace(/\\/g, "/") ? undefined : `names ${a[1]}, not the folder read`))
      if (why) {
        console.log(`FAIL ${handmade} threads [${shell}] ${c}\n     ${why}`)
        failed++
      }
    }
  }
}
rmSync(work, { recursive: true, force: true })
console.log(`${checked} command parses (sh${pwsh ? " and PowerShell" : ""}): ${failed ? `${failed} failed` : "all run as written"}`)
process.exit(failed ? 1 : 0)
