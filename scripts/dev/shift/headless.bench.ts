import { bench, url, web } from "@hona/strobe"

const address = new URL("./index.html?p=a", import.meta.url).href
export default bench({
  targets: { fixed: url(address), built: web() },
  cases: {
    draw: { primary: "frames.interval.p50", resetCheck: false, async run({ page, count }) {
      await page.eval(() => { const until = performance.now() + 80; while (performance.now() < until) {} })
      count("items", 1)
    } },
    ready: { primary: "frames.interval.p50", launch: { until: "main" } },
  },
})
