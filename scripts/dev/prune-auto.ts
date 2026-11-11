// Checks strobe's automatic cache retention (SPEC §5.14) on a scratch state folder: never beside a bench
// (checked before it starts and before each entry), never after a failed command or `track
// --schedules`, refused when the user's strobe.json does not parse, and from the daemon's minute tick.
// The state root moves into the scratch folder (LOCALAPPDATA on Windows, XDG_DATA_HOME elsewhere); the
// daemon it starts listens on its own port, and is ended by its pid.
//
//   bun scripts/dev/prune-auto.ts <strobe> <scratch folder>
//
// It runs short `--check` benches (the bench slot for seconds at a time). The scratch folder must not exist.
import { spawn, spawnSync } from "node:child_process"
import * as fs from "node:fs"
import * as path from "node:path"

const [bin, scratchArg] = process.argv.slice(2)
if (!bin || !scratchArg) throw new Error("usage: bun scripts/dev/prune-auto.ts <strobe> <scratch folder>")
const win = process.platform === "win32"
const P = win ? path.win32 : path.posix
const scratch = P.resolve(scratchArg)
if (fs.existsSync(scratch)) throw new Error(`${scratch} exists: give a folder that does not, so nothing real is touched`)
const data = P.join(scratch, "data")
const state = P.join(data, "strobe")
const env = { ...process.env, [win ? "LOCALAPPDATA" : "XDG_DATA_HOME"]: data, STROBE_PROGRESS: "stderr" }
const results: string[] = []
const check = (ok: boolean, what: string, detail = "") => {
  const line = `${ok ? "PASS" : "FAIL"}  ${what}${detail ? `: ${detail}` : ""}`
  results.push(line)
  console.log(line)
}
const strobe = (...args: string[]) => {
  const r = spawnSync(bin, args, { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], cwd: scratch, maxBuffer: 1 << 28 })
  return { code: r.status, out: r.stdout + r.stderr }
}
const cacheRoot = P.join(state, "builds", "cache")
// Old compile caches: `n` entries of `files` files each, 20 days unused.
const plant = (n: number, files: number) => {
  const t = new Date(Date.now() - 20 * 86_400_000)
  for (let i = 0; i < n; i++) {
    const d = P.join(cacheRoot, `old${String(i).padStart(13, "0")}`)
    fs.mkdirSync(d, { recursive: true })
    for (let f = 0; f < files; f++) {
      fs.writeFileSync(P.join(d, `${f}.o`), "x")
      fs.utimesSync(P.join(d, `${f}.o`), t, t)
    }
    fs.utimesSync(d, t, t)
  }
}
const left = () => (fs.existsSync(cacheRoot) ? fs.readdirSync(cacheRoot).filter((n) => n.startsWith("old") && !n.endsWith(".used")).length : 0)
const autoFile = P.join(state, "prune.json")
const quick = ["run", "--check", "--head", "bun -e 0"]
// A bench that holds a block for `s` seconds (a cli() case that sleeps).
fs.mkdirSync(P.join(scratch, "b"), { recursive: true })
const benchFile = (s: number) => {
  const f = P.join(scratch, "b", `sleep${s}.bench.ts`)
  fs.writeFileSync(P.join(scratch, "b", `sleep${s}.ts`), `await Bun.sleep(${s * 1000})\nconsole.log("strobe:metric slept=1")\n`)
  fs.writeFileSync(f, `import { bench, cli } from "@hona/strobe"\nexport default bench({\n  targets: { app: cli("bun ${P.join(scratch, "b", `sleep${s}.ts`).replaceAll("\\", "/")}") },\n  cases: { sleep: { targets: ["app"], primary: "metric.slept", run: (ctx) => ctx.exec() } },\n})\n`)
  return f
}
fs.writeFileSync(P.join(scratch, "b", "strobe.json"), "{}\n")
const startBench = (s: number) => spawn(bin, ["bench", benchFile(s), "--head", P.join(scratch, "b"), "--check"], { env, cwd: P.join(scratch, "b"), stdio: "ignore" })
const registered = () => fs.existsSync(P.join(state, "run", "benches")) && fs.readdirSync(P.join(state, "run", "benches")).some((n) => n.endsWith(".json"))
const until = async (ok: () => boolean, ms: number) => {
  const end = Date.now() + ms
  while (!ok() && Date.now() < end) await Bun.sleep(100)
  return ok()
}

let daemon: ReturnType<typeof spawn> | undefined
try {
  fs.mkdirSync(state, { recursive: true })
  // The daemon of this scratch state listens on its own port, never the real one's.
  fs.writeFileSync(P.join(state, "state.json"), JSON.stringify({ mcp_port: 47993 }))
  plant(20, 20)

  // --- a user strobe.json that does not parse: no automatic prune (the daemon's tick, where no command
  // fails on it), and status says so ---
  fs.writeFileSync(P.join(state, "strobe.json"), `{ "prune": { "unused": "off" } `)
  daemon = spawn(bin, ["daemon"], { env, cwd: scratch, stdio: "ignore" })
  await Bun.sleep(8000)
  const said = strobe("status", "--fast").out.split("\n").find((l) => /not pruned on their own/.test(l))?.trim() ?? ""
  check(said !== "" && left() === 20 && !fs.existsSync(autoFile), "a broken strobe.json refuses the automatic prune, and status says so", said)
  daemon.kill()
  daemon = undefined
  await Bun.sleep(1000)
  fs.writeFileSync(P.join(state, "strobe.json"), "{}\n")

  // --- not after a command that failed, nor after track --schedules ---
  const failedRun = strobe("run", "--check", "--head", "bun -e process.exit(3)")
  const schedules = strobe("track", "--schedules")
  check(failedRun.code !== 0 && left() === 20 && !fs.existsSync(autoFile), "no automatic prune after a command that failed", `exit ${failedRun.code}`)
  check(left() === 20 && !fs.existsSync(autoFile), "nor after track --schedules", `exit ${schedules.code}`)
  // `track <file> --schedule` only registers a schedule (exit 0): taken out again before any daemon runs it.
  const bdir = P.join(scratch, "b")
  const git = (...a: string[]) => spawnSync("git", ["-c", "user.email=a@b", "-c", "user.name=a", ...a], { cwd: bdir, encoding: "utf8" })
  git("init", "-q")
  git("add", "-A")
  git("commit", "-qm", "one")
  const sch = spawnSync(bin, ["track", benchFile(20), "--schedule", "nightly", "--json"], { env, cwd: bdir, encoding: "utf8" })
  const schId = (() => { try { return JSON.parse(sch.stdout).id as string } catch { return "" } })()
  check(sch.status === 0 && schId !== "" && left() === 20 && !fs.existsSync(autoFile), "nor after track --schedule (it registered one, exit 0)", `exit ${sch.status} ${schId}`)
  if (schId) strobe("track", "--unschedule", schId)

  // --- what the user's strobe.json says, read as it is ---
  const refused = () => strobe("status", "--fast", "--json").out.match(/"auto_refused":\s*("(?:[^"\\]|\\.)*"|null)/)?.[1] ?? "?"
  fs.writeFileSync(P.join(state, "strobe.json"), `{ "prune": { "unused": "0s" } }`)
  const zero = refused()
  check(/off/.test(zero), "prune.unused \"0s\" refuses the automatic prune and says to write \"off\"", zero)
  fs.rmSync(P.join(state, "strobe.json"))
  fs.mkdirSync(P.join(state, "strobe.json"))
  const unreadable = refused()
  check(/cannot be read/.test(unreadable), "a strobe.json that cannot be read refuses it, never taken for an absent one", unreadable)
  fs.rmdirSync(P.join(state, "strobe.json"))
  // Windows PowerShell 5.1's `>` writes UTF-16: read as what it says ("off"), and the daemon prunes nothing.
  fs.writeFileSync(P.join(state, "strobe.json"), Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(`{ "prune": { "unused": "off" } }\r\n`, "utf16le")]))
  const utf16 = refused()
  daemon = spawn(bin, ["daemon"], { env, cwd: scratch, stdio: "ignore" })
  await Bun.sleep(8000)
  check(utf16 === "null" && left() === 20 && !fs.existsSync(autoFile), "a UTF-16 strobe.json saying \"off\" is read: the daemon prunes nothing", `auto_refused ${utf16}, ${left()} left`)
  daemon.kill()
  daemon = undefined
  await Bun.sleep(1000)
  fs.writeFileSync(P.join(state, "strobe.json"), "{}\n")

  // --- the daemon: not while a bench runs, then on its next minute once it ended ---
  const a = startBench(20)
  const aEnded = new Promise<void>((r) => a.on("exit", () => r()))
  check(await until(registered, 20_000), "a bench runs (registered)")
  daemon = spawn(bin, ["daemon"], { env, cwd: scratch, stdio: "ignore" })
  await Bun.sleep(8000)
  check(left() === 20 && !fs.existsSync(autoFile), "the daemon prunes nothing while a bench runs")
  await aEnded
  const pruned = await until(() => left() === 0 && fs.existsSync(autoFile), 90_000)
  check(pruned, "once the bench ended, the daemon's tick pruned the old caches", `${left()} left, prune.json ${fs.existsSync(autoFile)}`)
  daemon.kill()
  daemon = undefined

  // --- a bench that starts during an automatic prune stops it before the next entry ---
  fs.rmSync(autoFile, { force: true })
  plant(win ? 300 : 600, win ? 150 : 300)
  const total = left()
  const run = spawn(bin, quick, { env, cwd: scratch, stdio: ["ignore", "pipe", "pipe"] })
  let out = ""
  run.stdout?.on("data", (d) => (out += d))
  run.stderr?.on("data", (d) => (out += d))
  const runEnded = new Promise<number>((r) => run.on("exit", (c) => r(c ?? -1)))
  const began = await until(() => left() < total, 120_000)
  const b = startBench(15)
  const bEnded = new Promise<void>((r) => b.on("exit", () => r()))
  await runEnded
  const rest = left()
  check(began && rest > 0 && rest < total && !fs.existsSync(autoFile), "a bench that started mid-prune stopped it (entries left, no day recorded)", `${total - rest} of ${total} removed; ${out.split("\n").find((l) => /pruned/.test(l))?.trim() ?? ""}`)
  await bEnded
} finally {
  daemon?.kill()
}
const failed = results.filter((r) => r.startsWith("FAIL")).length
console.log(`\n${results.length - failed} passed, ${failed} failed · scratch left in ${scratch}`)
process.exit(failed ? 1 : 0)
