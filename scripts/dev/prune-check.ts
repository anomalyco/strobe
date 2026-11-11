// Checks `strobe prune` and `strobe status`'s disk numbers (SPEC §5.14) on a scratch state folder: it
// plants every kind of entry (a real git worktree, one in use, a link out of the folder with a canary
// behind it, a read-only file, a file held open, a deep path, seeds linked into a dead launch's home,
// old and new recordings, an experiment naming a recording, history naming an experiment), runs
// strobe against it with its state root moved there (LOCALAPPDATA on Windows, XDG_DATA_HOME
// elsewhere), and prints each check beside an independent measurement (`du`).
//
//   bun scripts/dev/prune-check.ts <strobe> <scratch folder>
//
// The scratch folder is made (it must not exist, so no real state is ever used) and left for a look.
import { spawn, spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import * as fs from "node:fs"
import * as path from "node:path"

const [bin, scratchArg] = process.argv.slice(2)
if (!bin || !scratchArg) throw new Error("usage: bun scripts/dev/prune-check.ts <strobe> <scratch folder>")
const win = process.platform === "win32"
const P = win ? path.win32 : path.posix
const scratch = P.resolve(scratchArg)
if (fs.existsSync(scratch)) throw new Error(`${scratch} exists: give a folder that does not, so nothing real is touched`)
const data = P.join(scratch, "data")
const state = P.join(data, "strobe")
const env = { ...process.env, [win ? "LOCALAPPDATA" : "XDG_DATA_HOME"]: data, STROBE_PROGRESS: "off" }
const outside = P.join(scratch, "outside")
const DAY = 86_400_000
const results: string[] = []
const check = (ok: boolean, what: string, detail = "") => {
  const line = `${ok ? "PASS" : "FAIL"}  ${what}${detail ? `: ${detail}` : ""}`
  results.push(line)
  console.log(line)
}

const mk = (p: string) => fs.mkdirSync(p, { recursive: true })
const put = (p: string, bytes: number | string) => {
  mk(P.dirname(p))
  fs.writeFileSync(p, typeof bytes === "string" ? bytes : Buffer.alloc(bytes, 7))
}
const MB = 1 << 20
// Sets a tree's times `days` back, the folders after what they hold.
const age = (p: string, days: number) => {
  const t = new Date(Date.now() - days * DAY)
  const walk = (q: string) => {
    const s = fs.lstatSync(q)
    if (s.isDirectory()) for (const n of fs.readdirSync(q)) walk(P.join(q, n))
    if (!s.isSymbolicLink()) fs.utimesSync(q, t, t)
  }
  walk(p)
}
const git = (cwd: string, ...args: string[]) => {
  const r = spawnSync("git", args, { cwd, encoding: "utf8" })
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`)
  return r.stdout.trim()
}
// `host::paths` and `bench::hash_str`: a variant's files under builds/ are named by its root path's hash.
const hash = (s: string) => createHash("sha256").update(s).digest("hex").slice(0, 16)
const strobe = (...args: string[]) => {
  const r = spawnSync(bin, args, { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], maxBuffer: 1 << 28 })
  return { code: r.status, out: r.stdout, err: r.stderr, json: (() => { try { return JSON.parse(r.stdout) } catch { return null } })() }
}
// Allocated bytes under `p`, by `du` (each file once, links not followed): the independent measure.
const du = (p: string) => {
  const exe = win ? "C:\\Program Files\\Git\\usr\\bin\\du.exe" : "du"
  const r = spawnSync(exe, ["-s", "-x", "-B1", p], { encoding: "utf8" })
  return Number(r.stdout.split(/\s/)[0])
}
// Every path under `p` but strobe's own bookkeeping a prune writes (its lock, the kept walks).
const listing = (p: string): string[] => {
  const out: string[] = []
  const walk = (q: string) => {
    for (const n of fs.readdirSync(q)) {
      const c = P.join(q, n)
      if (n === "prune.lock" || n === "disk-cache.json") continue
      out.push(c)
      const s = fs.lstatSync(c)
      if (s.isDirectory() && !s.isSymbolicLink()) walk(c)
    }
  }
  walk(p)
  return out.sort()
}
const within = (a: number, b: number, pct: number) => Math.abs(a - b) <= (Math.max(a, b) * pct) / 100

// --- the scratch repository a bench would build from ---
const repo = P.join(scratch, "repo")
mk(repo)
git(repo, "init", "-q")
put(P.join(repo, "a.txt"), "a")
git(repo, "add", "a.txt")
git(repo, "-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qm", "one")
const sha1 = git(repo, "rev-parse", "HEAD")
put(P.join(repo, "a.txt"), "b")
git(repo, "-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qam", "two")
const sha2 = git(repo, "rev-parse", "HEAD")
put(P.join(repo, "a.txt"), "c")
git(repo, "-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qam", "three")
const sha3 = git(repo, "rev-parse", "HEAD")
// git's worktree.useRelativePaths came with 2.48.
const gitVersion = git(repo, "--version").match(/(\d+)\.(\d+)/)?.slice(1).map(Number) ?? [0, 0]
const relative = gitVersion[0] > 2 || (gitVersion[0] === 2 && gitVersion[1] >= 48)
const root = !win && process.getuid?.() === 0

// --- outside the state: a canary behind a link ---
put(P.join(outside, "canary.txt"), "do not delete")
put(P.join(outside, "big.bin"), 4 * MB)
const link = (target: string, at: string) => {
  mk(P.dirname(at))
  if (win) spawnSync("cmd", ["/c", "mklink", "/J", at, target])
  else fs.symlinkSync(target, at)
}

// --- builds ---
const builds = P.join(state, "builds")
const wt1 = P.join(builds, sha1.slice(0, 12))
const wt2 = P.join(builds, sha2.slice(0, 12))
mk(builds)
git(repo, "worktree", "add", "--detach", wt1, sha1)
git(repo, "worktree", "add", "--detach", wt2, sha2)
put(P.join(wt1, "node_modules", "pkg", "lib.bin"), 6 * MB)
const ro = P.join(wt1, "node_modules", "pkg", "readonly.bin")
put(ro, 2 * MB)
fs.chmodSync(ro, 0o444)
link(outside, P.join(wt1, "node_modules", "escape"))
// A path past Windows' 260 characters.
let deep = P.join(wt1, "node_modules")
while (deep.length < 300) deep = P.join(deep, "deep-folder-name-xx")
put(P.join(deep, "leaf.bin"), MB)
put(P.join(builds, "out", hash(wt1), win ? "app.exe" : "app"), 3 * MB)
put(P.join(builds, `${hash(wt1)}.key`), "k")
put(P.join(wt2, "dist", "out.bin"), 5 * MB)
put(P.join(builds, "out", hash(P.join(scratch, "a-folder-variant")), "app.bin"), 2 * MB)
put(P.join(builds, "cache", "aaaa1111bbbb2222", "target", "lib.rlib"), 7 * MB)
const lockedDir = P.join(builds, "cache", "cccc3333dddd4444")
const locked = P.join(lockedDir, "held.bin")
put(locked, 2 * MB)
put(P.join(lockedDir, "free.bin"), 3 * MB)
put(P.join(builds, "times.json"), "{}")
// A worktree git records with relative paths (worktree.useRelativePaths).
const wt3 = P.join(builds, sha3.slice(0, 12))
if (relative) {
  git(repo, "-c", "worktree.useRelativePaths=true", "worktree", "add", "--detach", wt3, sha3)
  put(P.join(wt3, "dist", "rel.bin"), 2 * MB)
}
// A filesystem mounted inside an entry (Linux, as root): its mount point stays, its file is never entered.
const mountDir = P.join(builds, "cache", "eeee5555ffff6666", "m")
if (root) {
  mk(mountDir)
  spawnSync("mount", ["-t", "tmpfs", "-o", "size=8m", "tmpfs", mountDir])
  put(P.join(mountDir, "inmount.txt"), "a mounted filesystem's file")
  put(P.join(builds, "cache", "eeee5555ffff6666", "beside.bin"), MB)
}
// A large compile cache nobody walked yet: a status with a short time cannot finish it.
const bigDir = P.join(builds, "cache", "bbbb7777bbbb7777")
// Each file past what NTFS keeps inside its own records (about 700 bytes), which Git's `du` counts as 1 KB.
for (let i = 0; i < (win ? 30_000 : 60_000); i++) put(P.join(bigDir, String(i), "a.o"), "x".repeat(2000))

// --- seeds, linked into a dead strobe's launch home, whose app wrote lines aiming outside ---
const seed = P.join(state, "seeds", "5eed5eed5eed5eed", "profile.db")
put(seed, 9 * MB)
fs.chmodSync(seed, 0o444)
const home = P.join(state, "run", "home-4194301-1")
mk(home)
fs.linkSync(seed, P.join(home, "profile.db"))
put(P.join(outside, "canary2.txt"), "a file the home's .strobe-shared names")
link(outside, P.join(home, "escape"))
put(
  P.join(home, ".strobe-shared"),
  [`${P.join(home, "profile.db")}\t${seed}`, `${P.join(outside, "canary2.txt")}\t${P.join(outside, "canary2.txt")}`, `${P.join(home, "escape", "canary.txt")}\t${seed}`].join("\n") + "\n",
)
put(P.join(home, "cache.bin"), MB)

// --- the rest of the caches ---
put(P.join(state, "gpu-cache", "exp_000001-head", "dirs", "x", "shader.bin"), 2 * MB)
put(P.join(state, "symbols", "lib.pdb", "ABCDEF1", "lib.pdb"), 3 * MB)
put(P.join(state, "sdk", "0000000000000001", "index.ts"), "x")
age(P.join(state, "sdk", "0000000000000001"), 3)
put(P.join(state, "sdk", "0000000000000002", "index.ts"), "x")
put(P.join(state, "engine", "v0.0", "tp.bin"), 2 * MB)
put(P.join(state, "run", "q-4194302-1-1.sql"), "select 1")
put(P.join(state, "run", "look-1.png"), MB)
age(P.join(state, "run", "look-1.png"), 2)
put(P.join(state, "run", "look-2.png"), MB)
mk(P.join(state, "run", "benches"))
// `strobe run --base/--head`'s bench file, two days old, which an experiment that can resume names.
put(P.join(state, "run", "cmd-aaaa11112222", "run.bench.ts"), "export default {}")
age(P.join(state, "run", "cmd-aaaa11112222"), 2)
put(P.join(state, "experiments", "exp_resume", "state.json"), JSON.stringify({ args: { pos: [P.join(state, "run", "cmd-aaaa11112222", "run.bench.ts")] } }))

// --- data ---
const rec = (id: string, days: number, meta = true) => {
  const d = P.join(state, "recordings", id)
  if (meta) put(P.join(d, "meta.json"), `{"id":"${id}"}`)
  put(P.join(d, "trace.pftrace"), 4 * MB)
  if (days) age(d, days)
}
rec("rec_old001", 40)
rec("rec_old002", 40)
rec("rec_new003", 0)
rec("rec_draft4", 0, false)
rec("rec_old005", 40)
const exp = (id: string, days: number, names: string[]) => {
  const d = P.join(state, "experiments", id)
  put(P.join(d, "result.json"), JSON.stringify({ id, series: names.map((r) => ({ recording: r })) }))
  put(P.join(d, "runner.log"), MB)
  if (days) age(d, days)
}
exp("exp_old001", 40, ["rec_old001"])
// A kept bisect names its step experiment, which names its own recording.
exp("exp_keep02", 0, ["rec_old002", "exp_keep02_s1"])
exp("exp_keep02_s1", 40, ["rec_old005"])
exp("exp_track3", 40, [])
age(P.join(state, "experiments", "exp_resume"), 40)
put(P.join(state, "experiments", "index.jsonl"), "")
put(P.join(state, "history", "dev", "ser_1.jsonl"), `{"exp":"exp_track3"}\n`)
// No daemon answers this state's port (the real one, another build, would keep other engines).
put(P.join(state, "state.json"), JSON.stringify({ mcp_port: 47994 }))

// --- live processes: one holds wt2's lease as a bench does, one holds a file open ---
const stamp2 = `${wt2}.used`
fs.writeFileSync(stamp2, "")
const ps = (script: string) => spawn("powershell", ["-NoProfile", "-Command", script], { stdio: "ignore" })
const holders = win
  ? [
      ps(`$f=[IO.File]::Open('${stamp2}','OpenOrCreate','ReadWrite','ReadWrite,Delete'); $f.Lock(0,1); Start-Sleep 600`),
      ps(`$f=[IO.File]::Open('${locked}','Open','Read','None'); Start-Sleep 600`),
    ]
  : // In a group of its own: the sleep flock starts holds the lock too, and ends with the group.
    [spawn("flock", ["-s", stamp2, "sleep", "600"], { stdio: "ignore", detached: true })]
await Bun.sleep(3000)

try {
  // --- a short status cannot walk the big cache: no exact number, an "at least" ---
  const short = strobe("status", "--fast", "--json").json?.disk
  const shortText = strobe("status", "--fast").out.split("\n").find((l) => / disk /.test(l)) ?? ""
  check(
    short?.complete === false && short?.total_bytes === null && short?.at_least_bytes > 0 && short?.categories?.find((c: any) => c.name === "builds")?.bytes === null,
    "a walk the deadline cut gives no exact total (total_bytes and builds' bytes null, at_least_bytes kept)",
    `complete ${short?.complete}, total ${short?.total_bytes}, at least ${short?.at_least_bytes}, unmeasured ${short?.unmeasured}, ${short?.seconds} s`,
  )
  check(/at least [^(]+\(at least /.test(shortText), "  and its line says at least, inside the parentheses too", shortText.trim())

  // --- status: its total against du ---
  const truth = du(state)
  const st = strobe("status", "--fast", "--disk", "--json")
  const disk = st.json?.disk
  const inState = disk ? disk.total_bytes - disk.outside_bytes : NaN
  check(within(inState, truth, 1), "status total = du of the state folder (1%)", `strobe ${inState}, du ${truth}`)
  check(disk?.complete === true, "status --disk measured every entry")

  // --- a plan alone takes no lock ---
  const lockFile = P.join(state, "run", "prune.lock")
  const dry0 = strobe("prune", "--dry-run")
  check(dry0.code === 0 && !fs.existsSync(lockFile), "prune --dry-run takes no lock and makes no lock file")

  // --- the plan, without a terminal: nothing goes ---
  const before = listing(scratch)
  const noTty = strobe("prune")
  check(noTty.code === 3 && /--force/.test(noTty.out), "prune without a terminal or --force removes nothing (exit 3, names --force)", `exit ${noTty.code}`)
  const diff = (a: string[], b: string[]) => [...a.filter((x) => !b.includes(x)).map((x) => `-${x}`), ...b.filter((x) => !a.includes(x)).map((x) => `+${x}`)].join(" ")
  check(JSON.stringify(listing(scratch)) === JSON.stringify(before), "  nothing changed", diff(before, listing(scratch)))
  const dry = strobe("prune", "--dry-run", "--json")
  check(dry.code === 0 && JSON.stringify(listing(scratch)) === JSON.stringify(before), "prune --dry-run removes nothing", `exit ${dry.code} ${diff(before, listing(scratch))}`)
  console.log(strobe("prune", "--dry-run").out)
  const planned = new Set<string>(dry.json.remove.map((e: any) => e.name))
  const want = [
    `builds/${sha1.slice(0, 12)}`, `builds/out/${hash(P.join(scratch, "a-folder-variant"))}`, "builds/cache/aaaa1111bbbb2222", "builds/cache/cccc3333dddd4444",
    "builds/cache/bbbb7777bbbb7777", "seeds/5eed5eed5eed5eed", "gpu-cache/exp_000001-head", "symbols/lib.pdb/ABCDEF1", "sdk/0000000000000001", "engine/v0.0",
    "run/home-4194301-1", "run/q-4194302-1-1.sql", "run/look-1.png",
    ...(relative ? [`builds/${sha3.slice(0, 12)}`] : []),
    ...(root ? ["builds/cache/eeee5555ffff6666"] : []),
  ]
  check(want.every((w) => planned.has(w)) && planned.size === want.length, "the default plan is every unused cache", [...planned].sort().join(", "))
  const keptWhy = Object.fromEntries(dry.json.kept.map((e: any) => [e.name, e.why]))
  check(/in use/.test(keptWhy[`builds/${sha2.slice(0, 12)}`] ?? ""), "the worktree a live process leases stays", keptWhy[`builds/${sha2.slice(0, 12)}`])
  check(keptWhy["sdk/0000000000000002"] !== undefined && keptWhy["run/look-2.png"] !== undefined, "a recent other build's SDK and a new temporary file stay")
  check(/resume/.test(keptWhy["run/cmd-aaaa11112222"] ?? ""), "run/cmd-<hash> stays while an experiment that can resume names it", keptWhy["run/cmd-aaaa11112222"])
  const frees = dry.json.frees_bytes

  // --- remove: exactly the plan ---
  const duBefore = du(scratch)
  const gone = strobe("prune", "--force", "--json")
  const duAfter = du(scratch)
  const removed = new Set<string>(gone.json.removed.map((e: any) => e.name))
  const failed = gone.json.failed.map((e: any) => e.name)
  const wantFailed = [...(win ? ["builds/cache/cccc3333dddd4444"] : []), ...(root ? ["builds/cache/eeee5555ffff6666"] : [])]
  check(JSON.stringify([...failed].sort()) === JSON.stringify(wantFailed.sort()) && gone.code === (wantFailed.length ? 1 : 0), "only the entries with a held file or a mount fail, named, and the rest goes", `failed ${failed.join(", ")}, exit ${gone.code}`)
  check([...removed].every((r) => planned.has(r)) && removed.size + failed.length === planned.size, "--force removes exactly the plan", [...removed].sort().join(", "))
  check(fs.readFileSync(P.join(outside, "canary.txt"), "utf8") === "do not delete" && fs.existsSync(P.join(outside, "big.bin")), "the canary behind the link is untouched")
  check(fs.existsSync(P.join(outside, "canary2.txt")) && fs.existsSync(P.join(outside, "canary.txt")), "lines of the dead home's .strobe-shared aiming outside (a path, a link) touched nothing")
  if (root) {
    const why = gone.json.failed.find((f: any) => f.name === "builds/cache/eeee5555ffff6666")?.errors?.join(" ") ?? ""
    check(fs.existsSync(P.join(mountDir, "inmount.txt")) && /another filesystem/.test(why) && !fs.existsSync(P.join(builds, "cache", "eeee5555ffff6666", "beside.bin")), "a mounted filesystem inside an entry is never entered; its neighbour goes", why)
  }
  if (relative) check(!fs.existsSync(wt3), "the worktree git records with relative paths is removed")
  check(fs.existsSync(P.join(wt2, "dist", "out.bin")), "the leased worktree is still there")
  check(fs.existsSync(P.join(state, "run", "benches")) && fs.existsSync(P.join(builds, "times.json")), "the bench registry and build times are kept")
  if (win) check(fs.existsSync(locked) && !fs.existsSync(P.join(lockedDir, "free.bin")), "the held file stays, its neighbour went")
  const slash = (s: string) => s.replaceAll("\\", "/").toLowerCase()
  const wtList = slash(git(repo, "worktree", "list", "--porcelain"))
  check(!wtList.includes(slash(wt1)) && !wtList.includes(slash(wt3)) && !/prunable/.test(wtList) && wtList.includes(slash(wt2)), "git worktree list: the removed worktrees are gone, nothing stale", wtList.split("\n").filter((l) => l.startsWith("worktree")).join(" | "))
  const reclaimed = gone.json.reclaimed_bytes
  check(within(reclaimed, duBefore - duAfter, 1), "Total reclaimed space = du before - du after (1%)", `strobe ${reclaimed}, du ${duBefore - duAfter}`)
  check(within(reclaimed, frees - (win ? 2 * MB : 0), 2), "  and = the plan's frees, less the held file (2%)", `plan ${frees}`)

  // --- data: only when named, older than 30 days, never a recording a kept experiment names ---
  const all = strobe("prune", "--all", "--force", "--json")
  const gone2 = new Set<string>(all.json.removed.map((e: any) => e.name))
  const kept2 = Object.fromEntries(all.json.kept.map((e: any) => [e.name, e.why]))
  check(gone2.has("rec_old001") && gone2.has("exp_old001"), "--all removes an old experiment and the old recording only it names")
  check(/experiment that stays/.test(kept2["rec_old002"] ?? ""), "an old recording a kept experiment names stays", kept2["rec_old002"])
  check(/history/.test(kept2["exp_track3"] ?? ""), "an old experiment tracked history names stays", kept2["exp_track3"])
  check(/experiment that stays/.test(kept2["exp_keep02_s1"] ?? "") && /experiment that stays/.test(kept2["rec_old005"] ?? ""), "a kept bisect's old step experiment, and the recording it names, stay", `${kept2["exp_keep02_s1"]} · ${kept2["rec_old005"]}`)
  check(!gone2.has("rec_new003") && !gone2.has("rec_draft4") && !gone2.has("exp_keep02"), "new data stays")

  // --- status again, against du ---
  const truth2 = du(state)
  const st2 = strobe("status", "--fast", "--disk", "--json").json.disk
  check(within(st2.total_bytes - st2.outside_bytes, truth2, 1), "status total after pruning = du (1%)", `strobe ${st2.total_bytes - st2.outside_bytes}, du ${truth2}`)
} finally {
  for (const h of holders) {
    if (win || !h.pid) h.kill()
    else process.kill(-h.pid)
  }
  if (root) spawnSync("umount", [mountDir])
}
const failedChecks = results.filter((r) => r.startsWith("FAIL")).length
console.log(`\n${results.length - failedChecks} passed, ${failedChecks} failed · scratch left in ${scratch}`)
process.exit(failedChecks ? 1 : 0)
