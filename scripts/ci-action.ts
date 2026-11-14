// The steps of strobe's GitHub Action (action.yml, SPEC §26):
//   bun scripts/ci-action.ts resolve   which strobe, where it is cached
//   bun scripts/ci-action.ts install   that strobe from npm (or the given binary), and its engine's cache key
//   bun scripts/ci-action.ts setup     strobe up, and whether a browser is there
//   bun scripts/ci-action.ts browser   Playwright's Chromium, only where no browser was found
//   bun scripts/ci-action.ts run       the benches, the step summary, the PR block, the artifacts
// Every input arrives in the environment as INPUT_<NAME> (action.yml puts it there), never in a shell
// line: an input is data, not code.
import { appendFileSync, chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import { basename, dirname, isAbsolute, join, resolve } from "node:path"

const input = (name: string) => (process.env[`INPUT_${name.toUpperCase().replace(/-/g, "_")}`] ?? "").trim()
const env = (name: string) => process.env[name] ?? ""

// A workflow command's text: GitHub reads %, CR and LF as escapes.
const escape = (s: string) => s.replace(/%/g, "%25").replace(/\r/g, "%0D").replace(/\n/g, "%0A")
const warn = (s: string) => console.log(`::warning title=strobe::${escape(scrubText(s))}`)
const error = (s: string) => console.log(`::error title=strobe::${escape(scrubText(s))}`)
function fail(s: string): never {
  error(s)
  process.exit(1)
}

function output(name: string, value: string) {
  const file = env("GITHUB_OUTPUT")
  if (!file) return console.log(`${name}=${value}`)
  const end = `strobe_${crypto.randomUUID()}`
  appendFileSync(file, `${name}<<${end}\n${value}\n${end}\n`)
}

function setEnv(name: string, value: string) {
  const file = env("GITHUB_ENV")
  const end = `strobe_${crypto.randomUUID()}`
  if (file) appendFileSync(file, `${name}<<${end}\n${value}\n${end}\n`)
  process.env[name] = value
}

function oneOf(name: string, allowed: string[], fallback: string): string {
  const v = input(name) || fallback
  if (!allowed.includes(v)) fail(`input ${name}: ${JSON.stringify(v)} is not one of ${allowed.join(", ")}`)
  return v
}

const flag = (name: string) => oneOf(name, ["true", "false"], "false") === "true"

// npm's name for this runner's OS and CPU, as strobe's release publishes its binaries.
function platform(): string {
  const os = { win32: "win32", darwin: "darwin", linux: "linux" }[process.platform as string]
  const cpu = { x64: "x64", arm64: "arm64" }[process.arch as string]
  if (!os || !cpu) fail(`strobe has no release for ${process.platform} ${process.arch}`)
  return `${os}-${cpu}`
}

const exe = (name: string) => (process.platform === "win32" ? `${name}.exe` : name)
const workspace = () => env("GITHUB_WORKSPACE") || process.cwd()
const cacheRoot = () => join(env("RUNNER_TOOL_CACHE") || env("RUNNER_TEMP") || join(workspace(), ".."), "strobe")

// The pull_request_target event runs a fork's code with the base repository's secrets and a token that
// can write: a bench runs the pull request's code, so the action never runs there.
function guard() {
  if (env("GITHUB_EVENT_NAME") === "pull_request_target")
    fail("strobe's action refuses pull_request_target: a bench runs the pull request's code, and that event hands it the base repository's secrets and a token that can write. Use pull_request (fork PRs get a read-only token and no secrets)")
}

function event(): any {
  try {
    return JSON.parse(readFileSync(env("GITHUB_EVENT_PATH"), "utf8"))
  } catch {
    return {}
  }
}

async function registry(path: string): Promise<any> {
  const url = `https://registry.npmjs.org/${path}`
  for (let attempt = 1; ; attempt++) {
    try {
      const r = await fetch(url, { headers: { accept: "application/json" } })
      if (r.status === 404) fail(`npm has no ${decodeURIComponent(path)}`)
      if (!r.ok) throw new Error(`HTTP ${r.status}`)
      return await r.json()
    } catch (e) {
      if (attempt === 3) fail(`${url}: ${e}`)
      await Bun.sleep(2000 * attempt)
    }
  }
}

// The strobe version to install: the action's own (its Cargo.toml), the project's (@hona/strobe in its
// node_modules, else the newest release its package.json's range allows) or the one given.
async function version(dir: string): Promise<string> {
  const v = input("version") || "action"
  if (v === "action") {
    const cargo = readFileSync(join(import.meta.dir, "..", "Cargo.toml"), "utf8")
    return cargo.match(/^version = "([^"]+)"/m)?.[1] ?? fail("the action's Cargo.toml names no version")
  }
  if (v === "project") {
    for (let d = dir; ; d = dirname(d)) {
      const installed = join(d, "node_modules", "@hona", "strobe", "package.json")
      if (existsSync(installed)) return JSON.parse(readFileSync(installed, "utf8")).version
      const pkg = join(d, "package.json")
      if (existsSync(pkg)) {
        const p = JSON.parse(readFileSync(pkg, "utf8"))
        const range: string | undefined = p.devDependencies?.["@hona/strobe"] ?? p.dependencies?.["@hona/strobe"]
        if (range) {
          const all = Object.keys((await registry("@hona%2fstrobe")).versions ?? {})
          const best = all.filter((x) => Bun.semver.satisfies(x, range)).sort(Bun.semver.order).at(-1)
          return best ?? fail(`${pkg}: no @hona/strobe release satisfies ${range}`)
        }
      }
      if (dirname(d) === d || resolve(d) === resolve(workspace())) break
    }
    fail(`version: project: no @hona/strobe in ${dir}'s package.json or node_modules (bun add -d @hona/strobe)`)
  }
  if (v === "latest" || /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(v)) return v
  fail(`input version: ${JSON.stringify(v)} is not action, project, latest or a version (0.29.0)`)
}

function workdir(): string {
  const d = resolve(workspace(), input("working-directory") || ".")
  if (!existsSync(d)) fail(`working-directory ${d} does not exist`)
  return d
}

function givenBinary(): string | null {
  const b = input("strobe-binary")
  if (!b) return null
  let p = isAbsolute(b) ? b : resolve(workspace(), b)
  if (!existsSync(p) && existsSync(p + ".exe")) p += ".exe"
  if (existsSync(p) && statSync(p).isDirectory()) p = join(p, exe("strobe"))
  if (!existsSync(p)) fail(`strobe-binary ${p} does not exist`)
  return p
}

async function resolveStep() {
  guard()
  oneOf("mode", ["check", "compare"], "check")
  oneOf("fail-on", ["worse", "unclear", "never"], "worse")
  oneOf("update-pr", ["auto", "true", "false"], "auto")
  oneOf("upload-artifacts", ["on-failure", "always", "never"], "on-failure")
  oneOf("install-browser", ["auto", "false"], "auto")
  flag("upload-recordings")
  oneOf("headless", ["true", "false"], "true")
  if (!input("bench")) fail("input bench: give one or more bench files (comma- or newline-separated)")
  const plat = platform()
  if (givenBinary()) {
    output("cache-key", "")
    output("cache-path", "")
    return
  }
  const v = await version(workdir())
  const meta = await registry(`@hona%2fstrobe-${plat}/${v}`)
  output("version", meta.version)
  output("cache-key", `strobe-${plat}-${meta.version}`)
  output("cache-path", join(cacheRoot(), meta.version, plat))
}

// A ustar archive's regular files, as (path, bytes).
function untar(tar: Uint8Array): [string, Uint8Array][] {
  const text = (a: number, b: number, h: Uint8Array) => new TextDecoder().decode(h.subarray(a, b)).replace(/\0.*$/s, "")
  const files: [string, Uint8Array][] = []
  for (let at = 0; at + 512 <= tar.length; ) {
    const h = tar.subarray(at, at + 512)
    if (h.every((b) => b === 0)) break
    const size = parseInt(text(124, 136, h).trim() || "0", 8)
    const name = text(0, 100, h)
    const prefix = text(345, 500, h)
    const body = at + 512
    if ((h[156] === 48 || h[156] === 0) && body + size <= tar.length) files.push([prefix ? `${prefix}/${name}` : name, tar.slice(body, body + size)])
    at = body + Math.ceil(size / 512) * 512
  }
  return files
}

// The platform package's tarball, checked against the registry's sha512 integrity on every run (a
// cached copy too) before its binaries are unpacked.
async function installNpm(v: string, dir: string): Promise<string> {
  const plat = platform()
  const name = `@hona/strobe-${plat}`
  const meta = await registry(`${name.replace("/", "%2f")}/${v}`)
  const integrity: string = meta.dist?.integrity ?? ""
  if (!integrity.startsWith("sha512-")) fail(`${name}@${v}: the registry gives no sha512 integrity`)
  const sha512 = (b: Uint8Array) => `sha512-${new Bun.CryptoHasher("sha512").update(b).digest("base64")}`
  mkdirSync(dir, { recursive: true })
  const tgz = join(dir, "package.tgz")
  let bytes = existsSync(tgz) ? new Uint8Array(readFileSync(tgz)) : null
  if (bytes && sha512(bytes) !== integrity) {
    warn(`the cached ${name}@${v} does not match the registry's integrity; downloading it again`)
    bytes = null
  }
  if (!bytes) {
    const r = await fetch(meta.dist.tarball)
    if (!r.ok) fail(`download ${meta.dist.tarball}: HTTP ${r.status}`)
    bytes = new Uint8Array(await r.arrayBuffer())
    if (sha512(bytes) !== integrity) fail(`${name}@${v}: the tarball's sha512 does not match the registry's integrity`)
    writeFileSync(tgz, bytes)
  }
  const files = untar(Bun.gunzipSync(bytes))
  for (const n of ["strobe", "strobew"]) {
    // Each binary written afresh from the checked tarball: no cached copy (or one the package lacks) stays.
    const to = join(dir, exe(n))
    rmSync(to, { force: true })
    const f = files.find(([p]) => p === `package/${exe(n)}`)
    if (!f) continue
    writeFileSync(to, f[1])
    if (process.platform !== "win32") chmodSync(to, 0o755)
  }
  const main = join(dir, exe("strobe"))
  if (!existsSync(main)) fail(`${name}@${v} holds no ${exe("strobe")}`)
  return main
}

async function strobe(args: string[], opts: { cwd?: string; capture?: boolean; env?: Record<string, string> } = {}): Promise<{ code: number; stdout: string }> {
  return run([env("STROBE_ACTION_EXE") || "strobe", ...args], opts)
}

// A program, run with an argument list. The job's log is readable by anyone who can read the repository's
// Actions, so what it prints there goes through `scrubText` line by line (a wait's busiest processes, a
// covering window's title, home paths); stdout is kept whole, unscrubbed, when captured (JSON to parse).
async function run(cmd: string[], opts: { cwd?: string; capture?: boolean; env?: Record<string, string> } = {}): Promise<{ code: number; stdout: string }> {
  const p = Bun.spawn(cmd, { cwd: opts.cwd, stdin: "ignore", stdout: "pipe", stderr: "pipe", env: { ...process.env, ...opts.env } })
  const relay = async (stream: ReadableStream<Uint8Array>, to: NodeJS.WriteStream) => {
    const decoder = new TextDecoder()
    let rest = ""
    for await (const chunk of stream) {
      const lines = (rest + decoder.decode(chunk, { stream: true })).split(/\r?\n|\r/)
      rest = lines.pop() ?? ""
      for (const line of lines) to.write(`${scrubText(line)}\n`)
    }
    if (rest) to.write(`${scrubText(rest)}\n`)
  }
  const [stdout] = await Promise.all([
    opts.capture === false ? relay(p.stdout, process.stdout).then(() => "") : new Response(p.stdout).text(),
    relay(p.stderr, process.stderr),
  ])
  return { code: (await p.exited) ?? 1, stdout }
}

function json(text: string): any {
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

async function installStep() {
  const given = givenBinary()
  const main = given ?? (await installNpm(env("STROBE_ACTION_VERSION"), env("STROBE_ACTION_CACHE")))
  // Later steps of the job run this strobe as `strobe`.
  if (env("GITHUB_PATH")) appendFileSync(env("GITHUB_PATH"), `${dirname(main)}\n`)
  setEnv("STROBE_ACTION_EXE", main)
  const v = Bun.spawnSync([main, "--version"], { stdin: "ignore" })
  if (v.exitCode !== 0) fail(`${main} does not run: ${v.stderr.toString().trim()}`)
  console.log(v.stdout.toString().trim())
  // The engine (trace_processor) has its own version, which the binary names; its folder is cached apart.
  const status = json((await strobe(["status", "--fast", "--json"])).stdout)
  const engine = status?.engine
  if (engine?.version && engine?.path) {
    output("engine-key", `strobe-engine-${platform()}-${engine.version}`)
    output("engine-path", dirname(engine.path))
  } else {
    output("engine-key", "")
    warn("strobe status named no engine; it downloads on first use, uncached")
  }
}

const PLAYWRIGHT = "1.65.0"

async function setupStep() {
  // Only the engine: nothing installed into the runner user's profile (a self-hosted machine's own strobe,
  // PATH and skill stay as they were), no privilege prompt, daemon, MCP registration or calibration.
  const up = await strobe(["up", "--skip", "self,skill,daemon,mcp,deep,calibrate"], { capture: false })
  if (up.code !== 0) fail(`strobe up failed (exit ${up.code}); its lines above say which step`)
  const check = json((await strobe(["up", "--check", "--json"])).stdout)
  const browser = check?.browser?.ok === true
  const wanted = oneOf("install-browser", ["auto", "false"], "auto") === "auto"
  output("browser-missing", String(!browser && wanted))
  if (!browser && !wanted) warn("no browser found and install-browser is false: page targets will fail (install Chrome, or set STROBE_BROWSER)")
}

// Into Playwright's own folder (PLAYWRIGHT_BROWSERS_PATH, else the user's cache), where strobe looks.
async function browserStep() {
  const p = await run(["bunx", `playwright@${PLAYWRIGHT}`, "install", "chromium"], { capture: false })
  if (p.code !== 0) fail(`bunx playwright@${PLAYWRIGHT} install chromium failed (exit ${p.code})`)
  const check = json((await strobe(["up", "--check", "--json"])).stdout)
  if (check?.browser?.ok !== true) fail(`strobe finds no browser after Playwright's install: ${check?.browser?.detail ?? "no detail"}`)
}

function git(args: string[], cwd: string): { code: number; out: string } {
  const p = Bun.spawnSync(["git", ...args], { cwd, stdin: "ignore", stdout: "pipe", stderr: "pipe" })
  return { code: p.exitCode ?? 1, out: (p.stdout.toString() + p.stderr.toString()).trim() }
}

// The commit to compare against: the input, else the pull request's base, else the commit a push
// moved from. Fetched (one commit) when the checkout lacks it.
function base(cwd: string, ev: any): string {
  const given = input("base")
  const zero = /^0+$/
  const want = given || ev.pull_request?.base?.sha || (ev.before && !zero.test(ev.before) ? ev.before : "")
  if (!want) return ""
  const has = (ref: string) => git(["rev-parse", "--verify", "--quiet", `${ref}^{commit}`], cwd)
  let r = has(want)
  if (r.code !== 0) {
    const f = git(["fetch", "--no-tags", "--depth=1", "origin", want], cwd)
    if (f.code !== 0) fail(`base ${want}: not in the checkout, and git fetch origin ${want} failed: ${f.out}. Check out with fetch-depth: 0, or keep checkout's credentials for a private repository`)
    r = has(given && !/^[0-9a-f]{7,40}$/.test(given) ? "FETCH_HEAD" : want)
  }
  if (r.code !== 0) fail(`base ${want} is no commit`)
  return r.out.split("\n")[0]
}

const list = (s: string) =>
  s
    .split(/[\n,]/)
    .map((x) => x.trim())
    .filter(Boolean)

// The worst primary verdict of a comparison: worse before anything undecided before anything decided.
const RANK = ["worse", "refused", "unclear", "underpowered", "saturated", "stopped", "same", "no_worse", "better"]

export function verdicts(result: any): string[] {
  return (result?.series ?? []).map((s: any) => {
    const kind = s.primary?.verdict_kind ?? (s.refusal ? "refused" : "unclear")
    return s.primary?.warned && (kind === "same" || kind === "no_worse") ? "unclear" : kind
  })
}

function worst(kinds: string[]): string {
  return kinds.sort((a, b) => (RANK.indexOf(a) + 1 || 99) - (RANK.indexOf(b) + 1 || 99))[0] ?? "unclear"
}

// Any comparison that came out worse: a primary or a decided secondary, a check's exact counter over base's
// (`series.counters`), or a leak run's growth of any kind on any side (`series.leak.<side>.{js_heap,dom_nodes,
// native_host}`: the primary is the heap's alone, and a leak run exits 0, so a DOM-only leak shows only there).
function regressed(result: any): boolean {
  const worse = (c: any) => c && typeof c === "object" && c.verdict_kind === "worse"
  return (result?.series ?? []).some(
    (s: any) =>
      [s.primary, ...(s.secondaries ?? [])].some((c: any) => worse(c) && c.decided !== false) ||
      (Array.isArray(s.counters) && s.counters.some((n: any) => n?.state === "worse")) ||
      Object.values(s.leak && typeof s.leak === "object" ? s.leak : {}).some((side: any) => side && typeof side === "object" && Object.values(side).some(worse)),
  )
}

// One summary block per experiment, at most 1 MiB in all (GitHub's cap for a step's summary).
let summaryBytes = 0
function summarize(text: string) {
  const file = env("GITHUB_STEP_SUMMARY")
  const block = `${text.trimEnd()}\n\n`
  if (!file) return process.stdout.write(block)
  const bytes = Buffer.byteLength(block)
  if (summaryBytes + bytes > 1_000_000) {
    if (summaryBytes <= 1_000_000) appendFileSync(file, "\n_strobe: the step summary is full (1 MiB); the rest is in the logs and artifacts._\n")
    summaryBytes = Infinity
    return
  }
  summaryBytes += bytes
  appendFileSync(file, block)
}

type Done = { bench: string; id: string; dir: string; result: any; code: number; leak: boolean }

const stateDir = () =>
  process.platform === "win32"
    ? join(env("LOCALAPPDATA") || join(env("USERPROFILE"), "AppData", "Local"), "strobe")
    : join(env("XDG_DATA_HOME") || join(env("HOME"), ".local", "share"), "strobe")

// What a bench that did not run says on a publishable surface (the step summary, an annotation): a
// refusal's validator and summary only, never its detail, which can name this machine's other processes
// (SPEC §1 rule 9, §26.2); any other error without a `; busiest …` tail.
const BUSIEST = /;?\s*(the )?(three )?busiest\b[\s\S]*$/i
function publicError(out: any, stdout: string): string {
  return scrubText(errorText(out, stdout))
}
function errorText(out: any, stdout: string): string {
  const refusal = out?.refusal
  if (refusal?.summary) return `${refusal.validator ?? out.validator ?? "refused"}: ${String(refusal.summary).replace(BUSIEST, "")}`
  if (out?.validator && typeof out.error === "string") {
    // `<validator>: <summary>[; <detail>]` (SPEC §10.1): the detail goes.
    const [head] = out.error.split("; ")
    return head.replace(BUSIEST, "")
  }
  const text = typeof out?.error === "string" ? out.error : stdout.trim().slice(0, 2000) || "no output; the log above says why"
  return `${text.replace(BUSIEST, "")}${out?.hint ? ` (fix: ${String(out.hint).replace(BUSIEST, "")})` : ""}`
}

async function bench(file: string, args: string[], cwd: string, leak: boolean): Promise<Done | null> {
  console.log(`::group::strobe bench ${file} ${args.join(" ")}`)
  const r = await strobe(["bench", file, ...args, "--json"], { cwd })
  console.log("::endgroup::")
  const out = json(r.stdout)
  if (!out || out.error || !out.id) {
    const why = publicError(out, r.stdout)
    error(`strobe bench ${file} (exit ${r.code}): ${why}`)
    const fence = "`".repeat(Math.max(3, ...(why.match(/`+/g) ?? []).map((m) => m.length + 1)))
    summarize(`### strobe · ${file}\n\nstrobe bench exited ${r.code}${out?.validator ? ` (${out.validator})` : ""}:\n\n${fence}\n${why}\n${fence}\n`)
    return null
  }
  // A comparison's JSON names its result.json; a check's does not: it is in strobe's state folder (SPEC §4.2).
  const dir = typeof out.result === "string" ? dirname(out.result) : join(stateDir(), "experiments", out.id)
  return { bench: file, id: out.id, dir, result: out, code: r.code, leak }
}

// The experiment's markdown in a format, read from the file report writes (--json names it). A strobe
// without that format writes something else (before 0.29: an HTML view named "summary").
async function markdownReport(d: Done, cwd: string, format: string): Promise<string> {
  const labels = input("labels") && !d.leak && d.result?.kind !== "check" ? ["--labels", input("labels")] : []
  const r = json((await strobe(["report", "experiment", d.id, "--format", format, ...labels, "--json"], { cwd })).stdout)
  const path: string = r?.path ?? ""
  if (!path.endsWith(".md")) return ""
  return typeof r.markdown === "string" ? r.markdown : existsSync(path) ? readFileSync(path, "utf8") : ""
}

// Only `--format summary`, which strobe writes for publishing (no process names, scrubbed paths): never
// `md`, which is for the machine's own user. `scrubText` over it too, for what strobe's own scrub misses.
async function summary(d: Done, cwd: string) {
  const text = scrubText(await markdownReport(d, cwd, "summary"))
  if (!text) warn(`this strobe has no report --format summary (0.29.0 and later): ${d.id} gets no step summary; its result is in the artifacts`)
  summarize(text || `### strobe · ${d.bench}\n\n\`${d.id}\`: this strobe writes no step summary (\`report --format summary\` needs 0.29.0 or later).\n`)
}

async function htmlReport(d: Done, cwd: string): Promise<string> {
  const out = d.dir ? join(d.dir, "report-experiment.html") : ""
  const r = await strobe(["report", "experiment", d.id, "--format", "html", ...(out ? ["--out", out] : [])], { cwd })
  return r.code === 0 && out && existsSync(out) ? out : ""
}

// Which pull request takes the block: a same-repository pull request, unless update-pr says no. A fork's
// never: its token is read-only, and its code ran in this job.
function pullRequest(ev: any): string {
  const want = input("update-pr") || "auto"
  if (want === "false") return ""
  const pr = ev.pull_request
  if (!pr) {
    if (want === "true") warn("update-pr: true, but this run is no pull request; the block goes to the step summary only")
    return ""
  }
  const fork = pr.head?.repo?.full_name !== env("GITHUB_REPOSITORY")
  if (fork) {
    if (want === "true") warn("update-pr: a fork's pull request is never edited (its token is read-only); the block is in the step summary")
    return ""
  }
  return String(pr.number)
}

// The lists of other processes strobe's warnings, notes and refusals carry, as the PR block and the step
// summary say them (`scrub_public` in src/app/pr/extract.rs): their count and the busiest one's share.
const LISTS: [string, string][] = [
  ["(busiest elsewhere: ", ""],
  ["busiest processes on the machine (% of a core): ", " on the machine"],
  ["busiest processes on the machine: ", " on the machine"],
  ["busiest processes (% of a core): ", ""],
  ["busiest processes: ", ""],
]
// A list's items are `<name> <share>%` joined by `, `, and a name may hold anything (`Discord (PTB).exe`,
// `a).exe`): the list ends after the last item that is followed by no `, ` and another item.
const ITEM = /^(.*?) (\d+(?:\.\d+)?)%/s

// Text the job publishes or uploads, without what names the machine and its user (SPEC §26.2): the lists of
// other processes, the titles of other apps' windows (`WindowHidden`: `"<title>" of process <pid>`), home
// folders (`~`) and the machine's name on `strobe up`'s first line.
function scrubText(text: string): string {
  return scrubPaths(scrubLists(scrubWindows(text)))
    .replace(/\b(?:dwm\.exe|windowserver|xorg|xwayland|gnome-shell|kwin_wayland|kwin_x11|weston|sway|picom|mutter) (\d+(?:\.\d+)?% of a core)\b/gi, "compositor $1")
    .replace(/^strobe up · .*$/m, "strobe up")
}

// Every quoted string in a clause that names `… of process <pid>`: a window's title or class, an element's
// name and key (`button "Send" (key "send") of process 4120`); and the app macOS names a window by (`a
// window of <app> of process <pid>`).
function scrubWindows(text: string): string {
  return text
    .replace(/"(?:[^"\\]|\\.)*"(?=(?:[^";]|"(?:[^"\\]|\\.)*")*? of process \d)/g, '"…"')
    .replace(/\ba window of (?:(?!; ).)+? of process (\d)/g, "a window of process $1")
}

// A home folder as `~` where a path starts (never mid-path: `/tmp/x/home/y` stays): this runner user's own
// (`/root`, a renamed profile), then any user's `C:\Users\<u>`, `/home/<u>`, `/Users/<u>`.
function scrubPaths(text: string): string {
  const start = String.raw`(^|[\s"'\x60(=,[]|file:\/\/)`
  const own = homedir().replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/\\\\|\//g, String.raw`[\\/]`)
  return text
    .replace(new RegExp(`${start}${own}(?=[\\\\/\\s"'\\x60)]|$)`, process.platform === "win32" ? "gim" : "gm"), "$1~")
    .replace(new RegExp(String.raw`(^|[\s"'\x60(=,[/\\])[A-Za-z]:[\\/](?:Users|Documents and Settings)[\\/][^\\/\s"'\x60<>|)]+`, "gm"), "$1~")
    .replace(new RegExp(String.raw`${start}\/(?:home|Users)\/[^/\s"'\x60<>|)]+`, "gm"), "$1~")
}

function scrubLists(text: string): string {
  // `ReservedCpus`: the processes it moved, in parentheses up to what the note says next, and those it could
  // not move, up to the share it put back.
  let rest = text
    .replace(/moved per block \(.*?\)(?=, \d+ more started|; \d+ could not be moved|; \d+% put back|$)/gm, "moved per block")
    .replace(/(\d+) could not be moved: .*?(; \d+% put back|$)/gm, "$1 could not be moved$2")
  let out = ""
  for (;;) {
    const found = LISTS.map(([m, w]) => [rest.indexOf(m), m, w] as const)
      .filter(([at]) => at >= 0)
      .sort((a, b) => a[0] - b[0])[0]
    if (!found) return out + rest
    const [at, marker, where] = found
    const wrapped = marker.startsWith("(")
    let end = at + marker.length
    const shares: number[] = []
    for (;;) {
      const m = ITEM.exec(rest.slice(end))
      if (!m) break
      shares.push(Number(m[2]))
      end += m[0].length
      if (!/^, .*? \d+(?:\.\d+)?%/s.test(rest.slice(end))) break
      end += 2
    }
    // No item read: the list as strobe 0.29 cut it, to `; ` or the parenthesis around it.
    if (!shares.length) {
      const stop = rest.slice(at + marker.length).search(wrapped ? /\)/ : /;|\)/)
      end = stop < 0 ? rest.length : at + marker.length + stop
    }
    const list = rest.slice(at + marker.length, end)
    out += rest.slice(0, at)
    if (list.startsWith("none named")) out += rest.slice(at, end)
    else {
      const peak = Math.max(...shares).toFixed(0)
      const said =
        shares.length === 0
          ? `other processes${where} (not named here)`
          : shares.length === 1
            ? `1 other process${where}, ${peak}% of a core`
            : `${shares.length} other processes${where}, the busiest ${peak}% of a core`
      out += wrapped ? `(${said}` : said
    }
    rest = rest.slice(end)
  }
}

// result.json without the bench machine's other processes' names (SPEC §26.4): each block's `background`
// ([name, % of a core]) and the reserve's `moved` and `failed` ({name: count}) keep their shares and counts
// under a number, and every text and key goes through `scrubText`.
function scrub(v: any, key = ""): any {
  if (typeof v === "string") return scrubText(v)
  if (Array.isArray(v)) {
    if (key === "background") return v.map((e, i) => (Array.isArray(e) ? [`other process ${i + 1}`, ...e.slice(1)] : scrub(e)))
    return v.map((e) => scrub(e))
  }
  if (v && typeof v === "object") {
    const out: any = {}
    for (const [k, x] of Object.entries(v)) {
      out[scrubText(k)] =
        (k === "moved" || k === "failed") && key === "reserve" && x && typeof x === "object" && !Array.isArray(x)
          ? Object.fromEntries(Object.values(x).map((n, i) => [`other process ${i + 1}`, n]))
          : scrub(x, k)
    }
    return out
  }
  return v
}

export function scrubResult(v: any): any {
  const out = scrub(v)
  const screen = (row: any) => { if (row?.screen?.compositor) delete row.screen.compositor.name }
  screen(out)
  for (const series of Array.isArray(out?.series) ? out.series : []) {
    screen(series)
    for (const blocks of [series?.blocks, series?.pass?.blocks]) if (Array.isArray(blocks)) blocks.forEach(screen)
  }
  return out
}

// The artifact's result.json, scrubbed, and its HTML report drawn from that copy alone: strobe reads it in a
// state folder that holds nothing else, so no recording (whose insights name other processes too) reaches it.
async function scrubbedReport(d: Done, to: string, cwd: string) {
  const result = join(to, "result.json")
  writeFileSync(result, JSON.stringify(scrubResult(JSON.parse(readFileSync(join(d.dir, "result.json"), "utf8"))), null, 2))
  const state = join(env("RUNNER_TEMP") || cacheRoot(), `strobe-report-state-${d.id}`)
  mkdirSync(state, { recursive: true })
  const out = join(to, "report-experiment.html")
  const r = await strobe(["report", "experiment", result, "--format", "html", "--out", out], { cwd, env: process.platform === "win32" ? { LOCALAPPDATA: state } : { XDG_DATA_HOME: state } })
  rmSync(state, { recursive: true, force: true })
  if (r.code !== 0 || !existsSync(out)) warn(`strobe report of ${d.id}'s scrubbed result failed (exit ${r.code}): its artifact holds result.json without the HTML report`)
}

async function artifacts(done: Done[], cwd: string): Promise<string> {
  const root = join(env("RUNNER_TEMP") || cacheRoot(), `strobe-artifacts-${done.map((d) => d.id).join("-") || Date.now()}`)
  mkdirSync(root, { recursive: true })
  const recordings = flag("upload-recordings")
  for (const d of done) {
    if (!d.dir || !existsSync(join(d.dir, "result.json"))) continue
    const to = join(root, d.id)
    mkdirSync(to, { recursive: true })
    await scrubbedReport(d, to, cwd)
    for (const f of readdirSync(d.dir)) {
      // The bench file's own console output, its paths' home folders as `~`.
      if (f === "runner.log") writeFileSync(join(to, f), scrubText(readFileSync(join(d.dir, f), "utf8")))
      if (f.startsWith("failed-") && f.endsWith(".png")) copyFileSync(join(d.dir, f), join(to, f))
    }
    // Recordings can hold what the target showed and typed (SPEC rule 9): only when asked.
    if (recordings) {
      const recs = new Set(readFileSync(join(d.dir, "result.json"), "utf8").match(/rec_[0-9a-f]{6,}/g) ?? [])
      const from = join(dirname(dirname(d.dir)), "recordings")
      for (const rec of recs) if (existsSync(join(from, rec))) {
        const dest = join(to, "recordings", rec)
        cpSync(join(from, rec), dest, { recursive: true })
        const meta = join(dest, "meta.json")
        if (existsSync(meta)) writeFileSync(meta, JSON.stringify(scrubResult(JSON.parse(readFileSync(meta, "utf8"))), null, 2))
      }
    }
  }
  return root
}

async function runStep() {
  guard()
  // The token goes to the PR edit alone, never into the environment of the bench (the project's code).
  const token = input("github-token")
  delete process.env.INPUT_GITHUB_TOKEN
  const mode = oneOf("mode", ["check", "compare"], "check")
  const failOn = oneOf("fail-on", ["worse", "unclear", "never"], "worse")
  const cwd = workdir()
  const ev = event()
  const benches = list(input("bench"))
  const headless = oneOf("headless", ["true", "false"], "true") === "true"
  const common: string[] = []
  for (const name of ["filter", "target", "params"]) if (input(name)) common.push(`--${name}`, input(name))
  if (headless) common.push("--headless")

  // A profile this runner cannot resolve (an app the job never builds, a guard's variable unset) is a
  // warning: the bench refuses only the series that use it.
  console.log("::group::strobe config")
  const config = await strobe(["config"], { cwd, capture: false })
  console.log("::endgroup::")
  if (config.code !== 0) warn(`strobe config: strobe.json has a problem on this runner (exit ${config.code}); its lines in the log say which`)

  const done: Done[] = []
  let failed = false
  const noworse = input("noworse")
  if (mode === "check") {
    // Every case once, on head; with noworse, on base too, each exact counter no worse than base's.
    // Never a timing verdict: a shared runner's timings move with its neighbours (SharedRunner).
    const b = noworse ? base(cwd, ev) : ""
    if (noworse && !b) warn("noworse needs a base commit (a pull request, a push, or the base input); checking head alone")
    for (const file of benches) {
      const args = ["--head", ".", "--check", ...common]
      if (b) args.push("--base", b, "--noworse", noworse)
      if (input("max-time")) args.push("--max-time", input("max-time"))
      const d = await bench(file, args, cwd, false)
      if (!d) {
        failed = true
        continue
      }
      done.push(d)
      // 1: a case failed; 4: every case refused. fail-on: never reports them; anything else is an error.
      if (d.code !== 0 && !(failOn === "never" && [1, 4].includes(d.code))) failed = true
    }
  } else {
    // A paired A/B of base and head on a machine this job has to itself (a self-hosted runner).
    const b = base(cwd, ev)
    if (!b) fail("mode compare needs a base commit: run on a pull request or a push, or give the base input")
    for (const file of benches) {
      const args = ["--base", b, "--head", ".", "--max-time", input("max-time") || "20m", ...common]
      for (const name of ["primary", "pairs", "equivalence", "noworse"]) if (input(name)) args.push(`--${name}`, input(name))
      if (failOn === "unclear") args.push("--require-verdict")
      const d = await bench(file, args, cwd, false)
      if (!d) {
        failed = true
        continue
      }
      done.push(d)
      const kinds = verdicts(d.result)
      if (failOn !== "never" && regressed(d.result)) failed = true
      if (failOn === "unclear" && (d.code === 5 || kinds.some((k) => !["better", "same", "no_worse"].includes(k)))) failed = true
      // 0, a verdict-less run with --require-verdict (5) or one --max-time cut short (124) keep their verdicts;
      // every series refused (4) is a result fail-on: never reports.
      if (![0, 5, 124].includes(d.code) && !(failOn === "never" && d.code === 4)) failed = true
    }
  }

  // A leak run per bench: each case's memory over repeated runs, judged by its slope.
  const leak = input("leak")
  const leakDone: Done[] = []
  const leaking = !!leak && leak !== "false"
  if (leaking) {
    // leak: true runs the filter's cases, or it names the cases itself.
    const filter = leak === "true" ? input("filter") : leak
    for (const file of benches) {
      const args = ["--head", ".", "--probes", "memory", "--repeat", input("leak-repeat") || "6"]
      if (filter) args.push("--filter", filter)
      for (const name of ["target", "params"]) if (input(name)) args.push(`--${name}`, input(name))
      if (headless) args.push("--headless")
      const d = await bench(file, args, cwd, true)
      if (!d) {
        failed = true
        continue
      }
      leakDone.push(d)
      if (failOn !== "never" && (d.code !== 0 || regressed(d.result))) failed = true
    }
  }

  for (const d of [...done, ...leakDone]) await summary(d, cwd)

  let reportPath = ""
  for (const d of [...done, ...leakDone]) reportPath ||= await htmlReport(d, cwd)

  let prUpdated = false
  const pr = mode === "compare" ? pullRequest(ev) : ""
  if (pr) {
    for (const d of done) {
      const args = ["report", "experiment", d.id, "--format", "pr", "--update-pr", pr, "-R", env("GITHUB_REPOSITORY")]
      if (input("labels")) args.push("--labels", input("labels"))
      const r = await strobe(args, { cwd, capture: false, env: token ? { GH_TOKEN: token } : {} })
      if (r.code === 0) prUpdated = true
      else if (input("update-pr") === "true") {
        error(`strobe report --update-pr ${pr} failed (exit ${r.code}): the job needs permissions: pull-requests: write`)
        failed = true
      } else warn(`strobe report --update-pr ${pr} failed (exit ${r.code}); give the job permissions: pull-requests: write, or update-pr: false`)
    }
  }

  // A bench file that did not run at all (a usage error, a refusal before anything ran), its leak run
  // included, is an error; a leak run that failed or found growth fails a check.
  const kinds = done.flatMap((d) => verdicts(d.result))
  const leakFailed = leakDone.some((d) => d.code !== 0 || regressed(d.result))
  const verdict =
    done.length < benches.length || (leaking && leakDone.length < benches.length)
      ? "error"
      : mode === "check"
        ? done.every((d) => d.code === 0 && d.result.passed !== false) && !leakFailed
          ? "pass"
          : "fail"
        : worst(kinds)
  output("experiment", done.map((d) => d.id).join(" "))
  output("leak-experiment", leakDone.map((d) => d.id).join(" "))
  output("verdict", verdict)
  output("regressed", String([...done, ...leakDone].some((d) => regressed(d.result))))
  output("report-path", reportPath)
  output("pr-updated", String(prUpdated))

  const upload = oneOf("upload-artifacts", ["on-failure", "always", "never"], "on-failure")
  const all = [...done, ...leakDone]
  if (all.length && (upload === "always" || (upload === "on-failure" && failed))) {
    output("artifact-path", await artifacts(all, cwd))
    output("artifact-name", input("artifact-name") || `strobe-${env("GITHUB_JOB") || "job"}-${all.map((d) => d.id).join("-")}`)
  }
  if (failed) {
    error(
      mode === "check"
        ? "strobe's check failed: the step summary says which case or why the bench did not run"
        : "strobe's comparison failed the fail-on rule: the step summary has the verdicts or why the bench did not run",
    )
    process.exit(1)
  }
}

if (import.meta.main) {
  const step = process.argv[2]
  const steps: Record<string, () => Promise<void>> = { resolve: resolveStep, install: installStep, setup: setupStep, browser: browserStep, run: runStep }
  if (!steps[step]) fail(`ci-action.ts: no step ${step}; one of ${Object.keys(steps).join(", ")}`)
  await steps[step]()
}
