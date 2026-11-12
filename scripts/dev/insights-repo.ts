// Known answers for `strobe query <exp> insights` (dev, not shipped): a throwaway git repository with two commits,
// c0 and c1, whose bench cases each make one suspect fire, or keep it silent where a neighbouring rule would misfire.
// The commits differ only in level.json, which the bench file reads from each variant's folder (ctx.root):
//
//   bun scripts/dev/insights-repo.ts <dir>
//
// Truth per run (c0 → c1):
//   regress      hotPath spins 20 → 32 ms: a comparison decides cpu.work more (+60%); its attribution blocks
//                move hotPath by about +12 ms (a mover), and hot-function fires on both sides (base too);
//                --why never: no suspects, and the two record --bench commands that profile each side
//   steady       hotPath spins 20 ms on both: same (±2%): no attribution blocks, "nothing to explain"
//   spread       five functions spin 5 ms each: hot-function silent (none holds over 25% of the samples)
//   waits        a 300 ms sleep: off-cpu-bound fires (the host's main thread ran under 1 ms of a 300 ms run; an
//                attribution block's cpu.work is mostly Bun's sampler, a core while the script sleeps)
//   busy         a 300 ms spin: off-cpu-bound silent
//   cli-regress  a command spins 30 → 45 ms: more, and no attribution block (a command ends inside each run)
//   leak         a page whose run keeps 0 → 2,000 Retained objects: js heap no growth → leaks, Retained the
//                first suspect, with its constructor's location in page.html; base's census lists no suspect
//   http-stall   the server's thread held 450 ms twice a run: http-stalls fires (two cohorts a run)
//   http-calm    the server idles 800 ms: http-stalls silent
//   http-untimed the server's thread held 450 ms in each run's ctx.untimed part only: http-stalls silent (its cohorts
//                are outside the measured runs' timed parts)
//   swap         c0 spins 30 ms in fnA, c1 in fnB (same cost): hot-function on head's fnB "over its threshold on head
//                only", base's fnA "base only", never "(base too)" (same rule, another function)
//   twofiles     work() of server/auth/util.ts and of client/auth/util.ts, 10 ms each: two rows in a..b hotspots (a
//                path cut to its last two steps made them one)
//   long-task    (batch.bench.ts) a click that spins 80 ms: one long task; noop: nothing. With --check --batch 2
//                they share one launch's recording: no case is given the other's long task
//   wait-driver  a page run that waits 300 ms for an element strobe's driver polls for: hot-function never names
//                strobe-driver.js
//   restyle      a page whose run toggles the width of 100 → 20,000 boxes: style and layout over script on head
//                only; base measured style and layout below the threshold, so head's hit reads "over its threshold on
//                head only", never "base not read for it"
//   port, sized, rand  (more.bench.ts; scripts/dev/next-commands.ts --make) a case of two targets (lib, lib2), a case of a
//                param n (2, and 4 with --params all), a case whose spin draws on ctx.random(): what each printed
//                rerun and profile command must keep (its target, its own params, the seed)
//
// Commands (from <dir>; --headless keeps the page off the screen):
//   strobe bench insights.bench.ts --base c0 --head c1 --filter regress,steady,cli-regress --pairs 6 --runs 3
//   strobe bench insights.bench.ts --base c0 --head c1 --filter regress --pairs 6 --runs 3 --why never
//   strobe bench insights.bench.ts --aa --head c1 --filter steady --pairs 3 --runs 3
//   strobe bench insights.bench.ts --head c1 --check --why always --filter regress,spread,waits,busy,http-stall,http-calm,http-untimed
//   strobe bench insights.bench.ts --head c1 --check --filter regress,waits
//   strobe bench insights.bench.ts --head c1 --blocks 2 --why always --filter regress --runs 3
//   strobe bench insights.bench.ts --base c0 --head c1 --probes memory --filter leak --headless --repeat 5
//   strobe bench insights.bench.ts --base c0 --head c1 --filter swap --pairs 6 --runs 3 --why always
//   strobe bench insights.bench.ts --aa --head c1 --filter steady --pairs 6 --runs 3 --why always   (A/A: no mover is a suspect)
//   strobe bench insights.bench.ts --base c0 --head . --filter regress,twofiles --pairs 6 --runs 3 --why always
//                (head the working tree at c1: one hotPath row, harness.ts on both sides; two work rows)
//   strobe bench batch.bench.ts --head c1 --check --batch 2 --headless
//   strobe bench insights.bench.ts --head c1 --check --why always --filter wait-driver --headless
//   strobe bench insights.bench.ts --base c0 --head c1 --filter restyle --pairs 4 --runs 3 --why always --headless
//   two plain folders, no .git, in a folder whose name holds a space (`git archive c0 | tar -x -C "<d> x/v1"`, the
//   same of c1 in v2), from <dir>; on Windows name one with a lowercase drive letter:
//   strobe bench insights.bench.ts --base "<d> x/v1" --head "<d> x/v2" --filter regress,twofiles --pairs 6 --runs 3 --why always
//                (one hotPath row of about +12 ms and two work rows, and hot-function on both sides: base too)
//   strobe bench more.bench.ts --base lib --head lib2 --check --filter port
//   strobe bench more.bench.ts --head c1 --check --filter sized --params all
//   strobe bench more.bench.ts --base c0 --head c1 --check --filter rand --runs 2 --pin none --power keep
// then `strobe query <exp> insights` of each.
import { execFileSync } from "node:child_process"
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"

const dir = resolve(process.argv[2] ?? "")
if (!process.argv[2]) {
  console.error("usage: bun scripts/dev/insights-repo.ts <dir>")
  process.exit(2)
}
rmSync(dir, { recursive: true, force: true })
mkdirSync(dir, { recursive: true })
const git = (...a: string[]) => execFileSync("git", ["-C", dir, ...a], { stdio: "pipe" }).toString()
git("init", "-q", "-b", "main")
git("config", "user.email", "insights@example.invalid")
git("config", "user.name", "insights")

writeFileSync(
  `${dir}/harness.ts`,
  `// The script() target: exports whose costs are known by construction.
// Each spins in its own body, so a profile bills its time to it (a shared helper would take every sample).
export function hotPath(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 1); return x > 0 }
function partA(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 1); return x }
function partB(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 2); return x }
function partC(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 3); return x }
function partD(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 4); return x }
function partE(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 5); return x }
export function spread(ms: number) { const each = ms / 5; return partA(each) + partB(each) + partC(each) + partD(each) + partE(each) > 0 }
export async function sleepFor(ms: number) { await Bun.sleep(ms); return true }
export function fnA(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 6); return x > 0 }
export function fnB(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 7); return x > 0 }
import { work as workA } from "./server/auth/util.ts"
import { work as workB } from "./client/auth/util.ts"
export function twoFiles(ms: number) { return workA(ms) + workB(ms) > 0 }
/** Holds this thread (the server's event loop) for \`ms\`: no request is answered meanwhile. */
export function stall(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 1); return x > 0 }
let server: ReturnType<typeof Bun.serve> | undefined
export function startServer() {
  server ??= Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => Response.json({ status: "ok" }) })
  return \`http://127.0.0.1:\${server.port}/health\`
}
`,
)
// Two files whose last two steps are the same, one function name each: two functions, never one row.
for (const d of ["server", "client"]) {
  mkdirSync(`${dir}/${d}/auth`, { recursive: true })
  writeFileSync(`${dir}/${d}/auth/util.ts`, `export function work(ms: number) { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + ${d === "server" ? 8 : 9}); return x }\n`)
}
writeFileSync(
  `${dir}/spin.ts`,
  `// The cli() target: spins level.json's cli milliseconds.
import { readFileSync } from "node:fs"
const ms = JSON.parse(readFileSync("level.json", "utf8")).cli
const end = performance.now() + ms
let x = 0
while (performance.now() < end) x += Math.sqrt(x + 1)
if (x < 0) console.log(x)
`,
)
writeFileSync(
  `${dir}/page.html`,
  `<!doctype html><title>insights</title>
<style>.box { display: inline-block; width: 10px; height: 4px; margin: 1px } body.wide .box { width: 12px }</style>
<p>leak page</p>
<button id="spin" onclick="longTask(80)">spin</button>
<script>
class Retained { constructor(i) { this.i = i; this.payload = new Array(16).fill(i) } }
window.kept = []
window.leakStep = (n) => { for (let i = 0; i < n; i++) window.kept.push(new Retained(i)); const tmp = []; for (let i = 0; i < 2000; i++) tmp.push(new Retained(i)); return window.kept.length + tmp.length }
// One task of \`ms\` on the main thread.
window.longTask = (ms) => { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 1); return x > 0 }
// An element that appears after \`ms\`, for strobe's driver to wait for (and taken away again).
// n boxes (kept between runs), whose width a class on the body sets: toggling it restyles and lays out every box.
window.boxes = (n) => { const have = document.querySelectorAll(".box").length; for (let i = have; i < n; i++) { const d = document.createElement("div"); d.className = "box"; document.body.append(d) } return n }
window.later = (ms) => { document.getElementById("late")?.remove(); setTimeout(() => { const p = document.createElement("p"); p.id = "late"; p.textContent = "late"; document.body.append(p) }, ms); return true }
</script>`,
)
writeFileSync(
  `${dir}/insights.bench.ts`,
  `import { bench, cli, script, web } from "@hona/strobe"
import { readFileSync } from "node:fs"
import { join } from "node:path"

const level = (root: string) => JSON.parse(readFileSync(join(root, "level.json"), "utf8"))
let server: { stop(force?: boolean): unknown; port?: number } | null = null

export default bench({
  targets: {
    lib: script("./harness.ts"),
    cmd: cli("bun spin.ts"),
    page: web({
      prepare(ctx) {
        const html = readFileSync(join(ctx.root, "page.html"), "utf8")
        server ??= Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(html, { headers: { "content-type": "text/html", "cache-control": "no-store" } }) })
        return { url: \`http://127.0.0.1:\${server.port}/\` }
      },
      url: (ctx) => ctx.prepared.url,
      teardown() {
        server?.stop(true)
        server = null
      },
    }),
  },
  async setup(ctx: any) {
    if (ctx.case?.startsWith("http-")) await ctx.httpProbe({ url: await ctx.call("startServer"), cadenceMs: 50, timeoutMs: 1000, idleMs: 300 })
  },
  cases: {
    regress: { targets: ["lib"], primary: "cpu.work", run: (ctx: any) => ctx.call("hotPath", level(ctx.root).hot) },
    steady: { targets: ["lib"], primary: "cpu.work", run: (ctx: any) => ctx.call("hotPath", 20) },
    spread: { targets: ["lib"], primary: "cpu.work", run: (ctx: any) => ctx.call("spread", 25) },
    waits: { targets: ["lib"], primary: "run.duration", metrics: ["cpu.work"], run: (ctx: any) => ctx.call("sleepFor", 300) },
    busy: { targets: ["lib"], primary: "run.duration", metrics: ["cpu.work"], run: (ctx: any) => ctx.call("hotPath", 300) },
    "cli-regress": { targets: ["cmd"], primary: "cpu.work", run: (ctx: any) => ctx.exec() },
    leak: { targets: ["page"], primary: "cpu.work", run: (ctx: any) => ctx.page.eval((n: number) => (window as any).leakStep(n), level(ctx.root).leak) },
    restyle: {
      targets: ["page"],
      primary: "main.work",
      run: async (ctx: any) => {
        await ctx.untimed(() => ctx.page.eval((n: number) => (window as any).boxes(n), level(ctx.root).restyle))
        await ctx.page.eval(() => document.body.classList.toggle("wide"))
      },
    },
    "http-stall": {
      targets: ["lib"],
      primary: "http.latency.p95",
      metrics: ["http.timeouts"],
      run: async ({ call }: any) => {
        await call("stall", 450)
        await call("sleepFor", 150)
        await call("stall", 450)
        await call("sleepFor", 150)
      },
    },
    "http-calm": { targets: ["lib"], primary: "http.latency.p95", metrics: ["http.timeouts"], run: ({ call }: any) => call("sleepFor", 1200) },
    "http-untimed": {
      targets: ["lib"],
      primary: "http.latency.p95",
      metrics: ["http.timeouts"],
      run: async (ctx: any) => {
        await ctx.untimed(() => ctx.call("stall", 450))
        await ctx.call("sleepFor", 600)
      },
    },
    swap: { targets: ["lib"], primary: "cpu.work", run: (ctx: any) => ctx.call(level(ctx.root).swap === "a" ? "fnA" : "fnB", 30) },
    twofiles: { targets: ["lib"], primary: "cpu.work", run: (ctx: any) => ctx.call("twoFiles", 10) },
    "wait-driver": {
      targets: ["page"],
      primary: "run.duration",
      run: async (ctx: any) => {
        await ctx.page.eval((ms: number) => (window as any).later(ms), 300)
        await ctx.page.waitFor({ selector: "#late" }, { timeout: 5000 })
      },
    },
  },
})
`,
)
// The batch cases in a file of their own: insights.bench.ts's setup reads ctx.case (its HTTP probe), which a
// launch shared by several cases cannot.
writeFileSync(
  `${dir}/batch.bench.ts`,
  `import { bench, web } from "@hona/strobe"
import { readFileSync } from "node:fs"
import { join } from "node:path"

let server: { stop(force?: boolean): unknown; port?: number } | null = null

export default bench({
  targets: {
    page: web({
      prepare(ctx) {
        const html = readFileSync(join(ctx.root, "page.html"), "utf8")
        server ??= Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(html, { headers: { "content-type": "text/html", "cache-control": "no-store" } }) })
        return { url: \`http://127.0.0.1:\${server.port}/\` }
      },
      url: (ctx) => ctx.prepared.url,
      teardown() {
        server?.stop(true)
        server = null
      },
    }),
  },
  cases: {
    "long-task": { primary: "main.work", run: ({ page }: any) => page.click("#spin") },
    noop: { primary: "main.work", run: ({ page }: any) => page.eval(() => true) },
  },
})
`,
)
// next-commands.ts --make: two implementations of the same exports, a param (every case of a file runs each of its
// values), a workload drawn from the seed.
writeFileSync(
  `${dir}/more.bench.ts`,
  `import { bench, script } from "@hona/strobe"

export default bench({
  targets: { lib: script("./harness.ts"), lib2: script("./harness.ts") },
  params: { n: { run: [2], more: [4] } },
  cases: {
    port: { targets: ["lib", "lib2"], primary: "cpu.work", run: (ctx: any) => ctx.call("hotPath", ctx.params.n) },
    sized: { targets: ["lib"], primary: "cpu.work", run: (ctx: any) => ctx.call("hotPath", ctx.params.n) },
    rand: { targets: ["lib"], primary: "cpu.work", run: (ctx: any) => ctx.call("hotPath", 2 + Math.floor(ctx.random() * 3)) },
  },
})
`,
)
writeFileSync(`${dir}/strobe.json`, "{}\n")
for (const [k, l] of [
  [0, { hot: 20, cli: 30, leak: 0, swap: "a", restyle: 100 }],
  [1, { hot: 32, cli: 45, leak: 2000, swap: "b", restyle: 20000 }],
] as const) {
  writeFileSync(`${dir}/level.json`, JSON.stringify(l) + "\n")
  git("add", "-A")
  git("commit", "-q", "-m", `c${k}: ${JSON.stringify(l)}`)
  git("tag", `c${k}`)
}
console.log(`${dir}: c0 and c1 (hotPath 20 → 32 ms, cli 30 → 45 ms, leak 0 → 2000 objects a run)`)
