// Dev bench (not shipped): a browser process that ends inside a run, and why cpu.renderer is refused
// (SPEC 9.6). Chromium, headless:
//   strobe bench scripts/dev/ended-role.bench.ts --head . --check --headless
// The truth, per run (cpu.work keeps what the ended processes used; EndedCpu says how much):
//   swap    the page goes to another site: its renderer, a renderer at the run's start, ends in the run:
//           cpu.renderer refused "a renderer process ended inside the run"
//   unread  a frame of another site comes and goes in the run: a renderer Chromium starts in the run ends
//           in it before any role was read for it: cpu.renderer refused "processes that ended inside the
//           run (role unread) used … ms, …% of cpu.renderer (over the 1% limit)" (Windows); where the frame
//           takes Chromium's spare renderer, alive at the run's start, it reads as swap's (Linux headless shell)
import { bench, url } from "@hona/strobe"

const port = Number(process.env.ENDED_PORT ?? 4824)
let server: ReturnType<typeof Bun.serve> | null = null

// The other site is localhost (the page is on 127.0.0.1). Chromium keeps a renderer it no longer needs
// about 2 s for reuse before it ends: each run waits 4 s.
const main = `<!doctype html><title>ended</title><h1>ended</h1><script>
window.frameIn = (spin) => new Promise((done) => {
  const f = document.createElement("iframe")
  f.id = "x"
  f.onload = () => done(true)
  f.src = "http://localhost:${port}/frame?spin=" + spin
  document.body.append(f)
})
window.frameOut = () => { document.getElementById("x")?.remove(); return true }
</script>`
const frame = (spin: number) => `<!doctype html><script>const t = performance.now(); while (performance.now() - t < ${spin}) {}</script>`

const target = url(`http://127.0.0.1:${port}/`, {
  prepare() {
    server ??= Bun.serve({
      port,
      fetch(req) {
        const u = new URL(req.url)
        const body = u.pathname === "/frame" ? frame(Number(u.searchParams.get("spin") ?? 0)) : main
        return new Response(body, { headers: { "content-type": "text/html", "cache-control": "no-store" } })
      },
    })
  },
  teardown() {
    server?.stop(true)
    server = null
  },
})

const metrics = ["cpu.work", "cpu.renderer", "cpu.app.total"]

export default bench({
  targets: { ended: target },
  cases: {
    swap: {
      primary: "cpu.work",
      metrics,
      async run({ page }) {
        await page.goto(`http://localhost:${port}/`)
        await page.wait(4000)
      },
      reset: ({ page }) => page.goto(`http://127.0.0.1:${port}/`),
    },
    unread: {
      primary: "cpu.work",
      metrics,
      async run({ page }) {
        await page.eval(() => (window as any).frameIn(100))
        await page.eval(() => (window as any).frameOut())
        await page.wait(4000)
      },
    },
  },
})
