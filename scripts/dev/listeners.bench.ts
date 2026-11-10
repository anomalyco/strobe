// Known-answer bench for a memory census's DOM listeners (dev only, not shipped): `none` loads a page that adds no
// event listener, `three` the same page adding three to its window. strobe's own input counters (touch on a phone,
// keys, OS input) add theirs to the page but count them in `window.__strobeListeners`, which the census takes off.
// From the repo root:
//   strobe bench scripts/dev/listeners.bench.ts --head . --probes memory --repeat 3 --headless --target desktop
//   strobe bench scripts/dev/listeners.bench.ts --head . --probes memory --repeat 3 --headless --target phone
//   strobe bench scripts/dev/listeners.bench.ts --head . --probes memory --repeat 3 --target phone --mobile=<serial>
// then `strobe query <rec> memory` of each case's leak recording. Truth: `none` 0 listeners, `three` 3, on every
// target; on the phone the processes come from Chromium's memory dump, named (browser, renderer, GPU, services).
import { bench, web } from "@hona/strobe"

const page = (n: number) => `<!doctype html><title>listeners ${n}</title><meta name="viewport" content="width=device-width">
<p id="ready">listeners ${n}</p>
<script>
for (let i = 0; i < ${n}; i++) addEventListener("strobe-test-" + i, () => {})
</script>`

let server: { stop(force?: boolean): unknown } | null = null
const prepare = () => {
  server ??= Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (r) => new Response(page(Number(new URL(r.url).searchParams.get("n") ?? 0)), { headers: { "content-type": "text/html", "cache-control": "no-store" } }) })
  return { url: `http://127.0.0.1:${(server as any).port}/` }
}
const teardown = () => {
  server?.stop(true)
  server = null
}
const url = (ctx: any) => `${ctx.prepared.url}?n=${ctx.case === "three" ? 3 : 0}`

export default bench({
  targets: {
    desktop: web({ prepare, url, teardown }),
    phone: web({ emulate: "mobile", prepare, url, teardown }),
  },
  setup: ({ page }) => page.waitFor({ selector: "#ready" }, { timeout: 10000 }),
  cases: {
    none: { primary: "run.duration", run: ({ page }) => page.wait(50) },
    three: { primary: "run.duration", run: ({ page }) => page.wait(50) },
  },
})
