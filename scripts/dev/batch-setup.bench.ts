// Dev bench for a batch whose setup differs per case (not shipped). Six cases on one headless page: setup opens
// the case's screen (heading "Review" or "Session") and every case expects its own, so a case set up as another
// fails `WrongScreen`. `MODE` picks the file's shape:
//   fallback  no setupKey, setup reads ctx.case: each case runs a launch of its own (Batch note, unbatched_cases)
//   seed      the same with ctx.random() in setup instead of ctx.case
//   keyed     setupKey gives the screen and setup reads ctx.setupKey: two launches of three cases
//   reads     setupKey, and setup still reads ctx.case: exit 2 at the first launch
//   bad       setupKey returns a BigInt for one case: exit 2 before anything builds
// Run from the repo root, with and without --batch, and compare which cases pass:
//   MODE=keyed strobe bench scripts/dev/batch-setup.bench.ts --head . --check --headless [--batch 10]
import { bench, url } from "@hona/strobe"

const page = new URL("pages/batch.html", import.meta.url).href
const mode = process.env.MODE ?? "keyed"
const screen = (name: string) => (name.startsWith("review") ? "Review" : "Session")
const open = (title: string) => (document.getElementById("title")!.textContent = title)

type Case = Parameters<typeof bench>[0]["cases"][string]
const cases: Record<string, Case> = {}
for (const name of ["review-a", "review-b", "review-c", "session-a", "session-b", "session-c"])
  cases[name] = {
    primary: "cpu.work",
    expect: { heading: screen(name) },
    run: ({ page }) => page.click("#check"),
    reset: ({ page }) => page.click("#check"),
  }

export default bench({
  targets: { page: url(page) },
  ...(mode === "fallback" || mode === "seed"
    ? {}
    : { setupKey: (ctx: { case: string }) => (mode === "bad" && ctx.case === "session-c" ? (1n as unknown as string) : screen(ctx.case)) }),
  async setup(ctx) {
    await ctx.page.waitFor("#text")
    const title = mode === "keyed" ? (ctx.setupKey as string) : mode === "seed" ? (ctx.random() < 2 ? screen(ctx.case) : "") : screen(ctx.case)
    await ctx.page.eval(open, title)
  },
  cases,
})
