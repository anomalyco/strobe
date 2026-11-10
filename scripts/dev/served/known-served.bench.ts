// known-web.bench.ts's page served for each variant (this folder's strobe.json: each checkout's own
// known-server.ts), so a check can run base and head and hold their exact counters to each other: the
// action's dogfood run (.github/workflows/check.yml, SPEC §26.6). Not shipped. From this folder:
//   strobe bench known-served.bench.ts --head . --check --headless --base origin/main \
//     --noworse count.ctx=0,count.page=0,net.requests=0,net.bytes=0,dom.nodes=0
// The truth per run, on both sides: phases count.ctx 7, count.page 5; net net.requests 3, the three bodies'
// 111,000 bytes and their headers in net.bytes; dom.nodes the page's own, the same before and after a run.
import { bench, web } from "@hona/strobe"

export default bench({
  targets: { known: web() },
  cases: {
    phases: {
      primary: "phase.b",
      metrics: ["phase.a", "count.ctx", "count.page", "dom.nodes"],
      async run({ page, phase, count }) {
        await phase("a", () => page.eval(() => (window as any).known.spin(10)))
        await phase("b", () => page.eval(() => (window as any).known.spin(40)))
        await phase("a", () => page.eval(() => (window as any).known.spin(10)))
        count("ctx", 3)
        count("ctx", 4)
        await page.eval(() => (globalThis as any).__strobeCount("page", 5))
      },
    },
    net: {
      primary: "net.requests",
      metrics: ["net.bytes", "net.requests{/blob/*}", "dom.nodes"],
      run: ({ page }) => page.eval((sizes: number[]) => (window as any).known.fetchAll(sizes), [1000, 10_000, 100_000]),
    },
    heap: {
      primary: "cpu.work",
      metrics: ["js.heap.used", "memory.app.private", "dom.nodes"],
      run: ({ page }) => page.eval(() => (window as any).known.keepBufferOnce(50 * 1024 * 1024)),
    },
  },
})
