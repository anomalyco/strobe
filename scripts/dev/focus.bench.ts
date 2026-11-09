// Dev bench for keys sent to a page with no focus (not shipped). A page that writes each key it gets, and a
// button that opens a popup window, which takes the focus from it on a desktop. Run from the repo root:
//   strobe bench scripts/dev/focus.bench.ts --head . --check            (headed)
//   strobe bench scripts/dev/focus.bench.ts --head . --check --headless [--engine firefox]
// Expected: `focused`, `unfocused` and `selector` pass (the press clicks its field first; Chromium's DevTools keys
// focus the page they go to, so they reach it after the popup too). `ignored` fails at its until (it waits for a key
// it never sends) with no word on focus: its key reached the page, even where the page reads no focus
// (document.hasFocus() is false: headless Firefox throughout, headless Chromium until a key or click). `unseen` is
// `ignored` on a page that wipes strobe's record of its keys: in headless Firefox it fails saying the page had no
// focus, so keys did not reach it; in Chromium it says nothing (the key gave the page focus).
import { bench, url } from "@hona/strobe"

const page = `<!doctype html><title>focus</title>
<style>body{font:14px system-ui;margin:8px}</style>
<button id="pop">pop</button> <input id="field"> <p id="got">keys:</p>
<script>
addEventListener("keydown", (e) => { got.textContent += " " + e.key })
// #unseen: the page wipes what strobe saw of its keys, as if none had come (the report's own check).
if (location.hash === "#unseen") addEventListener("keyup", () => { window.__strobeKeys.at = 0 })
pop.addEventListener("click", () => { window.open("about:blank", "_blank", "popup,width=200,height=200") })
</script>`

const keyed = (key: string) => ({ until: { selector: "#got", text: `keys: ${key}` }, timeout: 2000 })
// The popup opens on the click and takes the focus from the page's document.
const unfocus = async ({ page }: any) => {
  await page.click("#pop")
  await page.wait(500)
}

export default bench({
  targets: { page: url(`data:text/html,${encodeURIComponent(page)}`), unseen: url(`data:text/html,${encodeURIComponent(page)}#unseen`) },
  // What a run typed goes, so the next starts as it did.
  async reset({ page }) {
    await page.eval(() => {
      ;(document.getElementById("field") as HTMLInputElement).value = ""
      document.getElementById("got")!.textContent = "keys:"
    })
  },
  cases: {
    focused: { targets: ["page"], primary: "run.duration", run: async ({ page }) => void (await page.press("a", keyed("a"))) },
    ignored: { targets: ["page"], primary: "run.duration", run: async ({ page }) => void (await page.press("a", keyed("b"))) },
    // The page hides that its key came: headless (no focus until a click), the failure says no key reached it.
    unseen: { targets: ["unseen"], primary: "run.duration", run: async ({ page }) => void (await page.press("a", keyed("b"))) },
    unfocused: {
      targets: ["page"],
      primary: "run.duration",
      async run(ctx) {
        await unfocus(ctx)
        await ctx.page.press("a", keyed("a"))
      },
    },
    selector: {
      targets: ["page"],
      primary: "run.duration",
      async run(ctx) {
        await unfocus(ctx)
        await ctx.page.press("a", { ...keyed("a"), selector: "#field" })
      },
    },
  },
})
