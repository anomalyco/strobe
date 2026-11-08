// Known-answer bench for how strobe derives a page's metrics from its trace (dev only, not shipped), headless so
// it runs on a locked session. From the repo root:
//   strobe bench scripts/dev/derive/derive.bench.ts --head . --check
// Truth per run (runner-side waits, so each is the wait plus a mark's round trip, about 1 ms):
//   phases        phase.twice = 100 + 50 ms (entered twice in a row); phase.nested = 80 ms (entered inside itself)
//   other site    frames on both sites: 300 ms of animation on each side of a goto to another site's renderer;
//                 the new renderer's start may end a process inside the run (Linux: the zygote's helper,
//                 about 22 ms), whose CPU Linux's launch cgroups keep: cpu.work and cpu.app.total stand
//                 (EndedCpu says how much), and cpu.renderer is refused when it did (what ended is no role's)
//   two hops      the page goes to a second site and on by itself to a third: frames in all three renderers
//                 (`strobe query <recording> frames`, per process by `query <recording> sql`)
//   link          a navigation by a click on a link (no goto): main.work refused, never negative or short
//   untimed goto  count.hits = 5 + 7 = 12 (100 before the goto and 1000 after it are inside ctx.untimed)
//   redirect      net.requests = 2 (/hop and /payload), net.bytes{/hop} = the 302 alone (a few hundred bytes),
//                 net.bytes{/payload} = about 10 000
//   gaps          frames.interval.n = 5 gaps (10 isolated updates and 5 pairs, 350 ms apart: 20 frames; far
//                 enough apart for a headless page's 10 frames a second too)
// Headless, frame metrics are refused (`Headless`): `strobe query <recording> frames` reads `other site` and
// `gaps` from their recordings. Headed (an unlocked session), `--blocks 1 --filter gaps` with
// `primary: "frames.interval.p50"` refuses it TailTooShort (5 gaps < 20, where 20 frames passed it).
// `--batch 6`: one recording for every case; its meta.json lists each case's measured runs, and
// `strobe query <recording> threads` reads only them.
// Processes that end in a run: script.bench.ts here.
import { bench, fixture, url } from "@hona/strobe"

const other = (ctx: any) => `${ctx.fixture.url.replace("127.0.0.1", "localhost")}/b.html`
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

export default bench({
  fixture: fixture("bun server.ts {port}"),
  targets: { page: url("{fixture.url}/a.html", { headless: true }) },
  cases: {
    phases: {
      targets: ["page"],
      primary: "phase.twice",
      metrics: ["phase.nested"],
      run: async (ctx: any) => {
        await ctx.phase("twice", () => sleep(100))
        await ctx.phase("twice", () => sleep(50))
        await ctx.phase("nested", () => ctx.phase("nested", () => sleep(80)))
      },
    },
    // Headless: frames are refused as metrics; `strobe query <recording> frames` reads them from the trace.
    "other site": {
      targets: ["page"],
      primary: "run.duration",
      metrics: ["main.busy", "cpu.app.total", "cpu.work"],
      run: async (ctx: any) => {
        await ctx.page.eval(() => ((document.getElementById("box") as HTMLElement).dataset.run = "1"))
        await ctx.page.wait(300)
        await ctx.page.goto(other(ctx))
        await ctx.page.wait(300)
      },
      reset: async (ctx: any) => ctx.page.goto(`${ctx.fixture.url}/a.html`),
    },
    // The page goes to a second site and from there by itself to a third: strobe marks nothing in the
    // middle renderer but the document's own start (`metrics::doc_mark_script`).
    "two hops": {
      targets: ["page"],
      primary: "run.duration",
      run: async (ctx: any) => {
        const third = `${ctx.fixture.url.replace("127.0.0.1", "127.0.0.2")}/c.html`
        const second = `${ctx.fixture.url.replace("127.0.0.1", "localhost")}/b.html?next=${encodeURIComponent(third)}`
        await ctx.page.eval((u: string) => (setTimeout(() => (location.href = u), 0), true), second)
        // Past both navigations before asking the page anything (a read across one fails).
        await ctx.page.wait(1500)
        await ctx.page.waitFor("#c", { timeout: 10_000 })
        await ctx.page.wait(300)
      },
      reset: async (ctx: any) => ctx.page.goto(`${ctx.fixture.url}/a.html`),
    },
    link: {
      targets: ["page"],
      primary: "run.duration",
      metrics: ["main.work", "main.script"],
      run: async (ctx: any) => {
        await ctx.page.click("#go", { until: "#b" })
        await ctx.page.wait(200)
      },
      reset: async (ctx: any) => ctx.page.goto(`${ctx.fixture.url}/a.html`),
    },
    "untimed goto": {
      targets: ["page"],
      primary: "count.hits",
      run: async (ctx: any) => {
        await ctx.page.eval(() => (globalThis as any).__strobeCount("hits", 5))
        await ctx.untimed(async () => {
          await ctx.page.eval(() => (globalThis as any).__strobeCount("hits", 100))
          await ctx.page.goto(other(ctx))
          await ctx.page.eval(() => (globalThis as any).__strobeCount("hits", 1000))
        })
        await ctx.page.eval(() => (globalThis as any).__strobeCount("hits", 7))
      },
      reset: async (ctx: any) => ctx.page.goto(`${ctx.fixture.url}/a.html`),
    },
    redirect: {
      targets: ["page"],
      primary: "net.requests",
      metrics: ["net.bytes", "net.bytes{/hop}", "net.bytes{/payload}"],
      run: async (ctx: any) => {
        await ctx.page.eval(() => fetch("/hop").then((r) => r.text()).then((t) => t.length))
      },
    },
    gaps: {
      targets: ["page"],
      primary: "run.duration",
      run: async (ctx: any) => {
        await ctx.page.eval(() => (globalThis as any).isolated(10, 350))
        await ctx.page.eval(() => (globalThis as any).pairs(5, 350))
      },
    },
  },
})
