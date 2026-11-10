// Known-answer bench for `strobe query <rec> insights` on pages (dev only, not shipped): each case is built to fire
// one rule, or to stay silent where a neighbouring rule would misfire. From the repo root (noisy with --check):
//   strobe bench scripts/dev/insights.bench.ts --head . --check --headless
// then `strobe query <rec> insights` of each case's recording. Truth per run:
//   restyle  a click that flips one class on 6,000 boxes: style + layout tens of ms, script under 1 ms
//            → style-layout-dominant fires
//   script   a click whose listener spins 50 ms: script 50 ms, style and layout ~0 → style-layout-dominant silent
//   waits    a fetch the server answers after 300 ms: run.duration ≥ 300 ms, cpu.work a few ms → off-cpu-bound fires
//   busy     a 300 ms spin: cpu.work ≈ run.duration → off-cpu-bound silent
// The native rules (dominant-thread, wakes-while-idle on idle blocks) use main-thread.bench.ts's app-* cases:
// app-worker-spin fires dominant-thread, app-main-spin stays silent (its main thread does the work); app-timer-wait
// fires wakes-while-idle (a 10 ms interval), app-main-spin stays silent (busy: its switches are preemptions).
import { bench, web } from "@hona/strobe"

const page = `<!doctype html><title>insights</title>
<style>
#boxes { display: flex; flex-wrap: wrap; width: 1200px }
#boxes div { width: 40px; height: 12px; margin: 1px; font: 10px sans-serif }
body.alt #boxes div { width: 52px; height: 14px; margin: 2px; font: 11px serif; letter-spacing: 1px }
</style>
<button id="restyle">restyle</button> <button id="spin">spin</button>
<div id="boxes"></div>
<script>
const boxes = document.getElementById("boxes")
for (let i = 0; i < 6000; i++) { const d = document.createElement("div"); d.textContent = i; boxes.append(d) }
document.getElementById("restyle").onclick = () => document.body.classList.toggle("alt")
const spin = (ms) => { const end = performance.now() + ms; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 1); return x > 0 }
document.getElementById("spin").onclick = () => spin(50)
window.known = { spin }
</script>`

let server: { stop(force?: boolean): unknown; port?: number } | null = null

export default bench({
  targets: {
    page: web({
      prepare() {
        server ??= Bun.serve({
          hostname: "127.0.0.1",
          port: 0,
          fetch: async (r) => {
            if (new URL(r.url).pathname === "/slow") {
              await Bun.sleep(300)
              return new Response("ok", { headers: { "cache-control": "no-store" } })
            }
            return new Response(page, { headers: { "content-type": "text/html", "cache-control": "no-store" } })
          },
        })
        return { url: `http://127.0.0.1:${server.port}/` }
      },
      url: (ctx) => ctx.prepared.url,
      teardown() {
        server?.stop(true)
        server = null
      },
    }),
  },
  setup: ({ page }) => page.waitFor({ selector: "#restyle" }, { timeout: 10000 }),
  cases: {
    restyle: { primary: "cpu.work", metrics: ["main.style", "main.layout", "main.script", "main.work"], run: ({ page }) => page.click("#restyle") },
    script: { primary: "cpu.work", metrics: ["main.style", "main.layout", "main.script", "main.work"], run: ({ page }) => page.click("#spin") },
    waits: { primary: "run.duration", metrics: ["cpu.work"], run: ({ page }) => page.eval(async () => (await fetch("/slow", { cache: "no-store" })).text()) },
    busy: { primary: "run.duration", metrics: ["cpu.work"], run: ({ page }) => page.eval(() => (window as any).known.spin(300)) },
  },
})
