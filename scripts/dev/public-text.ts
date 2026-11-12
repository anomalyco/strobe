// Known answers for public text (dev, not shipped; SPEC §1 rule 9): every form in which strobe's notes and refusals
// name another program (a covering window's title, element or class, an app, busy and moved processes) or a person's
// folder, fed through each public surface (`report --format pr`, `--format summary`, the card's SVG and alt text) from a
// copy of a real comparison's result.json. Each SECRET token must be gone; each process id must stay.
//
//   bun scripts/dev/public-text.ts [--strobe <exe>] [<comparison exp_… or result.json>]
//
// Exit 1 when a token leaked.
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "fs"
import { homedir, platform, tmpdir } from "os"
import { basename, join } from "path"

const argv = process.argv.slice(2)
const opt = (n: string) => (argv.includes(`--${n}`) ? argv[argv.indexOf(`--${n}`) + 1] : undefined)
const STROBE = opt("strobe") ?? "strobe"
const state =
  platform() === "win32"
    ? join(process.env.LOCALAPPDATA ?? join(homedir(), "AppData/Local"), "strobe")
    : platform() === "darwin"
      ? join(homedir(), "Library/Application Support/strobe")
      : join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local/share"), "strobe")
const given = argv.find((a, i) => !a.startsWith("--") && argv[i - 1] !== "--strobe")
// A comparison: the latest experiment whose series ran timed pairs.
const pick = () => {
  const root = join(state, "experiments")
  const dirs = readdirSync(root).filter((d) => existsSync(join(root, d, "result.json"))).sort((a, b) => statSync(join(root, b, "result.json")).mtimeMs - statSync(join(root, a, "result.json")).mtimeMs)
  for (const d of dirs) {
    const r = JSON.parse(readFileSync(join(root, d, "result.json"), "utf8"))
    if (r.kind === "experiment" && r.comparison !== "implementations" && (r.series ?? []).some((s: any) => s.kind === "runs")) return join(root, d, "result.json")
  }
  throw new Error("no comparison here: run one, or name a result.json")
}
const src = given ? (existsSync(given) ? given : join(state, "experiments", given, "result.json")) : pick()
const home = homedir()
const user = basename(home)

// Each form, as the code that writes it does (file:line in the comment), with SECRET tokens where a name goes.
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
  ["VariantPlacement", `cwd \\\\?\\${home}; in C:\\Users\\SECRET19; c:\\users\\SECRET20\\proj\\x; /home/SECRET21; ${home.toLowerCase()}${platform() === "win32" ? "\\" : "/"}x`],
]
const tokens = [...new Set(forms.flatMap(([, t]) => t.match(/SECRET\d+/g) ?? []))]
if (user.length > 2) tokens.push(user)

const r = JSON.parse(readFileSync(src, "utf8"))
r.id = r.id ?? "exp_public"
r.warnings = forms.map(([id, text]) => ({ id, level: "warn", text }))
// Every comparison refused, the first with the longest window form: the card's calm line (drawn when nothing
// decided) and the PR's refused rows.
for (const [k, s] of r.series.filter((s: any) => s.kind === "runs").entries()) {
  const why = forms[k % 7][1]
  s.primary = { metric: s.primary?.metric ?? "cpu.work", decided: false, verdict: `refused (WindowHidden): ${why}`, verdict_kind: "refused", refusal: { validator: "WindowHidden", summary: why, fix: "keep the app's window uncovered" } }
  for (const t of s.secondaries ?? []) {
    t.verdict = `refused (WindowHidden): ${forms[3][1]}`
    t.verdict_kind = "refused"
    t.refusal = { validator: "WindowHidden", summary: forms[3][1] }
  }
  s.warnings = forms.slice(4).map(([id, text]) => ({ id, level: "warn", text }))
}
const dir = mkdtempSync(join(tmpdir(), "strobe-public-"))
const file = join(dir, "result.json")
writeFileSync(file, JSON.stringify(r))
const run = (args: string[]) => {
  const p = Bun.spawnSync([STROBE, ...args], { stdout: "pipe", stderr: "pipe" })
  return { out: p.stdout.toString(), err: p.stderr.toString(), code: p.exitCode ?? 0 }
}
const surfaces: [string, string][] = []
for (const f of ["pr", "summary"]) {
  const o = run(["report", file, "--format", f])
  surfaces.push([f, o.code === 0 ? o.out : `(exit ${o.code}: ${o.err.trim()})`])
}
const svg = join(dir, "card.svg")
const card = run(["report", file, "--format", "card", "--json", "--out", svg])
// The JSON's own `path` is where this machine wrote the card (local, never published): only its alt text counts.
surfaces.push(["card alt", card.code === 0 ? String(JSON.parse(card.out || "{}").alt ?? "") : `(exit ${card.code}: ${card.err.trim()})`])
surfaces.push(["card svg", existsSync(svg) ? readFileSync(svg, "utf8") : "(no card)"])
let leaks = 0
for (const [name, text] of surfaces) {
  if (text.startsWith("(exit")) {
    console.log(`FAIL ${name}: ${text}`)
    leaks++
    continue
  }
  const found = tokens.filter((t) => text.includes(t))
  for (const t of found) {
    const line = text.split("\n").find((l) => l.includes(t)) ?? ""
    console.log(`LEAK ${name}: ${t === user ? "<this user's name>" : t} in: ${line.replaceAll(user, "<user>").slice(0, 220)}`)
  }
  leaks += found.length
  // The process ids stay: what a reader needs to act on (`of process 4120`).
  const pids = ["4120", "4121", "4123"].filter((p) => (name === "pr" || name === "summary") && !text.includes(p))
  if (pids.length) console.log(`NOTE ${name}: process ids ${pids.join(", ")} not shown (cut or dropped)`)
}
rmSync(dir, { recursive: true, force: true })
console.log(`${tokens.length} names and folders through ${surfaces.length} surfaces: ${leaks ? `${leaks} leaked` : "none leaked"}`)
process.exit(leaks ? 1 : 0)
