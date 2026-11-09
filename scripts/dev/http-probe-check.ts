// The HTTP probe's known answers (http-probe.bench.ts, SPEC §8.9) and its secret hygiene, dev only, not shipped:
// runs every case as a --check with a fresh random token (the probe's bearer token, also in its URL's query),
// checks what strobe read against the truth, times `mixed` briefly (A/A: clean pairs, then a probe pass), records
// one case (record --bench: its attribution block probes too),
// builds each recording's Perfetto trace and the experiments' reports, then reads every file strobe wrote for them
// (gzipped ones unpacked), every answer it printed, for the token. Exit 0 only when every check held.
//   bun scripts/dev/http-probe-check.ts [path/to/strobe]
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { gunzipSync } from "node:zlib"

const strobe = process.argv[2] ?? "strobe"
const repo = join(import.meta.dir, "..", "..")
const bench = join(import.meta.dir, "http-probe.bench.ts")
const token = [...crypto.getRandomValues(new Uint8Array(24))].map((b) => b.toString(16).padStart(2, "0")).join("")
const started = Date.now() - 1000
const printed: string[] = []
let failed = 0
const check = (ok: boolean, what: string) => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}`)
  if (!ok) failed++
}

// The token reaches the bench file through a file, and the server only as its hash: no process strobe starts has it in
// its environment, and the server's heap (an attribution block keeps its heap profile) never holds it.
const scratch = mkdtempSync(join(tmpdir(), "http-probe-token-"))
const tokenFile = join(scratch, "token")
await Bun.write(tokenFile, token)
const tokenEnv = { HTTP_PROBE_TOKEN_FILE: tokenFile, HTTP_PROBE_TOKEN_SHA256: new Bun.CryptoHasher("sha256").update(`Bearer ${token}`).digest("hex") }

function run(args: string[], env: Record<string, string> = {}): any {
  const p = Bun.spawnSync([strobe, ...args, "--json", "--progress", "off"], {
    cwd: repo,
    env: { ...process.env, ...tokenEnv, ...env },
    stdout: "pipe",
    stderr: "pipe",
    timeout: 10 * 60_000,
  })
  const out = p.stdout.toString()
  printed.push(out, p.stderr.toString())
  try {
    return JSON.parse(out)
  } catch {
    throw new Error(`strobe ${args.join(" ")} (exit ${p.exitCode}): ${p.stderr.toString().slice(-2000)}`)
  }
}

const series = (r: any, c: string) => r.series.find((s: any) => s.case === c)
const val = (s: any, m: string) => s?.values?.[m] as number | undefined
const refusal = (s: any, m: string) => String(s?.refused?.[m] ?? "")

// Every case but driver-delayed, as checks.
const exp = run(["bench", bench, "--head", ".", "--check", "--filter", "stall-750ms,stall-750ms-long,status-500,wrong-json"])
const stall = series(exp, "stall-750ms")
check(stall?.state === "pass", `stall-750ms passed (${stall?.error ?? ""})`)
check((val(stall, "http.timeouts") ?? 0) >= 3 && (val(stall, "http.timeouts") ?? 9) <= 5, `stall-750ms: 3 to 5 timeouts at 400 ms (${val(stall, "http.timeouts")})`)
for (const m of ["http.latency.p95", "http.latency.max"]) check(/^RunIncomplete: .*right-censored/.test(refusal(stall, m)), `stall-750ms: ${m} refused as right-censored, never 400 (${refusal(stall, m) || val(stall, m)})`)
check(val(stall, "http.idle.p50") !== undefined, `stall-750ms: the idle control answered (${val(stall, "http.idle.p50")} ms)`)
const view = stall?.recording ? run(["query", stall.recording, "http"]) : null
const cohort = view?.cohorts?.[0]
check(!!cohort && cohort.probes >= 7 && cohort.probes <= 9, `stall-750ms: one cohort of 7 to 9 probes (${JSON.stringify(cohort?.latencies_ms)})`)
const lat: (number | null)[] = cohort?.latencies_ms ?? []
const answered = lat.filter((l): l is number => l !== null)
check(lat.slice(0, 3).every((l) => l === null) && answered.length >= 3 && answered.every((l, i) => i === 0 || l < answered[i - 1]), "stall-750ms: timeouts first, then a falling staircase of answers")
check((cohort?.held_ms ?? 0) >= 600 && (cohort?.held_ms ?? 0) <= 900, `stall-750ms: the server held them ≈750 ms (${cohort?.held_ms})`)

const long = series(exp, "stall-750ms-long")
check(long?.state === "pass", `stall-750ms-long passed (${long?.error ?? ""})`)
check(val(long, "http.timeouts") === 0, `stall-750ms-long: no timeout at 1500 ms (${val(long, "http.timeouts")})`)
check((val(long, "http.latency.max") ?? 0) >= 650 && (val(long, "http.latency.max") ?? 9e9) <= 850, `stall-750ms-long: the slowest ≈750 ms (${val(long, "http.latency.max")})`)

for (const c of ["status-500", "wrong-json"]) {
  const s = series(exp, c)
  check(s?.state === "pass", `${c} passed (${s?.error ?? ""})`)
  check((val(s, "http.errors") ?? 0) >= 2, `${c}: errors counted (${val(s, "http.errors")})`)
  for (const m of ["http.latency.p50", "http.latency.p95", "http.latency.max"]) check(/^RunIncomplete: .*no response that counts/.test(refusal(s, m)), `${c}: ${m} refused, never a fast answer (${refusal(s, m) || val(s, m)})`)
}

// strobe's own scheduler held 400 ms, 400 ms into the run (after 100 ms of settling and the 500 ms idle control).
const late = run(["bench", bench, "--head", ".", "--check", "--filter", "driver-delayed"], { STROBE_PROBE_PAUSE: "1000,400" })
const d = series(late, "driver-delayed")
check(d?.state === "pass", `driver-delayed passed (${d?.error ?? ""})`)
check((val(d, "http.missed_sends") ?? 0) >= 2, `driver-delayed: missed sends (${val(d, "http.missed_sends")})`)
check(val(d, "http.timeouts") === 0 && val(d, "http.errors") === 0, `driver-delayed: no timeout, no error: not the server (${val(d, "http.timeouts")}, ${val(d, "http.errors")})`)
check(/^OverheadBudget: .*missed/.test(refusal(d, "http.latency.p50")), `driver-delayed: latencies refused as strobe's (${refusal(d, "http.latency.p50") || val(d, "http.latency.p50")})`)

// A quick timed run of `mixed` (A/A): clean timing pairs whose setup still asks for the probe, then a probe pass.
const mixed = run(["bench", bench, "--head", ".", "--aa", "--pairs", "6", "--runs", "2", "--filter", "mixed"])
const mx = series(mixed, "mixed")
check(!!mx && !mx.error && (mx.pairs ?? 0) > 0, `mixed: clean timing pairs ran (${mx?.pairs ?? 0} pairs${mx?.error ? `: ${mx.error}` : ""})`)
check(mx?.pass?.http === true && (mx.pass.pairs?.length ?? 0) > 0, `mixed: a probe pass after them (${mx?.pass?.pairs?.length ?? 0} probe pairs)`)

// record --bench: the check and the attribution block, both probed.
const rec = run(["record", "--bench", `${bench}#stall-750ms`, "--head", "."])
const recs: string[] = (rec.recordings ?? []).map((r: any) => r.recording).filter((r: any) => typeof r === "string")
// A Chromium page's attribution block (headless): the probe carries a credential, so no heap snapshot.
const page = run(["record", "--bench", `${bench}#page`, "--head", ".", "--headless"])
const pageRec: string | undefined = page.recordings?.[0]?.recording
check(!!pageRec, `page: a Chromium attribution recording (${pageRec})`)
const stateDir = process.platform === "win32" ? join(process.env.LOCALAPPDATA!, "strobe") : join(process.env.XDG_DATA_HOME ?? join(process.env.HOME!, ".local", "share"), "strobe")
if (pageRec) {
  recs.push(pageRec)
  check(!existsSync(join(stateDir, "recordings", pageRec, "heap-0.heapsnapshot.gz")), `page: no heap snapshot beside the probe's credential`)
  check(existsSync(join(stateDir, "recordings", pageRec, "trace.pftrace")), `page: its trace is kept`)
}
for (const r of exp.series.map((s: any) => s.recording).concat(late.series.map((s: any) => s.recording))) if (r) recs.push(r)
check(recs.length >= 6, `recordings kept: ${recs.join(" ")}`)
for (const r of recs) {
  const o = run(["open", r, "--no-server"])
  check((o.built?.tracks?.["HTTP probe latency"] ?? 0) > 0, `${r}: Perfetto holds the probe (${JSON.stringify(o.built?.tracks ?? {})})`)
}

// Every file strobe wrote for these, and every answer it printed.
const out = mkdtempSync(join(tmpdir(), "http-probe-check-"))
for (const e of [exp.id, late.id, mixed.id]) run(["report", e, "--out", join(out, `${e}.html`)])
const state = stateDir
// Every recording and experiment written since this check began (record --bench's experiment and check block
// too), the reports, and the run folder strobe's runner and hosts use.
const since = (d: string) => (existsSync(d) ? readdirSync(d).map((n) => join(d, n)).filter((p) => statSync(p).mtimeMs >= started) : [])
const roots = [...since(join(state, "recordings")), ...since(join(state, "experiments")), ...since(join(state, "run")), out]
const needles = [token, Buffer.from(token).toString("base64"), token.toUpperCase()]
let files = 0
const seen: string[] = []
const walk = (p: string) => {
  let dir: boolean
  try {
    dir = statSync(p).isDirectory()
  } catch {
    return
  }
  if (dir) return readdirSync(p).forEach((n) => walk(join(p, n)))
  let bytes: Buffer
  try {
    bytes = readFileSync(p)
    if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = gunzipSync(bytes)
  } catch {
    return
  }
  files++
  const text = bytes.toString("latin1")
  if (needles.some((n) => text.includes(n))) seen.push(p)
}
roots.forEach(walk)
check(files > 0 && seen.length === 0, `the token is in none of the ${files} files strobe wrote${seen.length ? `: ${seen.join(", ")}` : ""}`)
check(!printed.some((t) => needles.some((n) => t.includes(n))), "the token is in nothing strobe printed")
rmSync(out, { recursive: true, force: true })
rmSync(scratch, { recursive: true, force: true })
console.log(failed ? `${failed} checks failed` : "every check held")
process.exit(failed ? 1 : 0)
