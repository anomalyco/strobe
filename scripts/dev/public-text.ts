// Known answers for public text (dev, not shipped; SPEC §1 rule 9): nothing another program shows (a covering window's
// title, element or class, an app, busy and moved processes) and no local folder reaches a public surface
// (`report --format pr`, `--format summary`, the card's SVG and alt text, the badge).
//
// Four passes, each on copies of real results from this machine's state:
//   forms           every form in which strobe's notes and refusals name another program or a folder, through a
//                   comparison's warnings, refusals and secondaries, beside a series titled `Resize 13" window` (an
//                   unmatched quote earlier in the report) and app names holding `&`, `|`, `<` and starting with
//                   "process"; each process id must stay, and each path list keep its delimiters;
//   implementations the same forms in an implementation comparison whose bench file is a home path, no `file_repo`;
//   folders         a comparison of two plain folders outside any repository (`/opt/<secret>/v1`): the rerun line;
//   canary          one result of every kind here (comparison, A/A, check, batch, blocks, leak, pixels,
//                   implementations, gaps, refusals, bisect): a covering-window form and home paths appended to every
//                   free-text string and put in place of every path, a block's `variant_git.root` beside it; no
//                   canary may reach any surface. A new field that a renderer publishes without the choke point
//                   (`pr::public`) fails here without a line of this script changing.
//
//   bun scripts/dev/public-text.ts [--strobe <exe>] [--show] [<comparison exp_… or result.json>] [--kinds exp_…,exp_…]
//
// Exit 1 when a token leaked, a process id or a delimiter was lost, or a surface failed.
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "fs"
import { homedir, platform, tmpdir } from "os"
import { basename, join } from "path"

const argv = process.argv.slice(2)
const opt = (n: string) => (argv.includes(`--${n}`) ? argv[argv.indexOf(`--${n}`) + 1] : undefined)
const STROBE = opt("strobe") ?? "strobe"
const win = platform() === "win32"
const state = win
  ? join(process.env.LOCALAPPDATA ?? join(homedir(), "AppData/Local"), "strobe")
  : platform() === "darwin"
    ? join(homedir(), "Library/Application Support/strobe")
    : join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local/share"), "strobe")
const given = argv.find((a, i) => !a.startsWith("--") && !["--strobe", "--kinds"].includes(argv[i - 1]))
const home = homedir()
const user = basename(home)
type J = any

const results = (() => {
  const root = join(state, "experiments")
  return readdirSync(root)
    .filter((d) => existsSync(join(root, d, "result.json")))
    .map((d) => ({ id: d, path: join(root, d, "result.json"), t: statSync(join(root, d, "result.json")).mtimeMs }))
    .sort((a, b) => b.t - a.t)
})()
const read = (p: string): J => JSON.parse(readFileSync(p, "utf8"))
const latest = (ok: (r: J) => boolean) => {
  for (const x of results) {
    try {
      if (ok(read(x.path))) return x.path
    } catch {}
  }
  throw new Error("no such result here: run one, or name a result.json")
}
const comparisonSrc = given ? (existsSync(given) ? given : join(state, "experiments", given, "result.json")) : latest((r) => r.kind === "experiment" && r.comparison !== "implementations" && (r.series ?? []).some((s: J) => s.kind === "runs"))

// ---------- the forms ----------

// Each form as the code that writes it does (file:line in the comment), with SECRET tokens where a name goes.
const forms: [string, string][] = [
  // target/a11y/input.rs, xtest.rs, wl_input.rs: a title over 90 characters, then its pid.
  ["WindowHidden", 'another window covers the point (1200, 800): "Private Chat — DM with SECRET01 about Project X and the rest of it" of process 4120'],
  // target/a11y/cg_input.rs + ax.rs: a window, and an app with no window title.
  ["WindowHidden", 'another app\'s window covers the point (10, 20): window "SECRET02 inbox" of process 4121'],
  ["WindowHidden", "another app's window covers the point (10, 20): a window of SECRET03 Mail of process 4122"],
  // driver/native.rs aim() + uia.rs / ax.rs: an element of another process over the target's.
  ["WindowHidden", '"Save" at (10, 20) is under pane "SECRET04 notes" (key "SECRET05") of process 4123: keep it uncovered'],
  // host/x11.rs: a title holding `; `, then (pid N).
  ["WindowHidden", 'the window is not on top at (100, 200): "SECRET06; draft" (pid 4124) is over it'],
  // host/wayland.rs: the region form.
  ["WindowHidden", '"SECRET07 \\"quoted\\" title" of process 4125 covers the region'],
  // target/a11y/input.rs: a window class.
  ["WindowHidden", 'another window covers the point (5, 5): a "SECRET08Class" window of process 4126'],
  // validity::named_work: names holding parentheses and `;`.
  ["Background", "other work ran on the measured CPUs during 2 block(s): pair 1 head: up to 94% of a core (busiest elsewhere: SECRET09 (x64).exe 421%, SECRET10).exe 273%, SECRET11;b) 45%, other 30%); pair 2 head: up to 50% of a core"],
  // bench.rs ReservedCpus::text: count-1 names ending in `)`, then each way the note goes on.
  ["ReservedCpus", "other processes of this user kept off CPUs 0-15 during 2 timed blocks: 3 moved per block (SECRET12 Helper (Renderer), SECRET13 Helper (GPU), SECRET14.exe); 95% put back after"],
  ["ReservedCpus", "other processes of this user kept off CPUs 0-15 during 2 timed blocks: 9 moved per block (SECRET15.exe ×2, x).exe, …), 3 more started during blocks and moved within 200 ms; 2 could not be moved: SECRET16.exe (elevated); 100% put back after"],
  // Paths: a file URL, `\\?\` home, a backslash home without a trailing part, lowercase drives, this machine's home.
  ["VariantPlacement", "failed loading file:///Users/SECRET17/app/index.html and file:///C:/Users/SECRET18/app/x.js"],
  ["VariantPlacement", `cwd \\\\?\\${home}; in C:\\Users\\SECRET19; c:\\users\\SECRET20\\proj\\x; /home/SECRET21; ${home.toLowerCase()}${win ? "\\" : "/"}x`],
  // Review 2, finding 1: app names a table cell escapes (`&`, `|`, `<`) and one that starts with the word "process".
  ["WindowHidden", "another app's window covers the point (10, 20): a window of SECRET22 Q&A Chat of process 4127"],
  ["WindowHidden", "another app's window covers the point (10, 20): a window of SECRET23 Foo | Bar of process 4128"],
  ["WindowHidden", "another app's window covers the point (10, 20): a window of SECRET24 <Viewer> of process 4129"],
  ["WindowHidden", "another app's window covers the point (10, 20): a window of process monitor SECRET25 of process 4130"],
  // Review 2, finding 3: adjacent paths in a list, a path in brackets, a UNC, WSL and ostree home, a sentence's stop.
  ["VariantPlacement", "PATH=/home/SECRET26:/home/SECRET27 dirs=/Users/SECRET28,/Users/SECRET29 at [C:\\Users\\SECRET30] and /home/SECRET31. Then more"],
  ["VariantPlacement", "share \\\\fileserver\\Users\\SECRET32\\proj; wsl /mnt/c/Users/SECRET33/proj; silverblue /var/home/SECRET34/proj"],
  // main 964e825: a held launch's hint names the app's path (target/isolate.rs), and a page left hidden after a run.
  ["Launch", `open /Users/SECRET35/Apps/Tool.app once from the Finder and allow it (if macOS still refuses, allow it in System Settings > Privacy & Security > Open Anyway), then run again`],
  ["WindowHidden", 'the frame after the run: the page became hidden (document.visibilityState is "hidden": a popup or another window over it, minimized, a background tab, or the display off), and a hidden page draws no frames'],
]
const tokens = [...new Set(forms.flatMap(([, t]) => t.match(/SECRET\d+/g) ?? []))]
if (user.length > 2) tokens.push(user)
// What must stay: the process ids (what a reader acts on), and the delimiters between scrubbed paths.
const pids = ["4120", "4121", "4122", "4123", "4127", "4128", "4129", "4130"]
// And the report around an unmatched `"`: never collapsed into `"…"` with the lines between.
const kept = ["PATH=~:~", "dirs=~,~", "[~]", "~. Then", '**Resize 13" window', 'fix 15" layout placed its harness']

const dir = mkdtempSync(join(tmpdir(), "strobe-public-"))
const run = (args: string[], env: Record<string, string> = {}) => {
  const p = Bun.spawnSync([STROBE, ...args], { stdout: "pipe", stderr: "pipe", env: { ...process.env, ...env } })
  return { out: p.stdout.toString(), err: p.stderr.toString(), code: p.exitCode ?? 0 }
}

// Every public surface of one result: [surface, text], or a refusal for a kind that has no such surface.
function surfaces(name: string, r: J): [string, string][] {
  let file = join(dir, `${name}.json`)
  writeFileSync(file, JSON.stringify(r))
  // A bisect's report is read by its handle: from a state folder of this run's own.
  let env: Record<string, string> = {}
  if (r.kind === "bisect") {
    const fake = join(dir, `state-${name}`)
    mkdirSync(join(fake, "strobe", "experiments", r.id), { recursive: true })
    writeFileSync(join(fake, "strobe", "experiments", r.id, "result.json"), JSON.stringify(r))
    env = win ? { LOCALAPPDATA: fake } : { XDG_DATA_HOME: fake }
    file = r.id
  }
  const out: [string, string][] = []
  const one = (f: string, args: string[], pick: (o: { out: string }) => string) => {
    const o = run(["report", file, "--format", f, ...args], env)
    // A kind with no such surface says so (exit 2): a check or pixels run decides nothing, a bisect's result read
    // from its path is no experiment's (`--json` puts the refusal on stdout).
    if (o.code === 2 && /decides nothing|compares nothing|no --format|draws|not a comparison|writes an experiment's result/.test(o.err + o.out)) return
    out.push([f, o.code === 0 ? pick(o) : `(exit ${o.code}: ${o.err.trim().split("\n")[0]})`])
  }
  one("pr", [], (o) => o.out)
  one("summary", [], (o) => o.out)
  const svg = join(dir, `${name}-card.svg`)
  // The JSON's own `path` is where this machine wrote the card (local, never published): only its alt text counts.
  one("card", ["--json", "--out", svg], (o) => String(JSON.parse(o.out || "{}").alt ?? ""))
  if (existsSync(svg)) out.push(["card svg", readFileSync(svg, "utf8")])
  const badge = join(dir, `${name}-badge.svg`)
  one("badge", ["--out", badge], () => "")
  if (existsSync(badge)) out.push(["badge svg", readFileSync(badge, "utf8")])
  return out
}

let bad = 0
const line = (text: string, t: string) => (text.split("\n").find((l) => l.includes(t)) ?? "").replaceAll(user, "<user>").slice(0, 240)
function judge(pass: string, got: [string, string][], secrets: string[], must: { pids?: boolean; kept?: boolean } = {}) {
  let leaks = 0
  for (const [name, text] of got) {
    if (text.startsWith("(exit")) {
      console.log(`FAIL ${pass} ${name}: ${text}`)
      bad++
      continue
    }
    for (const t of secrets.filter((t) => text.includes(t))) {
      console.log(`LEAK ${pass} ${name}: ${t === user ? "<this user's name>" : t} in: ${line(text, t)}`)
      leaks++
    }
    if (must.pids && (name === "pr" || name === "summary")) {
      const lost = pids.filter((p) => !text.includes(p))
      if (lost.length) console.log(`NOTE ${pass} ${name}: process ids ${lost.join(", ")} not shown (cut or dropped)`)
    }
    if (must.kept && name === "pr") {
      const lost = kept.filter((k) => !text.includes(k))
      if (lost.length) {
        console.log(`FAIL ${pass} ${name}: text lost (a delimiter, or lines collapsed into "…"): ${lost.join("  ")} not in the text`)
        bad++
      }
    }
    if (argv.includes("--show") && name === "pr") console.log(`--- ${pass} pr\n${text.replaceAll(user, "<user>")}`)
  }
  bad += leaks
  console.log(`${pass}: ${got.length} surfaces, ${leaks ? `${leaks} leaked` : "none leaked"}`)
}

// Every form in a result's warnings, its comparisons' refusals and its secondaries.
function withForms(r: J, kinds: string[] = ["runs"]) {
  r.id = r.id ?? "exp_public"
  // One unmatched `"` in the report, lines before the window forms (review 2's `fix 15" layout`).
  r.warnings = [{ id: "VariantPlacement", level: "warn", text: 'the build of fix 15" layout placed its harness' }, ...forms.map(([id, text]) => ({ id, level: "warn", text }))]
  const series = (r.series ?? []).filter((s: J) => kinds.includes(s.kind))
  for (const [k, s] of series.entries()) {
    const why = forms[[0, 2, 12, 13, 14, 15, 3, 4, 5, 6][k % 10]][1]
    s.primary = { metric: s.primary?.metric ?? "cpu.work", decided: false, verdict: `refused (WindowHidden): ${why}`, verdict_kind: "refused", refusal: { validator: "WindowHidden", summary: why, fix: "keep the app's window uncovered" } }
    for (const t of s.secondaries ?? []) {
      t.verdict = `refused (WindowHidden): ${forms[3][1]}`
      t.verdict_kind = "refused"
      t.refusal = { validator: "WindowHidden", summary: forms[3][1] }
    }
    s.warnings = forms.slice(4).map(([id, text]) => ({ id, level: "warn", text }))
  }
  return series
}

// ---------- forms ----------
{
  const r = read(comparisonSrc)
  const series = withForms(r)
  // An unmatched `"` on an earlier line of the report: a series title.
  if (series[0]) {
    series[0].case = 'Resize 13" window'
    series[0].label = `Resize 13" window · ${series[0].target ?? "page"}`
  }
  judge("forms", surfaces("forms", r), tokens, { pids: true, kept: true })
}

// ---------- implementations ----------
{
  const src = latest((r) => r.comparison === "implementations" && (r.series ?? []).some((s: J) => s.kind === "runs"))
  const r = read(src)
  delete r.file_repo
  r.file = win ? `C:\\Users\\SECRETIMPL\\secret\\app.bench.ts` : `/home/SECRETIMPL/secret/app.bench.ts`
  if (r.args?.pos) r.args.pos[0] = r.file
  withForms(r, ["runs", "gap", "refused"])
  for (const s of r.series ?? []) {
    if (s.kind === "gap") s.gap = { ...(s.gap ?? {}), reason: forms[0][1] }
    if (s.primary) s.primary.care_note = forms[2][1]
  }
  judge("implementations", surfaces("impl", r), [...tokens, "SECRETIMPL"])
}

// ---------- folders ----------
{
  const r = read(comparisonSrc)
  r.variants = ["base", "head"].map((role, i) => ({ role, name: `v${i + 1}`, label: `v${i + 1}`, spec: `/opt/SECRETCORP/v${i + 1}`, kind: "dir", commit: null, dirty: false, target: null }))
  r.args = { ...(r.args ?? {}), flags: { ...(r.args?.flags ?? {}), base: "/opt/SECRETCORP/v1", head: "/opt/SECRETCORP/v2" } }
  judge("folders", surfaces("folders", r), ["SECRETCORP"])
}

// ---------- canary ----------
const CANARIES = ["CANARYW", "CANARYU", "CANARYH", "CANARYF", "CANARYS", "CANARYV", "CANARYM", "CANARYO", "CANARYROOT"]
const WINDOW = 'another window covers the point (1, 2): "CANARYW title" of process 4999'
const HOMES = ["C:\\Users\\CANARYU\\proj", "/home/CANARYH/proj", "file:///Users/CANARYF/a.html", "\\\\srv\\Users\\CANARYS\\x", "/var/home/CANARYV/x", "/mnt/c/Users/CANARYM/x"]
const looksPath = (s: string) => /^(?:[A-Za-z]:[\\/]|\/|\\\\|file:\/\/)/.test(s)
// Fields read as identifiers or numbers, which no message is: left as they are.
const STRUCTURAL = new Set(["id", "kind", "metric", "unit", "verdict_kind", "validator", "level", "state", "role", "engine", "target_kind", "comparison", "better", "created", "commit", "sha", "digest", "label_commit"])
function plant(v: J, key = "", parent: J = null): J {
  if (typeof v === "string") {
    if (STRUCTURAL.has(key)) return v
    if (looksPath(v)) {
      // A plain folder's spec is no home: what only the rerun line and the variant's tag must keep out.
      if (key === "spec" && parent?.kind === "dir") return `/opt/CANARYO/${basename(v) || "v"}`
      const h = HOMES[v.length % HOMES.length]
      return `${h}${h.includes("\\") ? "\\" : "/"}${basename(v.replace(/\\/g, "/")) || "x"}`
    }
    return v.includes(" ") && v.length >= 8 ? `${v}; ${WINDOW}; at ${HOMES.join(" and ")}` : v
  }
  if (Array.isArray(v)) return v.map((x) => plant(x, key, parent))
  if (v && typeof v === "object") {
    const o: J = {}
    for (const [k, x] of Object.entries(v)) o[k] = plant(x, k, v)
    if (o.kind === "dir" && "spec" in o) o.root = "/opt/CANARYROOT/v"
    return o
  }
  return v
}
const kindOf = (r: J) =>
  [r.kind, r.comparison ?? "", r.plan?.aa || r.args?.flags?.aa ? "aa" : "", r.args?.flags?.batch ? "batch" : "", [...new Set((r.series ?? []).map?.((s: J) => s.kind) ?? [])].sort().join(",")].join("/")
const picked = new Map<string, string>()
for (const id of (opt("kinds") ?? "").split(",").filter(Boolean)) picked.set(id, join(state, "experiments", id, "result.json"))
if (!opt("kinds")) {
  for (const x of results) {
    try {
      const k = kindOf(read(x.path))
      if (![...picked.keys()].some((p) => p === k)) picked.set(k, x.path)
    } catch {}
  }
}
let canaryLeaks = 0
for (const [k, src] of picked) {
  const r = plant(read(src))
  const before = bad
  const got = surfaces(`canary-${picked.size > 0 ? [...picked.keys()].indexOf(k) : 0}`, r)
  for (const [name, text] of got) {
    if (text.startsWith("(exit")) {
      console.log(`FAIL canary ${basename(join(src, ".."))} (${k}) ${name}: ${text}`)
      bad++
      continue
    }
    for (const t of CANARIES.filter((t) => text.includes(t))) {
      console.log(`LEAK canary ${basename(join(src, ".."))} (${k}) ${name}: ${t} in: ${line(text, t)}`)
      bad++
      canaryLeaks++
    }
  }
  if (bad === before) console.log(`canary ${basename(join(src, ".."))} (${k || "?"}): ${got.map(([n]) => n).join(", ") || "no surface"}: none leaked`)
}
// A block's meta holds the folder its variant ran from (`variant_git.root`), a local path: a recording has no public
// surface (every public format refuses it), and an experiment's surfaces carry no root (`plant` puts one beside every
// variant above).
{
  const fake = join(dir, "state")
  const env: Record<string, string> = win ? { LOCALAPPDATA: fake } : { XDG_DATA_HOME: fake }
  const id = "rec_canary"
  mkdirSync(join(fake, "strobe", "recordings", id), { recursive: true })
  writeFileSync(join(fake, "strobe", "recordings", id, "meta.json"), JSON.stringify({ id, kind: "block", variant: "head", variant_label: "v2", variant_git: { spec: "/opt/CANARYO/v2", commit: null, dirty: false, root: "/opt/CANARYROOT/v2" }, runs: 3, values: {}, created: new Date().toISOString(), valid: true }))
  const said: string[] = []
  for (const f of ["pr", "summary", "card", "badge"]) {
    const o = run(["report", id, "--format", f, "--out", join(dir, `rec.${f}`)], env)
    const leaked = CANARIES.filter((t) => (o.out + (existsSync(join(dir, `rec.${f}`)) ? readFileSync(join(dir, `rec.${f}`), "utf8") : "")).includes(t))
    if (o.code === 0 || leaked.length) {
      console.log(`FAIL canary recording --format ${f}: ${o.code === 0 ? "wrote a public surface of a recording" : ""} ${leaked.join(", ")}`)
      bad++
    }
    said.push(`${f} exit ${o.code}`)
  }
  console.log(`canary recording ${id} (variant_git.root): no public surface (${said.join(", ")})`)
}

rmSync(dir, { recursive: true, force: true })
console.log(`${tokens.length} names and folders, ${picked.size} kinds of result under canaries: ${bad ? `${bad} failed` : "none leaked"}`)
process.exit(bad ? 1 : 0)
