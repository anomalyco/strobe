import { bench, url, web } from "@hona/strobe"

const address = new URL("../pages/planted.html?p=a", import.meta.url).href
export default bench({
  targets: { fixed: url(address), built: web({ url: address }) },
  cases: {
    draw: { primary: "frames.interval.p50", resetCheck: false, async run({ page, count }) {
      await page.eval(() => (window as any).known.spin(80))
      count("items", 1)
    } },
    ready: { primary: "frames.interval.p50", launch: { until: "body" } },
  },
})
