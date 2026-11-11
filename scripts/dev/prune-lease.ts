// Checks that `strobe prune` leaves a build a running bench uses (SPEC §5.14, `host::cache`), and that
// a bench makes a git ref's worktree again over a stale entry in the repository's worktree list (the
// folder removed by hand): a scratch state folder, a scratch repository whose cli() case sleeps 40 s,
// `strobe bench --check --head git:HEAD` in the background, and `strobe prune --force` while it runs.
//
//   bun scripts/dev/prune-lease.ts <strobe> <scratch folder>
//
// It runs a `--check` bench (the bench slot for about a minute). The scratch folder must not exist.
import { spawn, spawnSync } from "node:child_process"
import * as fs from "node:fs"
import * as path from "node:path"

const [bin, scratchArg] = process.argv.slice(2)
if (!bin || !scratchArg) throw new Error("usage: bun scripts/dev/prune-lease.ts <strobe> <scratch folder>")
const win = process.platform === "win32"
const P = win ? path.win32 : path.posix
const scratch = P.resolve(scratchArg)
if (fs.existsSync(scratch)) throw new Error(`${scratch} exists: give a folder that does not, so nothing real is touched`)
const data = P.join(scratch, "data")
const env = { ...process.env, [win ? "LOCALAPPDATA" : "XDG_DATA_HOME"]: data, STROBE_PROGRESS: "off" }
const results: string[] = []
const check = (ok: boolean, what: string, detail = "") => {
  const line = `${ok ? "PASS" : "FAIL"}  ${what}${detail ? `: ${detail}` : ""}`
  results.push(line)
  console.log(line)
}
const git = (cwd: string, ...args: string[]) => spawnSync("git", args, { cwd, encoding: "utf8" })
const strobe = (...args: string[]) => {
  const r = spawnSync(bin, args, { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], maxBuffer: 1 << 28 })
  return { code: r.status, json: (() => { try { return JSON.parse(r.stdout) } catch { return null } })(), err: r.stderr }
}

const repo = P.join(scratch, "repo")
fs.mkdirSync(repo, { recursive: true })
git(repo, "init", "-q")
fs.writeFileSync(P.join(repo, "sleep.ts"), `await Bun.sleep(40000)\nconsole.log("strobe:metric slept=1")\n`)
fs.writeFileSync(
  P.join(repo, "sleep.bench.ts"),
  `import { bench, cli } from "@hona/strobe"\nexport default bench({\n  targets: { app: cli("bun sleep.ts") },\n  cases: { sleep: { targets: ["app"], primary: "metric.slept", run: (ctx) => ctx.exec() } },\n})\n`,
)
fs.writeFileSync(P.join(repo, "strobe.json"), "{}\n")
git(repo, "add", "-A")
git(repo, "-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qm", "one")
const sha = git(repo, "rev-parse", "HEAD").stdout.trim()
const wt = P.join(data, "strobe", "builds", sha.slice(0, 12))
const name = `builds/${sha.slice(0, 12)}`

// A worktree at the path the bench will use, then its folder removed by hand: a stale entry.
fs.mkdirSync(P.dirname(wt), { recursive: true })
git(repo, "worktree", "add", "--detach", wt, sha)
fs.rmSync(wt, { recursive: true, force: true })
check(/prunable/.test(git(repo, "worktree", "list", "--porcelain").stdout), "a stale worktree entry is planted")

const log = fs.openSync(P.join(scratch, "bench.log"), "w")
const bench = spawn(bin, ["bench", P.join(repo, "sleep.bench.ts"), "--head", "git:HEAD", "--check", "--progress", "stdout"], { env: { ...env, STROBE_PROGRESS: "stdout" }, cwd: repo, stdio: ["ignore", log, log] })
const ended = new Promise<number>((r) => bench.on("exit", (c) => r(c ?? -1)))
let inUse = ""
for (let i = 0; i < 60 && !inUse; i++) {
  await Bun.sleep(1000)
  const dry = strobe("prune", "--dry-run", "--json").json
  inUse = dry?.kept?.find((e: any) => e.name === name)?.why ?? ""
}
check(/in use/.test(inUse), "while the bench runs, the plan keeps its worktree", inUse || "never seen")
const forced = strobe("prune", "--force", "--json").json
check(!forced?.removed?.some((e: any) => e.name === name) && fs.existsSync(P.join(wt, "sleep.ts")), "prune --force leaves it, and its files are there")
const code = await ended
check(code === 0, "the bench made the worktree over the stale entry and passed", `exit ${code}; ${P.join(scratch, "bench.log")}`)
const after = strobe("prune", "--dry-run", "--json").json
check(after?.remove?.some((e: any) => e.name === name), "once the bench ended, the worktree is a cache the plan removes")
const failed = results.filter((r) => r.startsWith("FAIL")).length
console.log(`\n${results.length - failed} passed, ${failed} failed · scratch left in ${scratch}`)
process.exit(failed ? 1 : 0)
