// Dev bench for keys sent to a page with no focus (not shipped). A page that writes each key it gets, and a
// button that opens a popup window, which takes the focus from it on a desktop. Run from the repo root:
//   strobe bench scripts/dev/focus.bench.ts --head . --check            (headed)
//   strobe bench scripts/dev/focus.bench.ts --head . --check --headless
// Expected: `focused` and `selector` pass (the press clicks its field first). `ignored` fails at its until (it waits
// for a key it never sends) with no word on focus: its key reached the page, even headless, where the page reads no
// focus (document.hasFocus() is false) until a click. `unfocused` passes where the page takes the keys anyway, and
// otherwise fails saying the page had no focus, so keys did not reach it.
import { bench, url } from "@hona/strobe"

const page = `<!doctype html><title>focus</title>
<style>body{font:14px system-ui;margin:8px}</style>
<button id="pop">pop</button> <input id="field"> <p id="got">keys:</p>
<script>
addEventListener("keydown", (e) => { got.textContent += " " + e.key })
pop.addEventListener("click", () => { window.open("about:blank", "_blank", "popup,width=200,height=200") })
</script>`

const keyed = (key: string) => ({ until: { selector: "#got", text: `keys: ${key}` }, timeout: 2000 })
// The popup opens on the click and takes the focus from the page's document.
const unfocus = async ({ page }: any) => {
  await page.click("#pop")
  await page.wait(500)
}

export default bench({
  targets: { page: url(`data:text/html,${encodeURIComponent(page)}`) },
  // What a run typed goes, so the next starts as it did.
  async reset({ page }) {
    await page.eval(() => {
      ;(document.getElementById("field") as HTMLInputElement).value = ""
      document.getElementById("got")!.textContent = "keys:"
    })
  },
  cases: {
    focused: { primary: "run.duration", run: async ({ page }) => void (await page.press("a", keyed("a"))) },
    ignored: { primary: "run.duration", run: async ({ page }) => void (await page.press("a", keyed("b"))) },
    unfocused: {
      primary: "run.duration",
      async run(ctx) {
        await unfocus(ctx)
        await ctx.page.press("a", keyed("a"))
      },
    },
    selector: {
      primary: "run.duration",
      async run(ctx) {
        await unfocus(ctx)
        await ctx.page.press("a", { ...keyed("a"), selector: "#field" })
      },
    },
  },
})
