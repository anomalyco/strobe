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
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "fs"
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
// The synthetic SQL cells use the same engine as real page traces, in this run's own state.
for (const v of existsSync(join(realState, "engine")) ? readdirSync(join(realState, "engine")) : []) {
  for (const exe of ["trace_processor_shell", "trace_processor_shell.exe"].filter(e => existsSync(join(realState, "engine", v, e)))) {
    mkdirSync(join(fakeState, "engine", v), { recursive: true })
    cpSync(join(realState, "engine", v, exe), join(fakeState, "engine", v, exe))
  }
}
if (argv.includes("--make")) {
  // A `'` and a space in every path: PowerShell and sh must both read the bench file's path whole.
  const repo = join(work, "O'Connor repo")
  const mk = Bun.spawnSync([bun, fileURLToPath(new URL("./insights-repo.ts", import.meta.url)), repo], { stdout: "pipe", stderr: "pipe" })
  if (mk.exitCode !== 0) throw new Error(`insights-repo.ts: ${mk.stderr.toString()}`)
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
    ["bench", ["insights.bench.ts", "--head", "c1", "--check", "--trace", "full", "--filter", "restyle", "--headless"]],
    ["bench", ["insights.bench.ts", "--head", "c1", "--check", "--why", "always", "--filter", "restyle", "--headless", "--style"]],
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
// Follow the advice, not just the syntax. These two recordings and their parent check belong to this
// invocation. Render a report but don't open a browser; never run commands from an arbitrary result.
let followed = 0
{
  const lean = "rec_syn_untraced", prof = "rec_handmade", exp = "exp_syn_onecheck"
  const folder = (id: string) => join(fakeState, id.startsWith("rec_") ? "recordings" : "experiments", id)
  for (const id of [lean, prof, exp]) mkdirSync(folder(id), { recursive: true })
  for (const file of ["cpu-0.cpuprofile", "meta.json"]) cpSync(join(handmade, file), join(folder(prof), file))
  writeFileSync(join(folder(lean), "meta.json"), JSON.stringify({ kind: "block", id: lean, probes: "lean", traced: false, frames_refused: "Headless: no display", experiment: exp, values: {} }))
  const run = (...a: string[]) => Bun.spawnSync([STROBE, ...a], { env: { ...process.env, ...fakeEnv }, stdout: "pipe", stderr: "pipe", timeout: 60000 })
  const fail = (why: string) => { console.log(`FAIL check recording: ${why}`); failed++ }
  for (const attribution of [undefined, prof]) {
    writeFileSync(join(folder(exp), "result.json"), JSON.stringify({ id: exp, kind: "check", series: [{ recording: lean, attribution }] }))
    for (const view of ["summary", "hotspots", "layout", "style", "gc"]) {
      const p = run("query", lean, view), text = p.stdout.toString() + p.stderr.toString()
      if (view === "summary" ? p.exitCode !== 0 : p.exitCode !== 2 || !text.includes("check block")) fail(`${view}: ${text}`)
      const commands = [...text.matchAll(/strobe (query|report) ([^·;\n]+)/g)]
      if (view === "summary" && !commands.length || attribution && view === "hotspots" && !commands.some(m => m[2].trim() === `${prof} hotspots`)) fail(`missing next view in ${text}`)
      for (const [, cmd, tail] of commands) {
        const a = tail.trim().split(/\s+/).filter(x => x !== "--open")
        if (![lean, prof, exp].some(id => a.includes(id))) { fail(`unexpected handle: ${cmd} ${tail}`); continue }
        const next = run(cmd, ...a)
        if (next.exitCode !== 0) fail(`${cmd} ${tail}: ${next.stderr}`)
        followed++
      }
    }
    if (run("query", exp, "hotspots").exitCode !== (attribution ? 0 : 2)) fail("the check's hotspots did not use its profile")
  }
  // Exercise source selection, not just the printed argv. The same result is read through its ID and a copy.
  const second = "rec_syn_second", p2 = "rec_syn_attr2"
  const put = (d: string, f: string, value: J) => { mkdirSync(d, { recursive: true }); writeFileSync(join(d, f), JSON.stringify(value)) }
  const copy = join(work, "someone's copy"), file = join(copy, "result.json")
  const writeResult = (value: J) => { for (const d of [copy, folder(exp)]) put(d, "result.json", { id: exp, ...value }) }
  const c = (name: string, recording?: string, attribution?: string) => ({ kind: "check", case: name, label: name, target: "page", passed: true, recording, attribution, primary: {metric: "cpu.work"} })
  const leanMeta = (id: string, batch?: J) => ({ id, experiment: exp, kind: "block", probes: "lean", traced: false, values: {}, runs: 1, batch })
  for (const id of [lean, second]) { put(folder(id), "meta.json", leanMeta(id)); put(folder(id), "threads.json", []) }
  for (const id of [prof, p2]) {
    put(folder(id), "meta.json", { id, kind: "block", probes: "attribution", token: "tok", runs: 1, values: {} })
    cpSync(join(handmade,"cpu-0.cpuprofile"),join(folder(id),"cpu-0.cpuprofile"))
    put(folder(id), "gecko.json", { parts: { Layout: [["run:0",null]], Gc: [["run:0",null]], Style: [["run:0",null],["run:0","StyleInvalidationTracking"]] } })
  }
  const batch = [c("first", lean, prof), c("second", lean, p2)].map((s,i) => ({...s,seq:i+1,runs:[`run:${i}`]}))
  const go = (a: string[], code: number) => { const p=run(...a), t=p.stdout.toString()+p.stderr.toString(); if(p.exitCode!==code) fail(`${a.join(" ")}: ${p.exitCode}, wanted ${code}: ${t}`); return t }
  // Execute only reads of this run's own fixtures. Assert the exact set (including multiplicity)
  // before following, so a duplicated ID, a wrong case or circular advice cannot pass.
  const advice = (a: string[], expected: string[]) => {
    const t=go(a,2), got=Array.from(t.matchAll(/strobe query ((?:exp_|rec_)\w+ \w+)/g),m=>m[1])
    if(JSON.stringify(got.sort())!==JSON.stringify([...expected].sort())) fail(`${a.join(" ")}: ${JSON.stringify(got)}, wanted ${JSON.stringify(expected)}`)
    else for(const cmd of got) {go(["query",...cmd.split(" ")],0); followed++}
  }
  // For an actual Chromium trace made above, compare the hint with the view on the attribution
  // recording itself. Optional probes may be absent even though the recording is present.
  for (const id of made) {
    const result = JSON.parse(readFileSync(join(folder(id), "result.json"), "utf8"))
    for (const s of result.series ?? []) {
      if (s.case === "restyle" && s.recording) {
        const lean = JSON.parse(readFileSync(join(folder(s.recording),"meta.json"),"utf8"))
        const why = go(["query",s.recording,"frames"],4)
        const has = typeof s.attribution === "string", hint = has ? `strobe query ${s.attribution} frames` : "--why always"
        if (lean.traced !== false || !why.includes("Headless:") || why.includes("--trace full") || !why.includes(hint) || has && why.includes("--why always")) fail(`headless frames: ${why}`)
        if (has) {
          const text = go(["query",s.attribution,"frames"],0), j = JSON.parse(go(["query",s.attribution,"frames","--json"],0))
          if (!text.includes(`${j.compositor_reports} compositor reports (no display; perturbed)`) || /\bpresented\b|vsyncs dropped|display .*Hz/.test(text) || j.headless!==true || !j.frames_refused || j.timing!=="perturbed" || "presented" in j || "dropped_pct" in j || "values" in j) fail(`headless pipeline: ${text} ${JSON.stringify(j)}`)
          if (JSON.parse(go(["query",s.attribution,"inputs","--json"],4)).validator!=="Headless") fail("headless inputs claimed screen latency")
          if (!go(["query",id,"frames"],4).includes(hint)) fail("experiment missed its kept profile")
          followed++
        }
        console.log(`headless frames: ${s.recording} (trace ${result.args?.flags?.trace ?? "default"}): 4; attribution ${s.attribution ?? "not requested"}`)
      }
      if (!s.recording || typeof s.attribution !== "string" || !existsSync(join(folder(s.attribution), "trace.pftrace"))) continue
      for (const view of ["layout", "gc", "style"]) {
        const available = run("query", s.attribution, view).exitCode === 0
        advice(["query", s.recording, view], [`${s.attribution} ${available ? view : "summary"}`])
      }
    }
  }
  for (const shape of ["single", "no profile", "several", "batch", "batch without profile", "repeat", "mixed", "refused sibling", "comparison", "blocks"]) {
    const shared=shape.startsWith("batch"), timed=shape==="comparison"||shape==="blocks", none=shape==="no profile"||shape==="batch without profile"
    put(folder(lean),"meta.json",leanMeta(lean,shared?batch:undefined))
    const s=c("first",lean,none?undefined:prof)
    const series: J[]=shared?[s,c("second",lean,none?undefined:p2)] : shape==="several"?[s,c("second",second,p2)] : shape==="mixed"?[s,{...c("failed"),passed:false}] : shape==="refused sibling"?[s,{...c("skipped"),kind:"refused"}] : shape==="repeat"?[{...s,repeat:{recordings:[lean,second]}}] : timed?[{...s,kind:shape==="blocks"?"blocks":"runs",timing_recordings:[{recording:lean}],attribution:{head:prof,base:shape==="comparison"?p2:undefined}}]:[s]
    writeResult({kind:timed?"experiment":"check",series})
    advice(["query",shape==="repeat"?second:lean,"hotspots"],none?[]:shared?[`${exp} summary`]:[`${timed?exp:prof} hotspots`])
    if(none && !go(["query", lean, "hotspots"], 2).includes("--why always")) fail(`${shape}: did not explain how to keep a profile`)
    if(shared && !none) advice(["query",lean,"layout","--case","second"],[`${p2} layout`])
    if(shape==="single") for(const view of ["layout","gc","style"]) advice(["query",lean,view],[`${prof} ${view}`])
    for (const h of [exp,file]) {
      if(none) advice(["query",h,"gc"],[])
      else if (["several","batch","mixed"].includes(shape)) advice(["query",h,"gc"],(shape==="mixed"?[prof]:[prof,p2]).map(id=>`${id} gc`))
      else { const t=go(["query",h,"hotspots"],0); if(!t.includes(prof) || !t.includes("first")) fail(`${shape}: missing case/recording provenance`) }
      if(shared||shape==="repeat") advice(["query",h,"threads"],(shared?[lean]:[lean,second]).map(id=>`${id} threads`))
      if(shape==="mixed") advice(["query",h,"threads"],[`${lean} threads`])
    }
    console.log(`source selection: ${shape} (id and copy)`)
  }
  // Script capture whose sampler saw no measured-run sample: the whole-host profile is still
  // there. Opening hotspots is valid (zero samples), so do not send it to summary.
  const hostOnly = "rec_syn_hostonly"
  put(folder(hostOnly), "meta.json", { id: hostOnly, kind: "block", probes: "attribution", script: true, runs: 1, values: {} })
  writeFileSync(join(folder(hostOnly), "whole-host.json.gz"), gzipSync(readFileSync(join(handmade, "cpu-0.cpuprofile"))))
  writeResult({kind:"check", series:[c("host",lean,hostOnly)]})
  advice(["query",lean,"hotspots"],[`${hostOnly} hotspots`])
  // Cross the recording views with the check shapes. A direct query supplies the expected
  // refusal (including the validator), not an approximation based on which files exist.
  const profileViews = "hotspots alloc layout style gc heap shifts waits".split(" ")
  const launchViews = "frames inputs threads gpu memory runs log network leaks sql".split(" ")
  const gz = (id: string, name: string, v: J) => writeFileSync(join(folder(id), name), gzipSync(JSON.stringify(v)))
  const snap = {snapshot: {meta: {node_fields:["type","name","id","self_size","edge_count"],edge_fields:["type","name_or_index","to_node"],node_types:[["synthetic"]],edge_types:[["property"]]}},nodes:[0,0,1,0,0],edges:[],strings:["root"]}
  const native = "rec_syn_native", terminal = "rec_syn_terminal", device = "rec_syn_device"
  put(folder(native), "meta.json", {...leanMeta(native), native:true, gpu:{gpu_3d_pct:0}, run_list:[{run:"run:0",ms:1}]})
  put(folder(native), "memory.json", {readings:[{processes:[]}]})
  put(folder(native), "frames.json", {runs:{"run:0":{t0_ns:0,t1_ns:1000000}},pids:[]})
  gz(native, "presents.json.gz", {presents:[],vsyncs:[],events:0,events_lost:0,t0_ns:0,t1_ns:1000000})
  put(folder(terminal), "meta.json", {...leanMeta(terminal), tty:true})
  put(folder(device), "meta.json", {...leanMeta(device), wir:true})
  gz(device, "console.json.gz", [])
  gz(device, "network.json.gz", {navigation:{},paint:[],spans:[],requests:[]})
  for (const name of ["heap-s0","heap-s1","heap-0"]) gz(device, `${name}.heapsnapshot.gz`, snap)
  gz(prof, "heap-0.heapsnapshot.gz", snap)
  put(folder(prof), "shifts.json", {shifts:[]})
  put(folder(prof), "alloc-0.heapprofile", {head:null,samples:[]})
  const stacks = {threads:{},modules:[],stacks:[],events_lost:0}
  gz(p2, "deep.json.gz", {...stacks,interval_us:1000,t0_ns:0,t1_ns:1000000,samples:[],pids:[],waits:{...stacks,waits:[]}})
  const seen = new Set<string>()
  let cells = 0
  const stamp = (p:string) => existsSync(p) ? String(statSync(p,{bigint:true}).mtimeNs) : "absent"
  const tree = (dir:string) => existsSync(dir) ? [stamp(dir),...readdirSync(dir,{recursive:true}).sort().map(n=>[n,stamp(join(dir,String(n)))])] : []
  const snapshot = (ids:string[]) => JSON.stringify([...ids.map(id=>tree(folder(id))),stamp(join(folder(exp),".used"))])
  const scratch = () => JSON.stringify([stamp(join(fakeState,"run")),tree(join(fakeState,"run","probe"))])
  const matrix = (name: string, series: J[], launches: string[], profiles: string[], shared = false) => {
    writeResult({kind:"check",series})
    for (const q of [...profileViews,...launchViews,"unknown","sql select 1","threads --timeline"]) {
      const [view, ...rest] = q.split(" "), args = view === "sql" && rest.length ? [view,rest.join(" ")] : [view,...rest]
      const ids = profileViews.includes(view) ? profiles : launches
      const opts = args.map(a => a.includes(" ") ? `'${a}'` : a).join(" ")
      const before = snapshot(ids), cold = scratch()
      const p = run("query",exp,...args), text = (p.stderr.toString() + p.stdout.toString()).trim()
      const listing = series.length > 1 || ids.length > 1 || shared
      // Every engine query creates/removes a unique q-*.sql file in run/. A changed directory
      // stamp catches even transient SQL files that would disappear from a subsequent listing.
      if (listing && snapshot(ids) !== before) fail(`${name} ${q}: listing wrote a recording or a used stamp`)
      if (listing && name === "page trace" && ["style","inputs","runs"].includes(view)) {
        const hot = scratch(), again=run("query",exp,...args)
        if (hot!==scratch() || snapshot(ids)!==before || p.exitCode!==again.exitCode || String(p.stderr)!==String(again.stderr)) fail(`${name} ${q}: second listing wrote files, restarted the engine, or changed its answer`)
        if(view!=="style" && cold!==hot) fail(`${name} ${q}: did not reuse style's trace facts`)
      } else if (listing && scratch()!==cold) fail(`${name} ${q}: listing wrote scratch or started the engine`)
      if (listing && view === "sql" && rest.length) {
        const stdout = run("query",exp,...args,"--progress","stdout")
        if (stdout.stdout.length || stdout.exitCode !== p.exitCode || !stdout.stderr.toString().startsWith("strobe: choose a recording")) fail(`${name}: SQL choice leaked stdout progress`)
        if(snapshot(ids)!==before) fail(`${name}: SQL choice wrote to the store or started the engine`)
      }
      const direct = ids.map(id => run("query",id,...args))
      const want = ids.filter((_,i) => direct[i].exitCode === 0)
      const tag = `${name}: ${opts}`
      if (!ids.length) {
        if (p.exitCode !== 2 || !text.includes(profileViews.includes(view) ? "--why always" : "no kept recording")) fail(`${tag}: ${text}`)
      } else if (!want.length) {
        // Forwarding a single case prefixes the source to the same refusal.
        const why = direct.map(r=>{
          const msg = r.stderr.toString().trim().replace(/^strobe: /, "")
          if (!listing || view !== "threads") return msg
          return msg.replace(/: its threads over time are in strobe query \S+ threads --timeline$/, ": request threads with --timeline for the recorded timeline")
            .replace(/: a native stack capture answers strobe query \S+ hotspots --by package$/, ": use the hotspots view of the native stack capture instead")
        })
        const distinct = [...new Set(why)]
        if (distinct.length === 1) {
          if (!why[0] || p.exitCode !== direct[0].exitCode || !text.includes(why[0])) fail(`${tag}: not the direct refusal (${why[0]}): ${text}`)
        } else if (p.exitCode !== ([124,255,1].find(code=>direct.some(r=>r.exitCode===code)) ?? (direct.every(r=>r.exitCode===4)?4:2)) || !text.startsWith("strobe: no recording of this check answers") || !distinct.every(w=>text.includes(w.split("\n")[0]))) fail(`${tag}: missing mixed reasons: ${text}`)
      } else if (series.length === 1 && ids.length === 1 && !shared) {
        if (p.exitCode !== 0 || !text.includes(`${series[0].label} · ${ids[0]}`)) fail(`${tag}: not forwarded from ${ids[0]}`)
      } else {
        const [header,...lines] = text.split(/\r?\n/)
        if(p.stdout.length || header !== "strobe: choose a recording of this check:") fail(`${tag}: unexpected preamble: ${text}`)
        const got = lines.map(l => l.trim())
        const expected = want.map(id => `strobe query ${id} ${opts}`)
        if (p.exitCode !== 2 || JSON.stringify(got) !== JSON.stringify(expected)) fail(`${tag}: ${text}, expected ${expected}`)
        else for (const cmd of got) {
          // These fixture words are plain, except the SQL statement. Pass the quoted one through a shell.
          const a = cmd.includes("'") ? argvVia(sh ? "sh" : "pwsh", cmd.slice(7)) : cmd.slice(7).split(" ")
          if (typeof a === "string") fail(`${tag}: ${a}`)
          else { go(a,0); followed++; seen.add(view) }
        }
      }
      cells++
    }
  }
  gz(lean,"timeline.json.gz",{threads:[]})
  for (const kept of [false,true]) {
    const p = kept ? prof : undefined, b = kept ? p2 : undefined, one = c("first",lean,p)
    const ps = p ? [p] : [], both = p && b ? [p,b] : []
    put(folder(lean),"meta.json",leanMeta(lean))
    matrix("one",[one],[lean],ps)
    matrix("repeat",[{...one,repeat:{recordings:[lean,second]}}],[lean,second],ps)
    matrix("several untraced",[one,c("second",second,b)],[lean,second],both)
    matrix("targets",[one,c("native",native,b),c("tty",terminal),c("device",device),c("script",undefined,kept?hostOnly:undefined)],
      [lean,native,terminal,device,...(kept?[hostOnly]:[])],[...both,...(kept?[hostOnly]:[])])
    put(folder(lean),"meta.json",leanMeta(lean,batch))
    matrix("shared",[one,c("second",lean,b)],[lean],both,true)
    if(kept && !go(["query",exp,"gc","--case","second"],0).startsWith(`second · ${p2}`)) fail("--case did not read the second profile")
  }
  // The actual page trace made by --make alongside an untraced launch. No new app is launched.
  // A trace without any index or run list. With no explicit measured keys only its last run
  // counts: an invalidation in run:0 alone cannot answer style. Re-record the same folder with
  // the invalidation in run:1; now it must be offered. Neither trace depends on a live page.
  const legacy="rec_syn_trace", old=folder(legacy)
  put(old,"meta.json",{id:legacy,kind:"block",probes:"attribution",token:"tok",runs:1})
  const event=(name:string,ts:number)=>({name,ts,ph:"X",cat:"devtools.timeline",pid:1,tid:1,dur:1})
  for(const invalidate of [15,35]) {
    put(old,"trace.pftrace",{traceEvents:[event("strobe:tok:run:0:start",10),event("strobe:tok:run:0:end",20),event("strobe:tok:run:1:start",30),event("strobe:tok:run:1:end",40),event("StyleInvalidationTracking",invalidate)]})
    writeResult({kind:"check",series:[c("traced",legacy,legacy),c("headless",second,hostOnly)]})
    const before=snapshot([legacy,second,hostOnly]), t=go(["query",exp,"style"],2), hot=scratch()
    if(t.includes(`strobe query ${legacy} style`)!==(invalidate===35))fail(`style used the wrong run (event at ${invalidate}): ${t}`)
    for(const v of ["style","inputs","runs"]) {
      const t=go(["query",exp,v],2)
      if(v!=="style" && !t.includes(`strobe query ${legacy} ${v}`))fail(`missing trace-only ${v}: ${t}`)
      if(hot!==scratch() || before!==snapshot([legacy,second,hostOnly]))fail(`warm ${v}: changed files or ran the engine`)
    }
    for(const v of ["style","inputs","runs"]){go(["query",legacy,v],v==="style" && invalidate===15?2:0);followed++}
  }
  // Break the cache, not the trace. Check a failed publication, an unusable lock on a warm
  // (deliberately stale) entry and a file in place of the entire cache directory.
  const probe = join(fakeState,"run","probe")
  const factsName = readdirSync(probe).find(n=>n.endsWith(".json") && JSON.parse(readFileSync(join(probe,n),"utf8")).runs["run:1"] === true)
  if (!factsName) throw Error("no warm trace facts for run:1")
  const facts = join(probe, factsName), lock = facts.replace(/\.json$/, ".lock"), warm = JSON.parse(readFileSync(facts,"utf8"))
  const uncached = () => advice(["query",exp,"style"],[`${legacy} style`])
  rmSync(facts); mkdirSync(facts); uncached(); rmSync(facts,{recursive:true})
  put(probe,factsName,{...warm,runs:{}}); rmSync(lock); mkdirSync(lock); uncached()
  rmSync(probe,{recursive:true}); writeFileSync(probe,""); uncached(); rmSync(probe)
  // No download or traffic to the registry: the sandbox's missing engine goes through a dead
  // local proxy. The second recording has no run list or trace (2). Preserve both reasons.
  const engine = join(fakeState,"engine"); renameSync(engine,engine+".saved")
  try {
    const offline = { ...process.env, ...fakeEnv, HTTPS_PROXY:"http://127.0.0.1:1", https_proxy:"http://127.0.0.1:1", NO_PROXY:"", no_proxy:"" }
    for (const reverse of [false,true]) {
      const series = [c("trace",legacy),c("no trace",second)]
      writeResult({kind:"check",series:reverse?series.reverse():series})
      const p = Bun.spawnSync([STROBE,"query",exp,"runs","--json"],{env:offline,timeout:15000})
      const j = JSON.parse(p.stdout.toString())
      if (p.exitCode!==255 || j.code!==255 || JSON.stringify(j.recording_errors?.map((r:J)=>r.code))!==JSON.stringify(reverse?[2,255]:[255,2])) fail(`offline engine: ${p.stdout} ${p.stderr}`)
    }
  } finally {
    rmSync(engine,{recursive:true,force:true}); renameSync(engine+".saved",engine)
  }
  for (const id of made) {
    const r = JSON.parse(readFileSync(join(folder(id),"result.json"),"utf8"))
    for (const s of r.series ?? []) if (s.attribution && typeof s.attribution === "string" && existsSync(join(folder(s.attribution),"trace.pftrace"))) {
      // Copy the old-format source shape: no index and no run list to short-circuit trace-only runs.
      const d=folder(s.attribution), m=JSON.parse(readFileSync(join(d,"meta.json"),"utf8"))
      delete m.measured; delete m.run_list; delete m.style_runs; delete m.trace_runs
      put(d,"meta.json",m)
      rmSync(join(d,"runs.json"),{force:true})
      matrix("page trace",[c("traced",s.attribution,s.attribution),c("headless",second,p2)],[s.attribution,second],[s.attribution,p2])
      const missing=join(d,"trace.pftrace"), original=readFileSync(missing)
      // Replacing the trace under the same id must not bring back its cached runs.
      writeFileSync(missing,JSON.stringify({traceEvents:[{ph:"X",name:"other",pid:1,tid:1,ts:0,dur:1}]}))
      const changed=run("query",exp,"runs")
      if(changed.stderr.toString().includes(`strobe query ${s.attribution} runs`))fail("replaced trace served old run keys")
      writeFileSync(missing,original)
    }
  }
  for (const v of [...profileViews,...launchViews]) if (!seen.has(v)) fail(`no executable listing for ${v}`)
  console.log(`${cells} recording-selection cells (forward, choose, or refuse)`)
  // Descending IDs, duplicate reasons, and two different types of refusal. Assert text order,
  // status and the complete JSON for each reason against a direct query.
  const mcp = (view:string) => {
    const msg={jsonrpc:"2.0",id:1,method:"tools/call",params:{name:"query",arguments:{handle:exp,view}}}
    const p=Bun.spawnSync([STROBE,"mcp"],{stdin:Buffer.from(JSON.stringify(msg)+"\n"),stdout:"pipe",stderr:"pipe",env:{...process.env,...fakeEnv},timeout:15000})
    return JSON.parse(p.stdout.toString()).result
  }
  for(const [view,first,second,code] of [["frames",{wir:true},{native:true},2],["frames",{traced:false,frames_refused:"Headless: no display"},{native:true},2],["alloc",{engine:"firefox"},{script:true,runtime:{name:"bun"}},4]] as const){
    const pairs=[["page","rec_syn_z",first],["another page","rec_syn_y",first],["other","rec_syn_a",second]] as const
    for(const [,id,m] of pairs) put(folder(id),"meta.json",{id,probes:"attribution",...m})
    writeResult({kind:"check",series:pairs.map(([name,id])=>c(name,id,id))})
    const text=go(["query",exp,view],code),j=JSON.parse(go(["query",exp,view,"--json"],code))
    if(text.trim()!==`strobe: ${j.error}` || !j.error.startsWith(`no recording of this check answers ${view}:`) || j.recording_errors?.length!==2) fail(`mixed ${view}: ${text}`)
    for(const [i,[series,id]] of [pairs[0],pairs[2]].entries()) {
      const r=JSON.parse(run("query",id,view,"--json").stdout.toString()),row=j.recording_errors?.[i]
      if(!row || row.series!==series || row.recording!==id || !["code","validator","error","hint"].every(k=>row[k]===r[k])) fail(`mixed ${view}: ${JSON.stringify(j)}`)
    }
    console.log(`mixed ${view} (exit ${code}): ${text.trim()}\n${JSON.stringify(j)}`)
    const tool=mcp(view)
    if(j.validator!==(code===4?"EngineUnsupported":null) || tool.structuredContent?.validator!==j.validator || tool.structuredContent?.error!==j.error || tool.content[0].text.endsWith("fix: "))fail(`mixed ${view}: MCP validator or fix`)
    if(view==="alloc")console.log(`MCP: ${JSON.stringify(tool)}`)
    if("wir" in first){
      writeResult({kind:"check",series:pairs.slice(0,2).map(([name,id])=>c(name,id,id))})
      const cli=JSON.parse(go(["query",exp,view,"--json"],4)),one=JSON.parse(go(["query",pairs[0][1],view,"--json"],4)),t=mcp(view)
      if(JSON.stringify(cli)!==JSON.stringify(one) || t.structuredContent?.validator!==one.validator || t.structuredContent?.hint!==one.hint)fail("same reason with different IDs did not collapse")
      for(const h of [file,exp]) for(const v of ["frames","threads","gpu"]) {
        const shared = JSON.parse(go(["query",h,v,"--json"],4))
        if (shared.recording_errors || !shared.hint || /rec_syn_|strobe query/.test(shared.hint)) fail(`not a recording-neutral ${v} fix: ${shared.hint}`)
      }
      console.log(`collapsed frames: ${go(["query",exp,view],4).trim()}\n${JSON.stringify(cli)}\nMCP: ${JSON.stringify(t)}`)
    }
  }
  const untimed="rec_syn_untimed"
  put(folder(untimed),"meta.json",{id:untimed,probes:"attribution",native:true})
  put(folder(untimed),"frames.json",{runs:{"run:0":{marks:[["a:phase:render:start",1],["a:phase:render:end",2]]}}})
  gz(untimed,"deep.json.gz",{...stacks,interval_us:1000,t0_ns:0,t1_ns:1000,samples:[],pids:[],untimed:true})
  writeResult({kind:"check",series:[c("native",undefined,untimed),c("other")]})
  const phase=["hotspots","--phase","render"], why=go(["query",untimed,...phase],2)
  if(!why.includes("samples carry no times") || go(["query",exp,...phase],2)!==why) fail("untimed phase listing differs from direct query")
  // Same failure, four shapes: base refusal alone, both sides failing independently, head-only failure,
  // and a passing check. Keep the base reason exactly once and keep useful detail after its first line.
  for(const [name,extra,want] of [
    ["refused base",{error:"base failed (NoPhase: no samples), so head was checked against nothing",refusal:{validator:"NoPhase",summary:"no samples"},base:{passed:false,error:"NoPhase: no samples"}},["base failed","no samples"]],
    ["both failed",{error:"head stopped",base:{passed:false,error:"could not load:\nmissing module"}},["head stopped","missing module"]],
    ["head failed",{error:"head stopped",base:{passed:true}},["head stopped"]], ["pass",{passed:true},[]],
  ] as const){
    writeResult({kind:"check",series:[{...c("first",lean),state: name==="pass"?"pass":"fail",passed:false,...extra}]})
    for(const h of [exp,file]) {
      const row = go(["report",h,"--format","summary"],0).split("\n").find(l=>l.startsWith("| first |"))
      const note = row?.split("|").at(-2) ?? ""
      for(const w of want) if(note.split(w).length!==2) fail(`${name}: expected ${w} exactly once in Notes: ${note}`)
    }
  }
  for(const d of [copy,folder(exp)])writeFileSync(join(d,"first.actions.json.gz"),gzipSync(JSON.stringify({actions:[]})))
  writeResult({kind:"check",series:[{...c("first"),actions:{count:1,path:"first.actions.json.gz"}}]})
  const checkText=(id:string,handle:string,cmd:string,t:string) => {
    const [prefix,suffix]=cmd.split("HANDLE")
    const start=t.indexOf(prefix), end=t.indexOf(suffix,start+prefix.length)
    if(start<0 || end<0) {fail(`missing ${cmd}`);return}
    const rest=t.slice(start+"strobe ".length,end+suffix.length)
    const expected=cmd.slice(7).split(" ").map(w=>w==="HANDLE"?handle.replace(/\\/g,"/"):w)
    for (const shell of (["sh","pwsh"] as const).filter(s=>s==="sh"?sh:pwsh)) {
      const a=argvVia(shell,rest)
      if(typeof a==="string" || JSON.stringify(a)!==JSON.stringify(expected)) {fail(`${shell}: ${rest} did not name the artifact read`);continue}
      go(a.filter(w=>w!=="--open"),0);followed++
    }
    if(handle!==id && (t.includes(`query ${id} `)||t.includes(`report experiment ${id} `)||t.includes(`--resume ${id}`)))fail(`command still names stored ${id}`)
  }
  for(const h of [exp,file]) for (const a of [["query",h,"summary"],["report",h,"--format","md"]])checkText(exp,h,"strobe query HANDLE actions",go(a,0))
  writeResult({kind:"experiment",series:[{kind:"runs",label:"first",primary:{metric:"cpu.work"}}],stopped:{code:130}})
  for(const a of [["query",file,"summary"],["report",file,"--format","md"]])checkText(exp,file,"strobe report experiment HANDLE --open",go(a,0))
  const prose = "slow query search took 40 ms; strobe query search actions; run(query )"
  for (const id of ["search", undefined]) {
    put(copy,"result.json",{id,kind:"check",series:[{...c("first"),passed:false,log:[prose],error:prose,hint:prose,actions:{count:1,path:"first.actions.json.gz"}}],warnings:[{id:"note",text:prose}]})
    for (const a of [["query",file,"summary"],["report",file,"--format","md"]]) {
      const t = go(a,0)
      if (t.split(prose).length !== 5) fail(`${a[0]} changed a log, error, hint or warning: ${t}`)
      if (id) checkText(id,file,"strobe query HANDLE actions",t.split("\n").find(l=>l.startsWith("      actions  ")) ?? "")
      else if (t.includes("result.json") || !t.includes("      actions  1 sent before it failed\n")) fail(`${a[0]} injected a path or lost the action count without an id`)
    }
  }
  put(copy,"result.json",{kind:"experiment",series:[{kind:"runs",label:"first",primary:{metric:"cpu.work"}}],stopped:{code:130}})
  for (const a of [["query",file,"summary"],["report",file,"--format","md"]]) {
    const t = go(a,0)
    if (t.includes("--resume") || t.includes("strobe report experiment  ")) fail(`${a[0]} offered an empty handle`)
  }
  const filmed=join(work,"filmed 'copy'");put(filmed,"meta.json",{id:lean,kind:"record",wir:true,video:{kind:"screen"},network:{pre_attach_resources:1},values:{}})
  writeFileSync(join(filmed,"network.json.gz"),gzipSync(JSON.stringify({navigation:{},paint:[],spans:[],requests:[]})))
  for(const a of [["query",filmed,"summary"],["report",filmed,"--format","md"]]){
    const text=go(a,0),v=text.split("\n").find(l=>l.startsWith("video    "))??"",n=text.split("\n").find(l=>l.startsWith("network  "))??""
    checkText(lean,filmed,"strobe report recording HANDLE --open",v);checkText(lean,filmed,"strobe query HANDLE network","strobe "+n.slice(n.indexOf("query ")))
  }
  put(folder(prof),"meta.json",{id:prof,probes:"attribution",launch:true,values:{}})
  checkText(prof,prof,"strobe query HANDLE hotspots",go(["query",prof,"summary"],0))
  for(const a of [["query",folder(prof),"summary"],["report",folder(prof),"--format","md"]]) checkText(prof,prof,"strobe query HANDLE hotspots",go(a,0))
  const paths=[...byPath,filmed]
  // Stored results are the common case. With STROBE_REFERENCE set, compare whole stdout bytes to
  // that release on exactly the same state. The only allowed edits remove verified failed views.
  const reference=process.env.STROBE_REFERENCE
  let compared=0, identical=0
  const compareStored=(name:string,id:string,changes:[string,string][] = [])=>{
    if(!reference)return
    for(const a of [["query",id,"summary"],["report",id,"--format","md"]]) {
      const old=Bun.spawnSync([reference,...a],{env:{...process.env,...fakeEnv},timeout:60000}), now=run(...a)
      if(old.exitCode || now.exitCode) { fail(`stored ${name} ${a[0]}: ${old.exitCode}/${now.exitCode}`);continue }
      let expected=old.stdout.toString()
      for(const [before,after] of changes) {
        if(!expected.includes(before))fail(`stored ${name} ${a[0]}: missing baseline line ${before}`)
        expected=expected.replaceAll(before,after)
      }
      if(!Buffer.from(expected).equals(now.stdout)) {
        const lines=expected.split("\n"), got=now.stdout.toString().split("\n"), i=lines.findIndex((l,i)=>l!==got[i]);fail(`stored ${name} ${a[0]}: expected ${lines[i]}, got ${got[i]}`)
      }
      compared++; if(old.stdout.equals(now.stdout))identical++
    }
  }
  for(const id of made)compareStored(id,id)
  const raw="rec_stored", q=`strobe query ${raw} `, nativeViews=["hotspots --by package","threads","frames"]
  for(const shape of ["page","script","command","terminal","phone","native-windowed","native-windowless","native-launch","native-no-stacks","http","leak-memory","leak-snapshots","leak-no-snapshots"]) {
    const d=folder(raw), nat=shape.startsWith("native"), leak=shape.startsWith("leak"), windowed=shape==="native-windowed"||shape==="native-no-stacks"
    rmSync(d,{recursive:true,force:true})
    const kind=nat?"native":shape==="terminal"?"tty":shape==="command"?"cli":shape==="script"||shape==="http"?"script":"web"
    put(d,"meta.json",{...leanMeta(raw),native:nat,script:kind==="script",cli:kind==="cli",tty:kind==="tty",launch:shape==="native-launch",phone:shape==="phone"?{serial:"fixture"}:null,probes:"attribution",frames_refused:shape==="native-windowless"?"NotForTarget: no window":null})
    if(nat) {
      put(d,"threads.json",[])
      if(shape!=="native-no-stacks")cpSync(join(folder(p2),"deep.json.gz"),join(d,"deep.json.gz"))
      if(windowed)for(const f of ["frames.json","presents.json.gz"])cpSync(join(folder(native),f),join(d,f))
    } else if(["page","phone","script","http"].includes(shape))cpSync(join(handmade,"cpu-0.cpuprofile"),join(d,"cpu-0.cpuprofile"))
    if(shape==="http")gz(raw,"http_probe.json.gz",{config:{method:"GET",url:"http://127.0.0.1/",cadence_ms:10,timeout_ms:100,idle_ms:0,connection:"new",max_in_flight:1,headers:[],has_authorization:false,predicate:{status:[200,299]}},start_ns:1,end_ns:10,samples:[]})
    if(leak) {
      put(d,"memory.json",{readings:[{processes:[]}]})
      if(shape==="leak-snapshots")for(const n of ["heap-s0","heap-s1","heap-0"])gz(raw,`${n}.heapsnapshot.gz`,snap)
    }
    const wantWhy=!["command","terminal"].includes(shape)
    const s={...c(shape,raw,wantWhy?raw:undefined),target_kind:kind,state:"pass",values:shape==="http"?{"http.probes":1}:{}}
    const all=leak?(shape==="leak-memory"?["memory --at 0","memory"]:["leaks","memory"]):nat?nativeViews:["hotspots --by package"]
    const kept=all.filter(v=>!(v==="frames"&&!windowed || v.startsWith("hotspots")&&shape==="native-no-stacks" || v==="leaks"&&shape==="leak-no-snapshots"))
    const line=(views:string[])=>views.map((v,i)=>i===0||leak?q+v:v).join(" · ")
    const changes:[string,string][]=kept.length===all.length?[]:[[line(all),line(kept)]]
    for(const v of all.filter(v=>!kept.includes(v)))if(run("query",raw,...v.split(" ")).exitCode===0)fail(`stored ${shape}: removed a working ${v}`)
    const formats=leak?["leak"]:nat?["check","blocks","comparison","implementations"]:["check"]
    for(const format of formats) {
      if(format==="check")writeResult({kind:"check",series:[s]})
      else if(leak)writeResult(comparison([{kind:"leak",case:shape,label:shape,leak:{head:{recording:raw,...(shape==="leak-memory"?{native_host:{}}:{})}}}],{}, {variants:[{role:"head",label:"head"}]}))
      else if(format==="blocks")writeResult(comparison([{...synthetic.find(([n])=>n==="exp_syn_blocks")![1].series[0],target_kind:kind,attribution:{head:raw},timing_recordings:[{recording:raw}]}],{blocks:"2"}))
      else writeResult(comparison([series(shape,kind,worse,{target_kind:kind,target_kinds:{base:kind,head:kind},attribution:{base:raw,head:raw}})],{}, {comparison:format==="implementations"?format:"builds"}))
      compareStored(`${shape}/${format}`,exp,changes)
      for(const a of [["query",exp,"summary"],["report",exp,"--format","md"]]) {
        const text=go(a,0), lines=text.split(/\r?\n/).filter(l=>l.includes(q))
        if(!lines.length || (wantWhy&&!lines.some(l=>l.endsWith(line(kept)))))fail(`stored ${shape}/${format}: missing ${line(kept)}: ${text}`)
        for(const l of lines)for(const v of l.slice(l.indexOf(q)+q.length).split(" · ")){go(["query",raw,...v.replace(q,"").split(" ")],0);followed++}
      }
      if(changes.length)console.log(`stored ${shape}/${format}: ${line(all)} -> ${line(kept)}`)
    }
  }
  // Two views of the same result: the command is local to its reader, not its producer.
  // Use new IDs so no fixture left in this run's store can satisfy a missing recording.
  const carry = ["rec_carry_launch","rec_carry_profile","rec_carry_video"]
  const carried = { ...c("carried",carry[0],carry[1]), video:{head:carry[2]}, state:"pass" }
  writeResult({kind:"check",series:[carried]})
  const writeCarried = (dir:string, parent = exp) => {
    for(const id of carry) put(join(dir,id),"meta.json",{id,kind:"block",experiment:parent,probes:"attribution",values:{}})
    cpSync(join(handmade,"cpu-0.cpuprofile"),join(dir,carry[1],"cpu-0.cpuprofile"))
    put(join(dir,carry[2]),"video.json",{file:"video.webm"}); writeFileSync(join(dir,carry[2],"video.webm"),"fixture")
  }
  const verifyCarried = (h:string, from:"copy"|"store"|"missing"|"incomplete") => {
    for(const a of [["query",h,"summary"],["report",h,"--format","md"]]) {
      const text = go(a,0)
      const lines = text.split(/\r?\n/).filter(l=>/^      (rec  |why  |video    )/.test(l))
      if(lines.length!==3) fail(`expected rec/why/video lines: ${text}`)
      for(const [i,line] of lines.entries()) {
        const id = line.trim().split(/\s+/)[1], cmd = line.split(" · strobe ").at(1)
        if(!carry.includes(id)) fail(`lost provenance: ${line}`)
        if(from==="missing" || from==="incomplete") {
          if(cmd || !line.includes(from==="incomplete"?"lacks the data":h===exp?"no longer stored":"not in this copy")) fail(`missing note: ${line}`)
        } else {
          const words = cmd && argvVia(sh?"sh":"pwsh",cmd)
          const expected = from==="copy" ? join(copy,id,"meta.json").replace(/\\/g,"/") : id
          if(!Array.isArray(words) || !words.includes(expected)) fail(`wrong source: ${line}`)
          else { go(words.filter(w=>w!=="--open"),0); followed++ }
        }
      }
    }
  }
  verifyCarried(file,"missing"); writeCarried(join(fakeState,"recordings"),"exp_other"); writeCarried(copy,"exp_other")
  verifyCarried(file,"missing"); verifyCarried(exp,"missing"); writeCarried(join(fakeState,"recordings")); verifyCarried(file,"store")
  compareStored("filmed check",exp)
  if(reference)console.log(`stored byte comparison: ${compared} outputs, ${identical} identical, ${compared-identical} with refused segments removed`)
  writeCarried(copy); verifyCarried(file,"copy")
  for(const id of carry) rmSync(folder(id),{recursive:true}); verifyCarried(exp,"missing"); verifyCarried(file,"copy")
  for(const dir of [copy,join(fakeState,"recordings")]) {
    writeCarried(dir)
    rmSync(join(dir,carry[1],"cpu-0.cpuprofile")); rmSync(join(dir,carry[2],"video.webm"))
    for(const target_kind of ["native","web"]) {
      writeResult({kind:"check",series:[{...carried,target_kind,values:target_kind==="web"?{"http.probes":5}:{}}]})
      verifyCarried(dir===copy?file:exp,"incomplete")
    }
  }
  const side="strobe old", cmp=comparison([series("first","impl",worse,{attribution:{base:carry[1],head:carry[1]}})],{},{comparison:"implementations",variants:[{name:side},{name:"new"}],side_names:[side,"new"]})
  writeResult(cmp)
  for(const h of [file,exp]) for(const a of [["query",h,"summary"],["report",h,"--format","md"]]) {
    const line=go(a,0).split("\n").find(l=>l.startsWith(`       ${side}: `))
    if(!line?.includes(carry[1]) || line.includes("strobe query")) fail(`implementation name bypassed link check: ${line}`)
  }
  // Old metadata can still carry display readings next to a refusal. Neither a renderer nor
  // its backing JSON may resurrect them. Keep the independent main-thread reading.
  const offscreen=join(work,"offscreen"), staleValues={"frames.n":600,"frames.dropped.pct":10,"interaction.p95":1,"display.hz":60,"main.busy":55}
  put(offscreen,"meta.json",{id:"rec_offscreen",kind:"record",headless:true,timing:"perturbed",duration_ms:1000,values:staleValues,batch:batch.map(c=>({...c,values:staleValues}))})
  const summary=JSON.parse(go(["query",offscreen,"summary","--json"],0))
  const md=JSON.parse(go(["report",offscreen,"--format","md","--json"],0)).markdown
  const idle=go(["query",offscreen,"insights"],0)
  const report=JSON.parse(go(["report",offscreen,"--format","html","--json"],0))
  const html=readFileSync(report.path,"utf8"), payload=JSON.parse(html.match(/const STROBE = (.+?);\r?\n/)?.[1] ?? "null")
  for(const m of [summary,payload?.data]) if(!m?.frames_refused || m.values["main.busy"]!==55 || Object.keys(m.values).some(k=>/^(frames|interaction|display)\./.test(k)))fail(`display values escaped in a recording payload: ${JSON.stringify(m?.values)}`)
  for(const m of [summary,payload?.data,JSON.parse(go(["query",offscreen,"summary","--case","first","--json"],0))])for(const c of m.batch)if(JSON.stringify(c.values)!==JSON.stringify({"main.busy":55}))fail(`batch display values escaped: ${JSON.stringify(c)}`)
  if(payload.data.pipeline_frames_available!==false)fail("report offers compositor reports without a trace")
  for(const [label,fields,available] of [["page",{},true],["other engine",{engine:"firefox"},false],["inspector",{wir:true},false],["launch",{launch:true},false],["untraced",{traced:false},false]] as const){
    const dir=join(work,"frame link",label);put(dir,"meta.json",{id:"rec_frame_link",headless:true,...fields});cpSync(join(old,"trace.pftrace"),join(dir,"trace.pftrace"))
    const report=JSON.parse(go(["report",dir,"--format","html","--json"],0)), html=readFileSync(report.path,"utf8")
    if(JSON.parse(html.match(/const STROBE = (.+?);\r?\n/)![1]).data.pipeline_frames_available!==available)fail(`wrong frames link: ${label}`)
  }
  if(!md.includes("refused (Headless") || /\bpresented\b|vsyncs dropped|input → presentation/.test(md) || !idle.includes("55.0% busy") || idle.includes("frames per second")) fail(`headless summary/insights: ${md} ${idle}`)
  for (const kind of ["implementations","video","blocks"]) {
    const dir=join(copy,kind),id=`exp_copy_${kind}`
    const s=kind==="blocks" ? synthetic.find(([n])=>n==="exp_syn_blocks")![1].series[0] : series("first","page",worse,{attribution:{base:prof,head:p2},video:kind==="video"?{base:prof,head:p2}:null})
    put(dir,"result.json",{id,...comparison([s],{}, {comparison:kind==="implementations"?kind:"builds"})}); paths.push(join(dir,"result.json"))
  }
  for(const path of paths){
    const id=JSON.parse(readFileSync(path.endsWith("result.json")?path:join(path,"meta.json"),"utf8")).id
    if(!id) continue
    const stale=new RegExp(`(?:\\bstrobe (?:query|report(?: recording| experiment)?|open|bench --resume)|· query) ${id.replace(/[.*+?^${}()|[\]\\]/g,"\\$&")}(?:\\s|\\)|$)`)
    for(const a of [["query",path,"summary"],["report",path,"--format","md"]]) {
      for(const l of go(a,0).split(/\r?\n/)) if(!/^      (log  |fix: )|^[!i] /.test(l) && stale.test(l)) fail(`stale command in ${a[0]} of ${path}: ${l}`)
    }
  }
  // A real settings file in cwd is for cwd, never for another named artifact. The user's settings
  // supply the fallback. No link or real user configuration is involved.
  const here=join(work,"unpacked"), other=join(work,"another project","rec")
  for (const d of [here,other])put(d,"meta.json",{id:lean,values:{"frames.interval.p50":20}})
  put(here,"strobe.json",{unit:"s"});put(fakeState,"strobe.json",{unit:"ms"})
  const unit=(h:string, want:string, flag:string[]=[])=>{const p=Bun.spawnSync([STROBE,"query",h,"summary",...flag],{cwd:here,env:{...process.env,...fakeEnv},timeout:15000});if(p.exitCode!==0||!p.stdout.toString().includes(`interval p50 ${want}`))fail(`unit for ${h}: expected ${want}`)}
  unit(".","20.0 ms");unit(other,"20.0 ms");put(join(work,"another project"),"strobe.json",{unit:"us"});unit(other,"20,000 µs");unit(other,"0.0200 s",["--unit","s"])
}
console.log(`${followed} follow-up views executed`)
rmSync(work, { recursive: true, force: true })
console.log(`${checked} command parses (sh${pwsh ? " and PowerShell" : ""}): ${failed ? `${failed} failed` : "all run as written"}`)
process.exit(failed ? 1 : 0)
