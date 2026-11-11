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

// A dead strobe's launch home whose app wrote lines aiming outside: the bench's start reaps it
// (`reap_dead_owners`, the path from before prune), which must touch nothing outside.
const outside = P.join(scratch, "outside")
const home = P.join(data, "strobe", "run", "home-4194303-1")
const driver = win ? P.join("AppData", "Local", "D3DSCache") : P.join(".cache", "mesa_shader_cache")
fs.mkdirSync(P.join(outside, "gpu", "dirs", driver), { recursive: true })
fs.writeFileSync(P.join(outside, "gpu", "dirs", driver, "keep.txt"), "a store outside strobe's")
fs.writeFileSync(P.join(outside, "gpu", "paths"), driver.replaceAll("\\", "/"))
fs.writeFileSync(P.join(outside, "canary.txt"), "named by the home's .strobe-shared")
fs.mkdirSync(P.join(home, driver), { recursive: true })
fs.writeFileSync(P.join(home, driver, "x.bin"), "x")
fs.writeFileSync(P.join(home, ".strobe-gpu-cache"), P.join(outside, "gpu"))
fs.writeFileSync(P.join(home, ".strobe-shared"), `${P.join(outside, "canary.txt")}\t${P.join(outside, "canary.txt")}\n`)

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
check(!fs.existsSync(home), "the bench's start reaped the dead home")
check(
  fs.existsSync(P.join(outside, "canary.txt")) && fs.existsSync(P.join(outside, "gpu", "dirs", driver, "keep.txt")) && fs.readFileSync(P.join(outside, "gpu", "paths"), "utf8") === driver.replaceAll("\\", "/"),
  "its .strobe-shared and .strobe-gpu-cache aiming outside touched nothing there",
)

// --- a prune between an experiment's launches: its GPU cache store and seed copy stay, and the next
// launch is warm ---
const repo2 = P.join(scratch, "repo2")
fs.mkdirSync(P.join(repo2, "seed"), { recursive: true })
git(repo2, "init", "-q")
const exe = win ? "fake-app.exe" : "fake-app"
fs.writeFileSync(
  P.join(repo2, "strobe.json"),
  JSON.stringify({
    build: { command: `bun -e "require('fs').copyFileSync(process.execPath, process.argv.at(-1))" "{cache}/${exe}"`, output: `{cache}/${exe}` },
    apps: { fake: { kind: "native", exe: { windows: "fake-app.exe", macos: "fake-app", linux: "fake-app" } } },
  }),
)
// The app writes a driver's shader cache into its home, as a GPU app does, then idles.
fs.writeFileSync(
  P.join(repo2, "app.ts"),
  `import { mkdirSync, writeFileSync } from "node:fs"\nimport { join } from "node:path"\nconst dir = process.platform === "win32" ? join(process.env.LOCALAPPDATA!, "D3DSCache", "check") : join(process.env.XDG_CACHE_HOME!, "mesa_shader_cache", "check")\nmkdirSync(dir, { recursive: true })\nwriteFileSync(join(dir, "launch-" + Date.now() + ".bin"), "x".repeat(4096))\nconsole.log("fake ready")\nsetInterval(() => {}, 1000)\n`,
)
fs.writeFileSync(P.join(repo2, "seed", "big.bin"), Buffer.alloc(65 << 20, 1))
const caseOf = (n: string) => `    ${n}: { targets: ["app"], primary: "cpu.work", run: ({ page }) => page.wait(8000) },`
fs.writeFileSync(
  P.join(repo2, "warm.bench.ts"),
  `import { bench, native } from "@hona/strobe"\nexport default bench({\n  targets: { app: native("fake", { args: [import.meta.dir + "/app.ts"], window: false, home: "./seed" }) },\n  cases: {\n${["one", "two", "three"].map(caseOf).join("\n")}\n  },\n})\n`,
)
git(repo2, "add", "-A")
git(repo2, "-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qm", "one")
const exp = "exp_warmcheck"
const store = P.join(data, "strobe", "gpu-cache", `${exp}-head`)
const log2 = fs.openSync(P.join(scratch, "bench2.log"), "w")
const bench2 = spawn(bin, ["bench", P.join(repo2, "warm.bench.ts"), "--head", ".", "--check", "--id", exp, "--progress", "stdout"], { env: { ...env, STROBE_PROGRESS: "stdout" }, cwd: repo2, stdio: ["ignore", log2, log2] })
const ended2 = new Promise<number>((r) => bench2.on("exit", (c) => r(c ?? -1)))
// The first launch has given its caches back to the store: a prune now lands between launches.
for (let i = 0; i < 240 && !fs.existsSync(P.join(store, "paths")); i++) await Bun.sleep(250)
const seedDir = fs.readdirSync(P.join(data, "strobe", "seeds")).map((n) => P.join(data, "strobe", "seeds", n)).find((p) => fs.statSync(p).isDirectory()) ?? ""
const seedTime = seedDir ? fs.statSync(P.join(seedDir, "big.bin")).mtimeMs : 0
const mid = strobe("prune", "--force", "--json").json
const midAt = Date.now()
const keptMid = Object.fromEntries((mid?.kept ?? []).map((e: any) => [e.name, e.why]))
const removedMid = (mid?.removed ?? []).map((e: any) => e.name)
check(
  fs.existsSync(P.join(store, "paths")) && !removedMid.some((n: string) => n.startsWith("gpu-cache/") || n.startsWith("seeds/")),
  "a prune between the launches keeps the experiment's GPU cache store and seed copy",
  `kept ${Object.entries(keptMid).filter(([n]) => /^(gpu-cache|seeds|builds)\//.test(n)).map(([n, w]) => `${n}: ${w}`).join("; ")}`,
)
const code2 = await ended2
const result = JSON.parse(fs.readFileSync(P.join(data, "strobe", "experiments", exp, "result.json"), "utf8"))
// Each launch's recording says whether it found GPU caches, and when it was made.
const metas = (result.series ?? []).map((s: any) => JSON.parse(fs.readFileSync(P.join(data, "strobe", "recordings", s.recording, "meta.json"), "utf8")))
const later = metas.filter((m: any) => Date.parse(m.created) > midAt)
check(
  code2 === 0 && metas.length === 3 && metas[0].gpu_cache === "cold" && later.length > 0 && later.every((m: any) => m.gpu_cache === "warm"),
  "the launches made after the prune were warm",
  `exit ${code2}, ${metas.map((m: any) => `${m.gpu_cache}${Date.parse(m.created) > midAt ? " (after the prune)" : ""}`).join(", ")}; ${P.join(scratch, "bench2.log")}`,
)
check(seedDir !== "" && fs.statSync(P.join(seedDir, "big.bin")).mtimeMs === seedTime, "the seed copy was not made again", seedDir)
const leftInGpu = fs.readdirSync(P.join(data, "strobe", "gpu-cache")).filter((n) => n.startsWith(exp))
check(leftInGpu.length === 0, "once the experiment ended, its store and the store's stamp are gone", leftInGpu.join(", ") || "nothing left")
const failed = results.filter((r) => r.startsWith("FAIL")).length
console.log(`\n${results.length - failed} passed, ${failed} failed · scratch left in ${scratch}`)
process.exit(failed ? 1 : 0)
