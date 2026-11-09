// Dev bench for servers a target's prepare starts (not shipped): the page on one, an API it fetches on another,
// both on this computer's loopback on ports prepare picks, the API's address in what prepare returns. On a phone
// (Chrome on Android, or an emulator) strobe reverses each loopback address in `ctx.prepared` to the phone's, so the
// page reaches its API there too. From the repo root:
//   strobe bench scripts/dev/prepared-servers.bench.ts --head . --check --headless   (an emulated phone)
//   strobe bench scripts/dev/prepared-servers.bench.ts --head . --check --mobile
//   strobe bench scripts/dev/prepared-servers.bench.ts --head . --probes memory --repeat 3 --mobile
// Each run fetches the API and fails unless it answers. Leak run truth: `keeps` holds about 100 KB more after each run
// (25,000 references to one string), so its js heap grows; `clean` keeps nothing, so its heap does not.
import { bench, web } from "@hona/strobe"

const page = `<!doctype html><title>prepared servers</title><meta name="viewport" content="width=device-width">
<p id="api">api: waiting</p>
<script>
window.kept = []
const api = new URLSearchParams(location.search).get("api")
fetch(api + "ping").then((r) => r.text()).then((t) => { document.getElementById("api").textContent = "api: " + t }, (e) => { document.getElementById("api").textContent = "api: failed (" + e.message + ")" })
</script>`

let servers: { stop(force?: boolean): unknown }[] = []
// One fetch of the API from the page: its answer, which must be `ok`.
const ping = (keep: boolean) => async ({ page }: any) => {
  const got = await page.eval(async (keep: boolean) => {
    const api = new URLSearchParams(location.search).get("api")
    const text = await (await fetch(api + "ping")).text()
    if (keep) (window as any).kept.push(new Array(25000).fill(text))
    return text
  }, keep)
  if (got !== "ok") throw new Error(`the API answered ${JSON.stringify(got)}, not "ok"`)
}

export default bench({
  targets: {
    phone: web({
      emulate: "mobile",
      prepare() {
        const json = { "access-control-allow-origin": "*", "cache-control": "no-store" }
        const api = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response("ok", { headers: json }) })
        const site = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(page, { headers: { "content-type": "text/html" } }) })
        servers = [api, site]
        return { url: `http://127.0.0.1:${site.port}/`, api: `http://127.0.0.1:${api.port}/` }
      },
      url: (ctx) => `${ctx.prepared.url}?api=${encodeURIComponent(ctx.prepared.api)}`,
      teardown() {
        for (const s of servers.splice(0)) s.stop(true)
      },
    }),
  },
  setup: ({ page }) => page.waitFor({ selector: "#api", text: "api: ok" }, { timeout: 10000 }),
  cases: {
    keeps: { primary: "run.duration", run: ping(true) },
    clean: { primary: "run.duration", run: ping(false) },
  },
})
