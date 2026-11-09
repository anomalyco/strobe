// Dev bench for image anchors and pixel waits (SPEC 9.6; not shipped): a window drawn with no accessibility tree
// (canvas-app.cs on Windows, built here with the .NET Framework's compiler; canvas-app.swift on macOS, built with
// swiftc; canvas-app.py, GTK, on Linux) and a page drawn on one canvas. The anchors are the fixtures' own tiles, written to a temporary folder. Run from the repo root:
//   strobe bench scripts/dev/canvas.bench.ts --head . --check
// Expected refusals: `ambiguous` AnchorAmbiguous (two ITEM tiles), `missing` AnchorNotFound, `dirty` ResetIncomplete
// (resetPixels), and on a locked session `click` SessionLocked (keys still reach the window there).
// Windows: the app presents each frame through a DXGI flip-model swap chain, so `latency`, `press edge` and
// `release edge` time pixel waits from real present events; the `gdi` target (canvas-app.exe --gdi) paints with GDI,
// which makes no present event: its interaction.* is refused PresentsUnmatched.
import { bench, native, url } from "@hona/strobe"
import { execFileSync } from "node:child_process"
import { existsSync, mkdirSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"

const dir = path.join(tmpdir(), "strobe-canvas")
mkdirSync(dir, { recursive: true })
const windows = process.platform === "win32"
const mac = process.platform === "darwin"
const exe = windows ? path.join(dir, "canvas-app.exe") : mac ? path.join(dir, "canvas-app") : path.join(import.meta.dir, "canvas-app.py")
const source = path.join(import.meta.dir, windows ? "canvas-app.cs" : "canvas-app.swift")
if ((windows || mac) && (!existsSync(exe) || statSync(exe).mtimeMs < statSync(source).mtimeMs)) {
  if (windows) execFileSync("C:/Windows/Microsoft.NET/Framework64/v4.0.30319/csc.exe", ["/nologo", "/target:winexe", `/out:${exe}`, source])
  else execFileSync("swiftc", ["-O", source, "-o", exe])
}
if (!existsSync(path.join(dir, "play.png"))) execFileSync(windows || mac ? exe : "python3", windows || mac ? ["--anchors", dir] : [exe, "--anchors", dir])

// A tile of the window's, and one of the page's (written by its setup at the page's devicePixelRatio, `pageScale`).
const tile = (name: string) => path.join(dir, `${name}.png`)
const pageTile = (name: string) => path.join(dir, `page-${name}.png`)
let pageScale = 1
// The locator of a target's tile: the page's says the scale it was written at.
const image = (target: string, name: string) => (target === "page" ? { image: pageTile(name), scale: pageScale } : { image: tile(name) })
// Where the count's bars are drawn.
const bars = [40, 270, 500, 40] as [number, number, number, number]
const counted = { pixels: { region: bars, changed: true } }
const still = { pixels: { region: bars, stable: 100 } }
let play: [number, number] = [0, 0]
// The Windows app's PRESS tile lights on a button's press, RELEASE on its release; R darkens both.
const pressTile = [40, 330, 160, 60] as [number, number, number, number]
const releaseTile = [240, 330, 160, 60] as [number, number, number, number]
const lit = (region: [number, number, number, number]) => ({ until: { pixels: { region, changed: true } } })
const dark = (region: [number, number, number, number]) => ({ until: { pixels: { region, stable: 100 } } })
// Cases of the anchors and waits themselves: not the GDI window's, which only shows what its presents lack.
const drawn = ["app", "page"]
// A click on a tile, timed from the one frame that showed it lit, and the tile dark again after.
const edge = (at: [number, number], tile: [number, number, number, number]) => ({
  targets: ["app", "gdi"],
  primary: "interaction.p50",
  metrics: ["driver.pixel_reads", "cpu.work"],
  run: ({ page }: any) => page.click(at, lit(tile)),
  reset: ({ page }: any) => page.press("r", dark(tile)),
})

export default bench({
  targets: {
    app: native(exe, { resetPixels: { region: bars } }),
    ...(windows ? { gdi: native(exe, { args: ["--gdi"], resetPixels: { region: bars } }) } : {}),
    page: url(new URL("pages/canvas.html", import.meta.url).href, { viewport: { width: 600, height: 360 }, resetPixels: { region: bars } }),
  },
  async setup({ page, target }) {
    if (target === "page") {
      // At the page's own scale, as its canvas draws them (a 1x tile scaled up matches no sharp one).
      pageScale = await page.eval(() => devicePixelRatio)
      for (const name of ["play", "item", "square", "missing"]) {
        const url = await page.eval((n: string, s: number) => (window as any).anchor(n, s), name, pageScale)
        writeFileSync(pageTile(name), Buffer.from(url.split(",")[1], "base64"))
      }
    }
    // Found once, outside the runs: the runs click the point (no pixel read to aim).
    const [m] = await page.query(image(target, "play"))
    if (!m?.point) throw new Error(`PLAY not found: ${JSON.stringify(m)}`)
    play = m.point
  },
  cases: {
    // Every distinct match of a tile drawn twice, in one capture.
    query: {
      targets: drawn,
      primary: "cpu.work",
      async run({ page, target }) {
        const found = await page.query(image(target, "item"))
        if (found.length !== 2) throw new Error(`ITEM: ${found.length} matches, not 2: ${JSON.stringify(found)}`)
      },
    },
    // Keys reach the window on a locked session too; the count's bars prove each one's reaction.
    keys: {
      targets: drawn,
      primary: "cpu.work",
      metrics: ["driver.pixel_reads", "driver.pixel_ms"],
      run: ({ page }) => page.press("Space", { until: counted }),
      reset: ({ page }) => page.press("r", { until: still }),
    },
    // Each key's latency from the one frame between the last capture without its bar and the first with it
    // (needs the OS's present events and a presenting window; the GDI window has none: PresentsUnmatched).
    latency: {
      targets: windows ? ["app", "gdi", "page"] : drawn,
      primary: "interaction.p50",
      metrics: ["driver.pixel_reads", "cpu.work"],
      run: ({ page }) => page.press("Space", { until: counted }),
      reset: ({ page }) => page.press("r", { until: still }),
    },
    // A click drawn on its press is timed from the press (`1 at a press`), one drawn on its release from the
    // release: PRESS lights on the button's press, RELEASE on its release.
    ...(windows ? { "press edge": edge([120, 360], pressTile), "release edge": edge([320, 360], releaseTile) } : {}),
    // An image that shows after the key.
    image: {
      targets: drawn,
      primary: "cpu.work",
      run: ({ page, target }) => page.press("x", { until: image(target, "square") }),
      reset: ({ page }) => page.press("x", { until: still }),
    },
    // A click at the point setup found; on a locked session refused SessionLocked.
    click: { targets: drawn, primary: "cpu.work", run: ({ page }) => page.click(play, { until: counted }), reset: ({ page }) => page.press("r", { until: still }) },
    // A click aimed at an image (two reads: the aim and the until).
    "click image": {
      targets: drawn,
      primary: "cpu.work",
      run: ({ page, target }) => page.click(image(target, "play"), { until: counted }),
      reset: ({ page }) => page.press("r", { until: still }),
    },
    // PLAY exported at @2x and @1.5x, found at the window's own scale.
    scaled: {
      targets: ["app"],
      primary: "cpu.work",
      async run({ page }) {
        await page.waitFor({ image: tile("play@2x"), scale: 2, threshold: 0.9 })
        await page.waitFor({ image: tile("play@1.5x"), scale: 1.5, threshold: 0.9 })
      },
    },
    ambiguous: { targets: drawn, primary: "cpu.work", run: ({ page, target }) => page.click(image(target, "item")) },
    missing: { targets: drawn, primary: "cpu.work", run: ({ page, target }) => page.waitFor(image(target, "missing"), { timeout: 2000 }) },
    // A reset that takes nothing back: the next run starts with a bar more (resetPixels: ResetIncomplete).
    dirty: { targets: drawn, primary: "cpu.work", run: ({ page }) => page.press("Space", { until: counted }), reset: async () => {} },
    // `--pixels-only`: the window's tiles.
    snapshot: { targets: drawn, primary: "cpu.work", run: ({ page }) => page.snapshot("tiles") },
  },
})
