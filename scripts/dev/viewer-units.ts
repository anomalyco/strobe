// The HTML viewers' time units, checked in a headless browser: every measured time on a report or
// history page in µs (the default), ms or s, switched by clicking the header's switch.
//
//   bun scripts/dev/viewer-units.ts [<page.html>...] [--out <dir>] [--browser <path>] [--width 1280]
//     [--rust <strobe>] [--viewers <dir>]
//
// Each page is any `strobe report … --format html` or `strobe report history …` file: its data is
// put into this tree's viewer/report.html or viewer/history.html (`--viewers <dir>`: that folder's
// viewer/ instead), so a page written by an installed strobe checks the viewer being edited. Built-in
// synthetic pages (SYNTHETIC below) are checked too, each with what it must read. Checks, per page
// and unit:
//   - the formatters against the table below (and the shared block alike in both viewers);
//   - a page opened at `#unit=X` and one switched to X by a click read the same, the address keeps
//     its other keys, and one with `unit=` taken out reads in the page's own unit again;
//   - every time reads in X with grouped digits at its magnitude's decimals; the same time in the
//     three units agrees within rounding; a time that does not change with the unit is wall-clock or
//     stored text (listed, and failed when it is not one of the known places below);
//   - no time is clipped: a number wider than its cell or box or broken across lines, an axis label
//     outside its chart or on its neighbour; no axis prints two ticks alike or ticks unevenly apart;
//     a section scrolled to (`#metric=`, `#scope=`) starts below the sticky header;
//   - the same at 100× every time (eight digits and more).
// Prints a pass/fail table; `--out` keeps it (results.txt, results.json) with the pages, else the
// pages go in a temporary folder that is removed after.
import { mkdirSync, readFileSync, writeFileSync, existsSync, mkdtempSync, rmSync } from "node:fs"
import { join, basename, resolve } from "node:path"
import { tmpdir } from "node:os"

const args = process.argv.slice(2)
const opt = (k: string) => {
  const i = args.indexOf(k)
  if (i < 0) return undefined
  const v = args[i + 1]
  args.splice(i, 2)
  return v
}
const outArg = opt("--out")
const outDir = resolve(outArg || mkdtempSync(join(tmpdir(), "viewer-units-")))
const viewersDir = resolve(opt("--viewers") || join(import.meta.dir, "..", ".."))
const browserArg = opt("--browser")
const width = Number(opt("--width") || 1280)
// `--rust <strobe>`: a strobe whose `stats` prints times (op value, signed, times, verdict): every
// vector and a sweep of values across magnitudes is checked against it, character for character.
const rust = opt("--rust")
// `page.html#scope=host` checks the page in that view too (its keys go before `unit=`).
const pages: { file: string; view: string; expect?: Expect[] }[] = args.map((p) => ({ file: resolve(p.split("#")[0]), view: p.includes("#") ? p.slice(p.indexOf("#") + 1) : "" }))
mkdirSync(outDir, { recursive: true })

const VIEWERS = { report: readFileSync(join(viewersDir, "viewer/report.html"), "utf8"), history: readFileSync(join(viewersDir, "viewer/history.html"), "utf8") }
const UNITS = ["us", "ms", "s"] as const
type Unit = (typeof UNITS)[number]
const LABEL: Record<Unit, string> = { us: "µs", ms: "ms", s: "s" }
const PER_MS: Record<Unit, number> = { us: 1000, ms: 1, s: 0.001 }
// The most decimals a time shows in each unit (fmt::TimeUnit::finest).
const FINEST: Record<Unit, number> = { us: 2, ms: 4, s: 6 }

// The expected text of each formatter for a value in ms, as the Rust seam prints it (fmt::time,
// time_signed, bound, times; the table us-A left at cleanup/usa, then more): `val` fmtVal(v, "ms"),
// `signed` fmtSigned, `bound` fmtBound of an absolute bound, `times` values printed together.
const VECTORS: { fn: string; v: number | number[]; unit?: string; want: Record<Unit, string> }[] = [
  { fn: "val", v: 0.004, want: { us: "4.00 µs", ms: "0.0040 ms", s: "0.000004 s" } },
  { fn: "val", v: 0.00999, want: { us: "9.99 µs", ms: "0.0100 ms", s: "0.000010 s" } },
  { fn: "val", v: 0.1, want: { us: "100 µs", ms: "0.100 ms", s: "0.000100 s" } },
  { fn: "val", v: 0.5, want: { us: "500 µs", ms: "0.500 ms", s: "0.000500 s" } },
  { fn: "val", v: 0.99996, want: { us: "1,000 µs", ms: "1.00 ms", s: "0.00100 s" } },
  { fn: "val", v: 9.996, want: { us: "9,996 µs", ms: "10.0 ms", s: "0.0100 s" } },
  { fn: "val", v: 42, want: { us: "42,000 µs", ms: "42.0 ms", s: "0.0420 s" } },
  { fn: "val", v: 99.96, want: { us: "99,960 µs", ms: "100 ms", s: "0.100 s" } },
  { fn: "val", v: 1000, want: { us: "1,000,000 µs", ms: "1,000 ms", s: "1.00 s" } },
  { fn: "val", v: 1234.5678, want: { us: "1,234,568 µs", ms: "1,235 ms", s: "1.23 s" } },
  { fn: "val", v: -0.042, want: { us: "−42.0 µs", ms: "−0.0420 ms", s: "−0.000042 s" } },
  { fn: "signed", v: -0.042, want: { us: "−42.0 µs", ms: "−0.0420 ms", s: "−0.000042 s" } },
  { fn: "signed", v: 1.234, want: { us: "+1,234 µs", ms: "+1.23 ms", s: "+0.00123 s" } },
  // Printed together: each as it prints alone (us-A's note, revised 13:55).
  { fn: "times", v: [-0.0032, -0.0012, 0.0124], want: { us: "−3.20 µs … −1.20 µs … +12.4 µs", ms: "−0.0032 ms … −0.0012 ms … +0.0124 ms", s: "−0.000003 s … −0.000001 s … +0.000012 s" } },
  { fn: "times", v: [-3837.429, -4012.11, -3661.804], want: { us: "−3,837,429 µs … −4,012,110 µs … −3,661,804 µs", ms: "−3,837 ms … −4,012 ms … −3,662 ms", s: "−3.84 s … −4.01 s … −3.66 s" } },
  { fn: "times", v: [0.12, 0.05, 2.5], want: { us: "+120 µs … +50.0 µs … +2,500 µs", ms: "+0.120 ms … +0.0500 ms … +2.50 ms", s: "+0.000120 s … +0.000050 s … +0.00250 s" } },
  { fn: "times", v: [0, -1e-7, 2e-7], want: { us: "0 µs … −<0.01 µs … +<0.01 µs", ms: "0 ms … −<0.0001 ms … +<0.0001 ms", s: "0 s … −<0.000001 s … +<0.000001 s" } },
  { fn: "bound", v: 0.00004, want: { us: "±0.04 µs", ms: "±<0.0001 ms", s: "±<0.000001 s" } },
  { fn: "bound", v: 0.04, want: { us: "±40.0 µs", ms: "±0.0400 ms", s: "±0.000040 s" } },
  { fn: "bound", v: 0.52, want: { us: "±520 µs", ms: "±0.520 ms", s: "±0.000520 s" } },
  { fn: "bound", v: 14.76, want: { us: "±14,760 µs", ms: "±14.8 ms", s: "±0.0148 s" } },
  { fn: "verdict", v: 1.2, want: { us: "no worse (≤1,200 µs, faster)", ms: "no worse (≤1.20 ms, faster)", s: "no worse (≤0.00120 s, faster)" } },
  { fn: "verdict", v: 0.04, want: { us: "same (±40.0 µs)", ms: "same (±0.0400 ms)", s: "same (±0.000040 s)" } },
  { fn: "count", v: 1234567, want: { us: "1,234,567", ms: "1,234,567", s: "1,234,567" } },
  { fn: "count", v: -12345, want: { us: "-12,345", ms: "-12,345", s: "-12,345" } },
  { fn: "count", v: 12.5, want: { us: "12.5", ms: "12.5", s: "12.5" } },
  { fn: "val", v: 0.0004, want: { us: "0.40 µs", ms: "0.0004 ms", s: "<0.000001 s" } },
  { fn: "val", v: 0.042, want: { us: "42.0 µs", ms: "0.0420 ms", s: "0.000042 s" } },
  { fn: "val", v: 1.234, want: { us: "1,234 µs", ms: "1.23 ms", s: "0.00123 s" } },
  { fn: "val", v: 16.6667, want: { us: "16,667 µs", ms: "16.7 ms", s: "0.0167 s" } },
  { fn: "val", v: 738.147, want: { us: "738,147 µs", ms: "738 ms", s: "0.738 s" } },
  { fn: "val", v: 18309.038, want: { us: "18,309,038 µs", ms: "18,309 ms", s: "18.3 s" } },
  { fn: "val", v: 0, want: { us: "0 µs", ms: "0 ms", s: "0 s" } },
  { fn: "val", v: 0.000004, want: { us: "<0.01 µs", ms: "<0.0001 ms", s: "<0.000001 s" } },
  { fn: "val", v: 0.0125, want: { us: "12.5 µs", ms: "0.0125 ms", s: "0.000013 s" } },
  { fn: "val", v: 0.125, want: { us: "125 µs", ms: "0.125 ms", s: "0.000125 s" } },
  { fn: "val", v: 2.5, want: { us: "2,500 µs", ms: "2.50 ms", s: "0.00250 s" } },
  { fn: "val", v: 0.0005, want: { us: "0.50 µs", ms: "0.0005 ms", s: "<0.000001 s" } },
  { fn: "val", v: 250, want: { us: "250,000 µs", ms: "250 ms", s: "0.250 s" } },
  { fn: "val", v: 123456.7, want: { us: "123,456,700 µs", ms: "123,457 ms", s: "123 s" } },
  { fn: "val", v: 0.099996, want: { us: "100 µs", ms: "0.100 ms", s: "0.000100 s" } },
  { fn: "val", v: -5, want: { us: "−5,000 µs", ms: "−5.00 ms", s: "−0.00500 s" } },
  { fn: "signed", v: -3837.429, want: { us: "−3,837,429 µs", ms: "−3,837 ms", s: "−3.84 s" } },
  { fn: "signed", v: 0.000001, want: { us: "+<0.01 µs", ms: "+<0.0001 ms", s: "+<0.000001 s" } },
  { fn: "signed", v: -0.000001, want: { us: "−<0.01 µs", ms: "−<0.0001 ms", s: "−<0.000001 s" } },
  { fn: "signed", v: 0, want: { us: "0 µs", ms: "0 ms", s: "0 s" } },
  { fn: "signed", v: 1.5, want: { us: "+1,500 µs", ms: "+1.50 ms", s: "+0.00150 s" } },
  { fn: "bound", v: 0.5, want: { us: "±500 µs", ms: "±0.500 ms", s: "±0.000500 s" } },
  { fn: "bound", v: 0.0000001, want: { us: "±<0.01 µs", ms: "±<0.0001 ms", s: "±<0.000001 s" } },
  { fn: "times", v: [0.00003, 0.052], want: { us: "+0.03 µs … +52.0 µs", ms: "+<0.0001 ms … +0.0520 ms", s: "+<0.000001 s … +0.000052 s" } },
  { fn: "times", v: [-4012.11, -3661.804], want: { us: "−4,012,110 µs … −3,661,804 µs", ms: "−4,012 ms … −3,662 ms", s: "−4.01 s … −3.66 s" } },
  // Other units, the same in both pages and every time unit (fmt::value): never `-0.00`.
  { fn: "plain", unit: "score", v: -0.000012, want: { us: "0.00 score", ms: "0.00 score", s: "0.00 score" } },
  { fn: "plain", unit: "%", v: -0.00001, want: { us: "0.00%", ms: "0.00%", s: "0.00%" } },
  { fn: "plain", unit: "%", v: -0.001, want: { us: "-0.001%", ms: "-0.001%", s: "-0.001%" } },
  { fn: "plain", unit: "value", v: 101, want: { us: "101", ms: "101", s: "101" } },
  { fn: "plain", unit: "bytes", v: 3565158, want: { us: "3.40 MB", ms: "3.40 MB", s: "3.40 MB" } },
  { fn: "plain", unit: "bytes", v: 1434, want: { us: "1.4 KB", ms: "1.4 KB", s: "1.4 KB" } },
  { fn: "plain", unit: "MB", v: 0.287, want: { us: "0.29 MB", ms: "0.29 MB", s: "0.29 MB" } },
  { fn: "plain", unit: "instructions", v: 4.123e9, want: { us: "4.12 G instr", ms: "4.12 G instr", s: "4.12 G instr" } },
  { fn: "plain", unit: "count", v: 12.000000000001, want: { us: "12", ms: "12", s: "12" } },
]

// A time as a page shows it: sign, `<` floor, number, unit. `us` (ASCII) and `μs` (Greek mu) are
// never right in a page: they are caught to fail.
const TOKEN = /(^|[^\w.,])([−+-]?)(<?)(\d[\d,]*(?:\.\d+)?)\s?(µs|μs|us|ms|s)(?!\w)/g

// Times that do not change with the unit, and why that is right. Anything else that keeps a time in
// every unit fails (a wall-clock `s` too, outside these places).
const KEPT: [RegExp, string][] = [
  [/^Long tasks \(>50 ms\)$|tasks over 50 ms/, "a metric's name or definition: a long task is one over 50 ms"],
  [/[\w-]\d+(ms|µs)\b|\d(ms|µs)-/, "a name the bench file gave (`stall-750ms`): no space, never a formatted time"],
  [/every \d+ ms · timeout \d+ ms/, "the HTTP probe's configuration, as given"],
  [/timed out at \d+ ms/, "the HTTP probe's timeout, as configured"],
  [/at least 50 ms\)/, "the stall rule's floor (query::http STALL_MS), written as the terminal writes it"],
  [/^Note: /, "a note stored with the result (its warnings' words)"],
  [/^Series runtime: |^Duration: |One cold run, no warmup|\(~\d[\d.]* (ms|s|min|hr)\)/, "a wall-clock stretch (fmt::ms_duration: `850 ms` in every unit)"],
  [/^Measurement refused \(|^(leaks|no app leak|grows, then levels off|no growth)\b|^(found|no change):/, "a refusal's, a leak run's or a bisect's words as stored"],
]
const KEPT_CTX: [RegExp, string][] = [
  [/plan-v|plan-seg|eyebrow-meta/, "a wall-clock stretch (fmt::ms_duration: `850 ms` in every unit)"],
  [/cohort-at/, "where a probe cohort sits in the recording (wall clock)"],
  [/vid-/, "the video's clock"],
  [/cli-pre/, "the stored CLI text (drawn again by the Rust side)"],
  [/caveat-text|ledger|ins-title|insight-card/, "words stored with the result (warnings, the rerun ledger, insights)"],
]
// Chart labels that may stay in seconds in every unit: the video strip's clock, a census's times.
const KEPT_SVG = /vid-|@census$/

function findBrowser(): string {
  const env = process.env
  const c = [
    browserArg,
    `${env["ProgramFiles(x86)"]}/Microsoft/Edge/Application/msedge.exe`,
    `${env.ProgramFiles}/Microsoft/Edge/Application/msedge.exe`,
    `${env.ProgramFiles}/Google/Chrome/Application/chrome.exe`,
    `${env.LOCALAPPDATA}/Google/Chrome/Application/chrome.exe`,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean) as string[]
  const f = c.find((p) => existsSync(p))
  if (!f) throw new Error("no Chromium-based browser found: pass --browser <path>")
  return f
}

// The page's data in this tree's viewer.
function retemplate(file: string): { kind: "report" | "history"; path: string } {
  const html = readFileSync(file, "utf8")
  const m = /const (STROBE|DATA) = (.*);\r?\n/.exec(html)
  if (!m) throw new Error(`${file}: no page data`)
  const kind = m[1] === "STROBE" ? "report" : "history"
  const out = join(outDir, basename(file))
  writeFileSync(out, VIEWERS[kind].replace("/*STROBE_DATA*/null", () => m[2]))
  return { kind, path: out }
}

// --- A small CDP client over the browser's WebSocket ---
class Cdp {
  ws: WebSocket
  id = 0
  pending = new Map<number, (m: any) => void>()
  waiters: { method: string; fn: (p: any) => void }[] = []
  constructor(ws: WebSocket) {
    this.ws = ws
    ws.onmessage = (e) => {
      const m = JSON.parse(String(e.data))
      if (m.id && this.pending.has(m.id)) {
        this.pending.get(m.id)!(m)
        this.pending.delete(m.id)
      } else if (m.method) {
        this.waiters = this.waiters.filter((w) => (w.method === m.method ? (w.fn(m.params), false) : true))
      }
    }
  }
  send(method: string, params: any = {}, sessionId?: string, ms = 30000): Promise<any> {
    const id = ++this.id
    return new Promise((ok, fail) => {
      const t = setTimeout(() => fail(new Error(`${method} timed out`)), ms)
      this.pending.set(id, (m) => {
        clearTimeout(t)
        m.error ? fail(new Error(`${method}: ${m.error.message}`)) : ok(m.result)
      })
      this.ws.send(JSON.stringify({ id, method, params, sessionId }))
    })
  }
  next(method: string, ms = 30000): Promise<any> {
    return new Promise((ok, fail) => {
      const t = setTimeout(() => fail(new Error(`${method} not seen`)), ms)
      this.waiters.push({ method, fn: (p) => (clearTimeout(t), ok(p)) })
    })
  }
}

let browser: ReturnType<typeof Bun.spawn> | null = null
let browserCdp: Cdp | null = null
const profile = mkdtempSync(join(tmpdir(), "viewer-units-profile-"))
// Leaves nothing behind: the browser, its profile, and the pages unless `--out` keeps them. Killing
// the browser alone leaves its helper processes holding the profile's files: it is asked to close
// (all of them with it), and the files go once they are free.
async function closeBrowser() {
  try { await Promise.race([browserCdp?.send("Browser.close", {}), Bun.sleep(3000)]) } catch {}
  await Promise.race([browser?.exited, Bun.sleep(5000)])
  browser?.kill()
}
function cleanUp() {
  browser?.kill()
  try { rmSync(profile, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 }) } catch {}
  if (!outArg) try { rmSync(outDir, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 }) } catch {}
}
const watchdog = setTimeout(() => {
  console.error("viewer-units: no result in 20 min, stopping")
  cleanUp()
  process.exit(3)
}, 1200000)

async function open(): Promise<{ cdp: Cdp; sid: string }> {
  browser = Bun.spawn([findBrowser(), "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars", "--mute-audio", "--allow-file-access-from-files", "about:blank"], { stdout: "ignore", stderr: "ignore" })
  const portFile = join(profile, "DevToolsActivePort")
  // The file appears before the browser has finished writing it (and while it holds it locked).
  let text = ""
  for (let i = 0; i < 150 && !/\n\S/.test(text); i++) {
    try { text = readFileSync(portFile, "utf8") } catch {}
    if (!/\n\S/.test(text)) await Bun.sleep(100)
  }
  const [port, path] = text.split(/\r?\n/)
  const ws = new WebSocket(`ws://127.0.0.1:${port}${path}`)
  await new Promise((ok, fail) => ((ws.onopen = ok), (ws.onerror = fail)))
  const cdp = new Cdp(ws)
  browserCdp = cdp
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" })
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true })
  await cdp.send("Page.enable", {}, sessionId)
  await cdp.send("Runtime.enable", {}, sessionId)
  await cdp.send("Emulation.setDeviceMetricsOverride", { width, height: 900, deviceScaleFactor: 1, mobile: false }, sessionId)
  return { cdp, sid: sessionId }
}

async function evaluate(c: { cdp: Cdp; sid: string }, expression: string): Promise<any> {
  const r = await c.cdp.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true }, c.sid)
  if (r.exceptionDetails) throw new Error(`page: ${r.exceptionDetails.exception?.description || r.exceptionDetails.text}`)
  return r.result.value
}

async function load(c: { cdp: Cdp; sid: string }, url: string) {
  // Through about:blank, so a page differing only in its hash loads again.
  await c.cdp.send("Page.navigate", { url: "about:blank" }, c.sid)
  const loaded = c.cdp.next("Page.loadEventFired")
  await c.cdp.send("Page.navigate", { url }, c.sid)
  await loaded
  await evaluate(c, "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")
}

// Opens what a reader would open, the same way whichever unit is shown: all metrics, queries, the
// frame distribution, the history's point tables.
const EXPAND = `(() => {
  if (typeof state !== "undefined" && typeof render === "function") { state.metricsOpen = true; state.queriesOpen = true; state.distOpen = true; render(); }
  document.querySelectorAll("details").forEach((d) => { d.open = true; });
  return new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
})()`

// Every text a reader can see or hover (text, title, aria-label), in order, with where it is and
// whether a time in it is clipped.
const READ = `(() => {
  const TOKEN = ${TOKEN.toString()};
  // Its nearest boxes, and the section it is in (" @census").
  const ctxOf = (el) => {
    const parts = [];
    for (let e = el; e && e !== document.body && parts.length < 4; e = e.parentElement) {
      const c = (e.getAttribute("class") || "").trim().split(/\\s+/).filter(Boolean)[0];
      parts.push(e.tagName.toLowerCase() + (e.id ? "#" + e.id : "") + (c ? "." + c : ""));
    }
    const area = el.closest("[id]:not(#app)")?.id;
    return parts.join("<") + (area ? " @" + area : "");
  };
  const clippers = (el) => {
    const out = [];
    for (let e = el; e && e !== document.body; e = e.parentElement) {
      const s = getComputedStyle(e);
      // A box that scrolls keeps what it holds reachable (and so do its own boxes): only one that hides
      // or clips, inside it, cuts a number.
      if (/auto|scroll/.test(s.overflowX)) break;
      if (/hidden|clip/.test(s.overflowX)) out.push(e);
    }
    return out;
  };
  const items = [];
  const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let n;
  while ((n = w.nextNode())) {
    const p = n.parentElement;
    if (!p || p.closest("script,style") || !n.data.trim()) continue;
    const svg = p.closest("svg");
    // (A no-break space reads as a space: it only keeps a time on one line.) A blocks strip's end
    // ticks are an axis's labels, as a chart's are: one or two of them, as the ends print.
    const item = { t: n.data.replace(/\\u00a0/g, " "), ctx: ctxOf(p), svg: !!svg || !!p.closest(".blk-tick"), clip: null };
    TOKEN.lastIndex = 0;
    if (TOKEN.test(n.data) && p.getClientRects().length) {
      if (svg) {
        const b = p.getBBox();
        const vb = svg.viewBox?.baseVal;
        const W = vb && vb.width ? vb.width : svg.clientWidth;
        if (b.x < -0.5 || b.x + b.width > W + 0.5) item.clip = "outside its chart: x " + b.x.toFixed(1) + "–" + (b.x + b.width).toFixed(1) + " of " + W;
        else for (const o of svg.querySelectorAll("text")) {
          if (o === p || !o.textContent.trim()) continue;
          const q = o.getBBox();
          if (Math.abs(q.y - b.y) < 2 && q.x < b.x + b.width - 0.5 && b.x < q.x + q.width - 0.5) { item.clip = "on its neighbour '" + o.textContent + "'"; break; }
        }
      } else {
        const r = document.createRange();
        r.selectNodeContents(n);
        const t = r.getBoundingClientRect();
        if (t.width > 0) {
          const own = p.getBoundingClientRect();
          const s = getComputedStyle(p);
          if (s.display !== "inline" && t.right > own.right + 1) item.clip = "wider than its box by " + (t.right - own.right).toFixed(1) + " px";
          for (const c of clippers(p)) {
            const cr = c.getBoundingClientRect();
            if (t.right > cr.right + 1 || t.left < cr.left - 1) { item.clip = "cut by " + ctxOf(c) + " by " + Math.max(t.right - cr.right, cr.left - t.left).toFixed(1) + " px"; break; }
          }
          if (!item.clip && p.scrollWidth > p.clientWidth + 1 && /hidden|clip/.test(s.overflowX)) item.clip = "overflows its cell (" + p.scrollWidth + " > " + p.clientWidth + ")";
          // A number broken across lines (its column too narrow for it).
          if (!item.clip) for (const m of n.data.matchAll(TOKEN)) {
            const k = document.createRange();
            k.setStart(n, m.index + m[1].length);
            k.setEnd(n, m.index + m[0].length);
            const tops = new Set([...k.getClientRects()].filter((q) => q.width > 0).map((q) => Math.round(q.top)));
            if (tops.size > 1) { item.clip = "broken across lines: '" + m[0].trim() + "'"; break; }
          }
        }
      }
    }
    items.push(item);
  }
  // Each chart's tick labels: a text that is one number alone (with its unit, sign or %), where it sits.
  const TICK = /^[−+-]?<?\\d[\\d,]*(\\.\\d+)?\\s?(µs|μs|us|ms|s|%|pp|B|KB|MB|GB|score|\\/s)?$/;
  const charts = [...document.querySelectorAll("svg")].map((svg) => [...svg.querySelectorAll("text")]
    .filter((t) => TICK.test(t.textContent.trim()) && t.getClientRects().length)
    .map((t) => { const b = t.getBBox(); return { t: t.textContent.trim(), left: b.x, right: b.x + b.width, mid: b.y + b.height / 2, ctx: ctxOf(t) }; }))
    .filter((l) => l.length > 1);
  for (const el of document.querySelectorAll("[title],[aria-label]")) {
    for (const a of ["title", "aria-label"]) {
      const v = el.getAttribute(a);
      if (v) items.push({ t: v.replace(/\\u00a0/g, " "), ctx: "@" + a + ":" + ctxOf(el), svg: false, clip: null });
    }
  }
  const pressed = [...document.querySelectorAll('[aria-label="Time unit"] button[aria-pressed="true"]')].map((b) => b.dataset.unit);
  return { items, charts, hash: location.hash, pressed, mu: document.body.innerText.includes("\\u03bc") };
})()`

// A chart's axes: tick labels in a column (one right edge: a y-axis) or a row (one middle: an x-axis).
// No two of an axis's labels may read alike, and an axis whose labels share a unit steps evenly.
type Label = { t: string; left: number; right: number; mid: number; ctx: string }
function axisProblems(charts: Label[][]): string[] {
  const out: string[] = []
  for (const labels of charts) {
    const axes: Label[][] = []
    for (const key of ["right", "mid"] as const) {
      const groups = new Map<number, Label[]>()
      for (const l of labels) {
        const k = [...groups.keys()].find((g) => Math.abs(g - l[key]) < 1) ?? l[key]
        groups.set(k, [...(groups.get(k) || []), l])
      }
      axes.push(...[...groups.values()].filter((g) => g.length > 2))
    }
    for (const axis of axes) {
      const texts = axis.map((l) => l.t)
      const dup = texts.find((t, i) => texts.indexOf(t) !== i)
      if (dup) out.push(`'${dup}' twice on one axis: ${texts.join(" | ")} (${axis[0].ctx})`)
      // Any axis whose labels share one unit (a time, MB, %, none): binary MB ticks round in bytes
      // print uneven steps.
      const suffix = (t: string) => t.replace(/^[−+-]?<?[\d,]*(\.\d+)?\s?/, "")
      if (dup || axis.some((l) => l.t.startsWith("<") || suffix(l.t) !== suffix(axis[0].t))) continue
      const vals = axis.map((l) => (/^[−-]/.test(l.t) ? -1 : 1) * parseFloat(l.t.replace(/^[−+-]/, "").replace(/,/g, ""))).sort((a, b) => a - b)
      const dec = Math.max(...axis.map((l) => (l.t.match(/\.(\d+)/)?.[1] || "").length))
      const steps = vals.slice(1).map((v, i) => v - vals[i])
      // Round ticks print their steps exactly: any difference in the last digit is an uneven axis.
      if (steps.some((s) => Math.abs(s - steps[0]) > 0.5 * 10 ** -dec)) out.push(`uneven ticks: ${texts.join(" | ")} (${axis[0].ctx})`)
    }
  }
  return out
}

type Item = { t: string; ctx: string; svg: boolean; clip: string | null }
type Tok = { text: string; sign: string; floor: boolean; num: string; label: string; ctx: string; svg: boolean; whole: string; start: number; end: number }

// A number, with its unit or without: a bare one before `→`, `vs`, `…` or `;` takes the unit of the
// time that ends its run (`738,147 → 640,112 µs`, `latencies: 671; 571 ms`).
const NUM = /(^|[^\w.,])([−+-]?)(<?)(\d[\d,]*(?:\.\d+)?)(?:\s?(µs|μs|us|ms|s)(?!\w))?/g
const JOIN = /^\s*(→|vs|…|;)\s*$/

function tokens(items: Item[]): Tok[] {
  const out: Tok[] = []
  for (const it of items) {
    const ms = [...it.t.matchAll(NUM)].map((m) => ({ m, start: m.index! + m[1].length, end: m.index! + m[0].length }))
    let run: typeof ms = []
    ms.forEach((x, i) => {
      if (!x.m[5]) {
        const next = ms[i + 1]
        run = next && JOIN.test(it.t.slice(x.end, next.start)) ? [...run, x] : []
        return
      }
      for (const y of [...run, x]) out.push({ text: it.t.slice(y.start, y.end), sign: y.m[2], floor: !!y.m[3], num: y.m[4], label: x.m[5], ctx: it.ctx, svg: it.svg, whole: it.t, start: y.start, end: y.end })
      run = []
    })
  }
  return out
}

// The text with each time (and the bare numbers it labels) as ⟨t⟩.
function skeletonOf(items: Item[]): string {
  return items.filter((x) => !x.svg).map((x) => {
    const spans = tokens([x])
    let s = x.t
    for (const t of spans.sort((a, b) => b.start - a.start)) s = s.slice(0, t.start) + "⟨t⟩" + s.slice(t.end)
    return s
  }).join("\n")
}

const numOf = (t: Tok) => (t.sign === "−" || t.sign === "-" ? -1 : 1) * parseFloat(t.num.replace(/,/g, ""))
const decimals = (t: Tok) => (t.num.split(".")[1] || "").length
const grouped = (num: string) => /^\d{1,3}(,\d{3})*(\.\d+)?$/.test(num) && !/^\d{4,}/.test(num)
// The decimals a time of this size prints with: every whole digit and three significant ones, at most
// the unit's finest (fmt::time_digits). Read back from a printed number, which may sit either side of
// a step it was rounded across (99.96 prints 100): either size's decimals.
const ownDigits = (x: number, finest: number) => (x >= 100 ? 0 : x >= 10 ? 1 : x >= 1 ? 2 : Math.min(finest, [3, 4, 5, 6].find((k) => x * 10 ** k >= 100 - 1e-9) ?? finest))

// A time's text against its unit's rules: grouping, and (outside axes) its size's decimals; one
// printed beside others (an interval) prints as it would alone (fmt::times).
function formatProblem(t: Tok, u: Unit): string | null {
  if (t.label !== LABEL[u]) return `in ${t.label}, not ${LABEL[u]}`
  if (!grouped(t.num)) return "digits not grouped"
  if (t.svg || t.floor) return null
  const a = Math.abs(numOf(t))
  const d = decimals(t)
  const half = 0.5 * 10 ** -d
  const own = [ownDigits(a, FINEST[u]), ownDigits(Math.max(0, a - half), FINEST[u]), ownDigits(a + half, FINEST[u])]
  const ok = a === 0 ? d === 0 : own.includes(d)
  return ok ? null : `${d} decimals at ${a}`
}

// The same time in two units agrees within what each shows.
function agree(a: Tok, ua: Unit, b: Tok, ub: Unit): boolean {
  const ms = (t: Tok, u: Unit) => numOf(t) / PER_MS[u]
  const half = (t: Tok, u: Unit) => (0.5 * 10 ** -decimals(t)) / PER_MS[u]
  if (a.floor || b.floor) {
    const fa = a.floor ? Math.abs(ms(a, ua)) : Math.abs(ms(a, ua)) - half(a, ua)
    const fb = b.floor ? Math.abs(ms(b, ub)) : Math.abs(ms(b, ub)) - half(b, ub)
    return (a.floor ? fb <= fa * 1.0001 : true) && (b.floor ? fa <= fb * 1.0001 : true)
  }
  return Math.abs(ms(a, ua) - ms(b, ub)) <= half(a, ua) + half(b, ub) + 1e-12
}

const kept = (t: Tok) => KEPT.find(([re]) => re.test(t.whole))?.[1] || KEPT_CTX.find(([re]) => re.test(t.ctx))?.[1] || null

// The viewer's formatters against Rust's (`strobe stats`): the vectors and a sweep of values from
// 10⁻⁸ ms to 10⁶ ms (both signs, ties and steps a rounding crosses), each unit.
async function parity(c: { cdp: Cdp; sid: string }, kind: "report" | "history") {
  const sweep: number[] = [0]
  // Each the double nearest a short decimal (`9.995e-2`), which both sides read alike: serde_json
  // (without float_roundtrip) can read a 17-digit one a unit in the last place off.
  for (let e = -8; e <= 6; e++) for (const m of [1, 1.5, 2.5, 4.9996, 9.9996, 9.995, 12.345, 99.95, 99.996, 314.159]) sweep.push(parseFloat(`${m}e${e}`), -parseFloat(`${m}e${e}`))
  const groups: number[][] = [[-0.0032, -0.0012, 0.0124], [0.12, 0.05, 2.5], [0, -1e-7, 2e-7], [0.00003, 0.052], [-3837.429, -4012.11, -3661.804], [5.9, 0.004, 120.5], [0.3, 0.30005, 0.9999]]
  const bounds = [0.00004, 0.04, 0.52, 1.2, 14.76, 0.0000001]
  for (const u of UNITS) {
    const reqs = [
      ...sweep.map((v) => ({ op: "value", v, unit: "ms", time_unit: u })),
      ...sweep.map((v) => ({ op: "signed", v, unit: "ms", time_unit: u })),
      ...sweep.filter((v) => Math.abs(v) >= 1 || v === 0).map((v) => ({ op: "value", v: Math.round(v), unit: "count", time_unit: u })),
      // Other units (fmt::value), the history page's values included: never `-0.00`, Rust's digits.
      ...(u === "us" ? ["%", "MB", "value", "bytes", "instructions", "score", "pp", "/s", "count"].flatMap((unit) => sweep.map((v) => ({ op: "value", v, unit, time_unit: u }))) : []),
      ...groups.map((v) => ({ op: "times", v, signed: true, time_unit: u })),
      ...bounds.flatMap((b) => [
        { op: "verdict", c: { metric: "main.work", relative: false, bound: b, verdict: "same (±0 ms)" }, time_unit: u },
        { op: "verdict", c: { metric: "main.work", relative: false, bound: b, verdict: "no worse (≤1.20 ms, faster)" }, time_unit: u },
        { op: "verdict", c: { metric: "main.work", relative: false, bound: b, verdict: "worse (>1.20 ms)" }, time_unit: u },
      ]),
    ]
    const p = Bun.spawnSync([rust!, "stats"], { stdin: Buffer.from(reqs.map((r) => JSON.stringify(r)).join("\n") + "\n"), timeout: 60000 })
    const want = p.stdout.toString().trim().split(/\r?\n/).map((l) => JSON.parse(l))
    const js: string[][] = await evaluate(c, `(() => { TIME_UNIT = "${u}"; return ${JSON.stringify(reqs)}.map((r) =>
      r.op === "times" ? fmtTimes(r.v, true)
      : r.op === "verdict" ? [verdictText(r.c, "ms")]
      : r.unit !== "ms" ? [${kind === "report" ? `fmtVal(r.v, r.unit)` : `val(r.v, r.unit)`}]
      : r.op === "signed" ? [${kind === "report" ? `fmtSigned(r.v, "ms")` : `fmtTimeSigned(r.v)`}]
      : [${kind === "report" ? `fmtVal(r.v, "ms")` : `val(r.v, "ms")`}]); })()`)
    // Differences by kind (op and unit), each with its count and first case.
    const kinds = new Map<string, { n: number; first: string }>()
    const negZero: string[] = []
    reqs.forEach((r: any, i) => {
      const w = want[i]?.texts || [want[i]?.text]
      if (JSON.stringify(w) === JSON.stringify(js[i])) return
      // Rust's `{:.0}` of a tiny negative prints `-0`: the page's `0` is right (SPEC §1 rule 1), the
      // difference the Rust side's to close; counted apart so the rest still has to match.
      if (w.length === 1 && /^-0(\.0+)?( |$)/.test(w[0]) && w[0].slice(1) === js[i][0]) return void negZero.push(`${r.unit} ${r.v}: Rust '${w[0]}'`)
      const k = `${r.op} ${r.unit || ""}`.trim()
      const first = `${JSON.stringify(r.v ?? r.c?.bound)}${r.op === "verdict" ? ` '${r.c.verdict}'` : ""}: Rust ${JSON.stringify(w)}, page ${JSON.stringify(js[i])}`
      kinds.set(k, { n: (kinds.get(k)?.n || 0) + 1, first: kinds.get(k)?.first || first })
    })
    const bad = [...kinds].map(([k, { n, first }]) => `${k}: ${n} differ, e.g. ${first}`)
    add(`${kind}.html`, `same text as Rust's stats (${reqs.length} values)`, u, !bad.length && want.length === reqs.length, bad.length ? bad.slice(0, 10).join("\n      ") + (bad.length > 10 ? `\n      … ${bad.length - 10} more` : "") : `${reqs.length - negZero.length} alike${negZero.length ? `, ${negZero.length} where Rust prints -0` : ""}`)
    if (negZero.length) add(`${kind}.html`, "Rust prints -0 for a tiny negative, the page 0 (the Rust side's to fix)", u, false, `${negZero.length}: ${[...new Set(negZero.map((x) => x.split(" ")[0]))].join(", ")} units, e.g. ${negZero[0]}`)
  }
}

// --- Synthetic pages: small page data that reaches each place a review found wrong ---
// What a page must read: `text` some text in `where` (a CSS selector, default the page) includes each
// string, `not` no text matches; `kv` a recorded value's row reads so; `special` a check of its own.
type Expect =
  | { units: Unit[] | "all"; where?: string; text?: Partial<Record<Unit, string[]>>; not?: RegExp[]; check: string }
  | { kv: [string, Record<Unit, string>][]; check: string }
  | { special: "video" | "hash" | "anchor" | "forest-click" | "uniq"; where?: string; check: string }

const pairBlocks = (n: number, base: Record<string, number>, head: Record<string, number>, extra: (k: number, role: string) => object = () => ({})) =>
  Array.from({ length: n }).flatMap((_, k) => [
    { pair: k, variant: "base", values: Object.fromEntries(Object.entries(base).map(([m, v]) => [m, v * (1 + 0.001 * k)])), ...extra(k, "base") },
    { pair: k, variant: "head", values: Object.fromEntries(Object.entries(head).map(([m, v]) => [m, v * (1 + 0.001 * k)])), ...extra(k, "head") },
  ])
// A comparison by difference, decided, its interval about the change.
const cmp = (metric: string, base: number, head: number, o: object = {}) => {
  const d = head - base, w = Math.abs(d) / 2 || 0.01
  return { metric, base, head, estimate: d, lo: d - w, hi: d + w, relative: false, decided: true, verdict_kind: d > 0 ? "worse" : "better", verdict: d > 0 ? "worse" : "better", bound: 0.5, confidence: 0.95, p: 0.01, n: 6, ...o }
}
const experiment = (series: object[], o: object = {}) => ({
  view: "experiment",
  data: { kind: "experiment", id: "exp_synthetic", created: "2026-11-12T00:00:00Z", file: "synthetic.bench.ts", plan: { runs_per_block: 5 }, variants: [{ role: "base", label: "base", spec: "git:base" }, { role: "head", label: "head", spec: "git:head" }], series, ...o },
})
const recording = (data: object) => ({ view: "recording", data: { id: "rec_synthetic", kind: "record", target: "synthetic", created: "2026-11-12T00:00:00Z", duration_ms: 1000, valid: true, ...data } })
// A frame stream: 40 gaps a block about one 60 Hz frame, three of them 94.4 ms (the frame pacing
// view, its gaps' distribution reaching 100 ms).
const frames = (k: number) => ({ intervals: Array.from({ length: 40 }, (_, i) => (i < 3 ? 94.4 : 16.6 + ((i + k) % 5) * 0.05)) })

const SYNTHETIC: { name: string; payload: any; view?: string; expect: Expect[] }[] = [
  {
    // A decided shift between two times that print alike at base's digits (120.0 → 120.4 ms).
    name: "signed-difference",
    payload: experiment([{ label: "diff", kind: "runs", pairs: 6, primary: cmp("main.work", 120, 120.4), secondaries: [cmp("run.duration", 200, 200, { decided: false, verdict_kind: "untested", verdict: "—" })], blocks: pairBlocks(6, { "main.work": 120, "run.duration": 200 }, { "main.work": 120.4, "run.duration": 200 }) }]),
    expect: [{ check: "the change per run is its own size, never 0", units: "all", where: "p.hero-prose", text: { us: ["(+400 µs per 200,000 µs run)"], ms: ["(+0.400 ms per 200 ms run)"], s: ["(+0.000400 s per 0.200 s run)"] } }],
  },
  {
    // The same across tiers (8.45 µs → 120 µs): never 10 ns digits on a value over 100 µs.
    name: "signed-difference-tiers",
    payload: experiment([{ label: "diff", kind: "runs", pairs: 6, primary: cmp("main.work", 0.00845, 0.1204), secondaries: [cmp("run.duration", 200, 200, { decided: false, verdict_kind: "untested", verdict: "—" })], blocks: pairBlocks(6, { "main.work": 0.00845, "run.duration": 200 }, { "main.work": 0.1204, "run.duration": 200 }) }]),
    expect: [{ check: "the change per run at its own digits", units: "all", where: "p.hero-prose", text: { us: ["(+112 µs per 200,000 µs run)"], ms: ["(+0.112 ms per 200 ms run)"], s: ["(+0.000112 s per 0.200 s run)"] } }],
  },
  ...[["claim", "takes 12,400 µs less time in open"], ["claim-older", "takes 12.4 ms less time in open"]].map(([name, claim]) => ({
    // Two implementations: the headline's claim, a time compared by difference, written in one unit.
    name,
    payload: experiment([{
      label: "open", case: "open", kind: "runs", pairs: 6, claim,
      primary: cmp("phase.open", 12.4, 0, { verdict: "faster", holm_decided: true }),
      blocks: pairBlocks(6, { "phase.open": 12.4 }, { "phase.open": 0 }),
      // Where each spends its time: a function of 150 ms and a process of 18.3 s a run.
      hotspots: Object.fromEntries(["base", "head"].map((role) => [role, { recording: `rec_${role}`, runs: 5, side: { profiler: "v8", per_run: true, runs: 5, functions: [{ name: "render", ms: role === "base" ? 150 : 18309.038, where: "app.js:1" }], processes: [{ name: "app", pid: 1, ms: 18309.038 }] }, threads: { threads: [{ name: "main", cpu_ms_per_run: 18309.038 }] } }])),
    }], { comparison: "implementations", scoreboard: { compared: 1, better: 1 }, variants: [{ role: "base", name: "Original", target: "Original", label: "original" }, { role: "head", name: "Rewrite", target: "Rewrite", label: "rewrite" }] }),
    expect: [{ check: "the headline's time in the unit shown", units: "all", where: "h1.hero-headline", text: { us: ["Rewrite takes 12,400 µs less time in open than Original"], ms: ["Rewrite takes 12.4 ms less time in open than Original"], s: ["Rewrite takes 0.0124 s less time in open than Original"] } }] as Expect[],
  })),
  {
    // A kept block's recording (no metric catalog in the page): its times outside the catalog.
    name: "recording-values",
    payload: recording({ values: { "run.duration": 500, "untimed.ms": 172.42, "warmup.stepped.first": 84.5, "warmup.stepped.last": 61.27, "cpu.main": 12.5, "main.busy.cdp": 42.5, "http.latency.p50": 3.25, "driver.read_ms": 0.75 } }),
    expect: [{ check: "every recorded time in the unit shown", kv: [
      ["untimed.ms", { us: "172,420 µs", ms: "172 ms", s: "0.172 s" }],
      ["warmup.stepped.first", { us: "84,500 µs", ms: "84.5 ms", s: "0.0845 s" }],
      ["warmup.stepped.last", { us: "61,270 µs", ms: "61.3 ms", s: "0.0613 s" }],
      ["cpu.main", { us: "12,500 µs", ms: "12.5 ms", s: "0.0125 s" }],
      ["main.busy.cdp", { us: "42.5%", ms: "42.5%", s: "42.5%" }],
      ["http.latency.p50", { us: "3,250 µs", ms: "3.25 ms", s: "0.00325 s" }],
      ["driver.read_ms", { us: "750 µs", ms: "0.750 ms", s: "0.000750 s" }],
    ] }],
  },
  {
    // A count compared by difference: its axes' zero is 0, not 0%.
    name: "count-axes",
    payload: experiment([{ label: "items", kind: "runs", pairs: 6, primary: cmp("count.items", 10, 12, { lo: 1, hi: 3 }), blocks: pairBlocks(6, { "count.items": 10 }, { "count.items": 12 }) }]),
    expect: [{ check: "no 0% on a count's axes", units: "all", where: "svg", not: [/^0%$/] }],
  },
  {
    // Every big table at once, 8-digit times: phases, hotspots, movers, all metrics, frame pacing.
    name: "big-tables",
    view: "metric=main.work",
    payload: experiment([{
      label: "big", kind: "runs", pairs: 6,
      primary: cmp("main.work", 18309.038, 9155.019),
      secondaries: [cmp("main.script", 12345.678, 6172.839), cmp("main.style", 3456.789, 1728.394), cmp("main.layout", 2345.678, 1172.839), cmp("frames.interval.p95", 18.3, 17.1)],
      blocks: pairBlocks(6, { "main.work": 18309.038, "main.script": 12345.678, "main.style": 3456.789, "main.layout": 2345.678, "frames.n": 300, "display.hz": 60, "frames.interval.p95": 18.3 }, { "main.work": 9155.019, "main.script": 6172.839, "main.style": 1728.394, "main.layout": 1172.839, "frames.n": 300, "display.hz": 60, "frames.interval.p95": 17.1 }, frames),
      hotspots: Object.fromEntries(["base", "head"].map((role, i) => [role, { runs: 5, rows: [{ name: "render", where: "app.js:10:2", self_ms: (i ? 45775.095 : 91545.19) * 5 }, { name: "layoutAll", where: "app.js:20:2", self_ms: (i ? 1000 : 61728.39) * 5 }] }])),
    }], { insights: { series: [{ series: "big", movers: [{ kind: "CPU self time", function: "render app.js", base_per_run: 18309.038, head_per_run: 9.5, unit: "ms" }, { kind: "forced layout", function: "layoutAll app.js", base_per_run: 12345.678, head_per_run: 18309.038, unit: "ms" }], head: [] }] } }),
    expect: [{ check: "the section asked for starts below the header", special: "anchor" }, { check: "a metric's row scrolls to its pairs below the header", special: "forest-click" }],
  },
  {
    // A script recording: its whole host by lifecycle part, its hotspots and phases, 10 s and more.
    name: "recording-host",
    view: "scope=host",
    payload: recording({
      values: { "main.work": 30000, "main.script": 18309.038, "main.style": 5000, "main.layout": 3000 },
      attribution: { runs: 5, rows: [{ name: "render", where: "app.js:10:2", self_ms: 91545.19 }], whole_host: { label: "Whole host", rows: [{ name: "boot", where: "host.js:1:1", self_ms: 18309.038 }], lifecycle: [{ part: "startup", label: "startup", wall_ms: 10000, sampled_ms: 7779.349, spans: 1 }, { part: "runs", label: "runs", wall_ms: 18309.038, sampled_ms: 12345.678, spans: 5 }], modules: { count: 12, ms: 345.6, max_ms: 120.5, max_file: "x.js" } } },
    }),
    expect: [{ check: "the section asked for starts below the header", special: "anchor" }],
  },
  {
    // `--blocks 3` whose ends print alike in ms.
    name: "blocks-ends",
    payload: experiment([{ label: "blocks", kind: "blocks", primary: { metric: "main.work" }, blocks_summary: { blocks: 3, primary: { metric: "main.work", values: [738.1, 738.2, 738.4], median: 738.2, lo: 738.1, hi: 738.4, confidence: 0.75 } }, blocks: [0, 1, 2].map((k) => ({ pair: k, variant: "head", values: { "main.work": [738.1, 738.2, 738.4][k] } })) }]),
    expect: [{ check: "the strip's end ticks never read alike", special: "uniq", where: ".blk-tick" }],
  },
  {
    // A recording with a video: a unit switch keeps its player, frame and speed.
    name: "video",
    payload: recording({ values: { "run.duration": 500 }, video_player: { sides: [{ role: "recording", recording: "rec_synthetic", src: "data:video/mp4;base64,AAAA", video: { kind: "screen", width: 10, height: 10, fps: 30, duration_s: 2, runs: [{ run: "run:0", start_s: 0.1, end_s: 1.5, measured: true }], markers: [{ label: "press", kind: "input", t_s: 0.5, reaction_s: 0.55 }] } }] } }),
    expect: [{ check: "a unit switch keeps the player, its speed and its time", special: "video" }, { check: "`unit=` taken out of the address: the page's own unit", special: "hash" }],
  },
]

// History: a score near 0, two times 0.2 µs apart, a time whose axis spans 0 to 1 ms, a value and
// bytes.
const historySeries = (id: string, unit: string, medians: number[]) => ({
  id, meta: { label: id, metric: unit === "ms" ? "main.work" : `metric.${id}`, unit, file: "synthetic.bench.ts", fingerprint: {} },
  points: medians.map((m, i) => ({ commit: `c${i}`, short: `c${i}`, valid: true, median: m, lo: m, hi: m, n: 6, confidence: 0.97, at: "2026-11-12T00:00:00Z", subject: `commit ${i}`, exp: `exp_${i}` })),
  changes: [], marks: [],
})
const HISTORY = {
  name: "history",
  payload: { series: [historySeries("score", "score", [0, 0.0001, 0.00005]), historySeries("close", "ms", [0.004, 0.0042, 0.0041]), historySeries("span", "ms", [0.0967741935483871, 0.9032258064516129]), historySeries("value", "value", [101, 101, 101]), historySeries("bytes", "bytes", [3565158, 3565158, 3565158])], device: "synthetic", created: "2026-11-12T00:00:00Z" },
  expect: [
    { check: "no -0 on an axis", units: "all", where: "svg", not: [/^[-−]0(\.0+)?( |%|$)/] },
    { check: "values as the text prints them (fmt::value)", units: "all", where: "td", text: { us: ["101", "3.40 MB"], ms: ["101", "3.40 MB"], s: ["101", "3.40 MB"] }, not: [/^101 value$/] },
  ] as Expect[],
}

type Row = { page: string; check: string; unit: string; pass: boolean; detail: string }
const rows: Row[] = []
const add = (page: string, check: string, unit: string, pass: boolean, detail = "") => rows.push({ page, check, unit, pass, detail })
const keptList: { page: string; text: string; ctx: string; why: string }[] = []

type Page = { cdp: Cdp; sid: string }
const tick = (c: Page) => evaluate(c, "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")
// Every text in what `sel` selects (text nodes, trimmed).
// (A no-break space reads as a space: it only keeps a time on one line.)
const textsIn = (c: Page, sel: string): Promise<string[]> => evaluate(c, `[...document.querySelectorAll(${JSON.stringify(sel)})].flatMap((el) => { const out = []; const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT); let n; while ((n = w.nextNode())) if (n.data.trim()) out.push(n.data.trim().replace(/\\u00a0/g, " ")); return out; })`)
// A real click on what `sel` selects first (scrolled into view).
async function click(c: Page, sel: string): Promise<boolean> {
  const box = await evaluate(c, `(() => { const b = document.querySelector(${JSON.stringify(sel)}); if (!b) return null; b.scrollIntoView({ block: "center" }); const r = b.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`)
  if (!box) return false
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await c.cdp.send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "left", clickCount: 1 }, c.sid)
  await tick(c)
  return true
}
// Whether the section a view scrolled to starts below the sticky header (`what`: its selector).
const belowHeader = (c: Page, what: string) => evaluate(c, `(() => { const t = ${what}; const bar = document.querySelector("header.topbar"); if (!t || !bar) return "nothing to scroll to"; const top = t.getBoundingClientRect().top, under = bar.getBoundingClientRect().bottom; return top >= under - 1 ? "" : "its top at " + top.toFixed(0) + " px, under the header ending at " + under.toFixed(0) + " px"; })()`)
const VIEW_TARGET = (view: string) => (/(^|&)metric=/.test(view) ? `document.getElementById("evidence")` : /(^|&)scope=/.test(view) ? `document.querySelector('[aria-label="CPU profile scope"]')?.closest("section")` : null)

// A page's text expectations in the unit it shows now.
async function textExpect(c: Page, name: string, u: Unit, exps: Expect[]) {
  for (const e of exps) {
    if ("kv" in e) {
      const rows: [string, string][] = await evaluate(c, `[...document.querySelectorAll(".rec-kv-row")].map((r) => [r.children[0]?.textContent.trim(), r.children[1]?.textContent.trim().replace(/\\u00a0/g, " ")])`)
      const bad = e.kv.filter(([k, w]) => rows.find((r) => r[0] === k)?.[1] !== w[u]).map(([k, w]) => `${k}: '${rows.find((r) => r[0] === k)?.[1]}', want '${w[u]}'`)
      add(name, e.check, u, !bad.length, bad.join("; "))
    } else if ("special" in e && e.special === "uniq") {
      const t = await textsIn(c, e.where!)
      add(name, e.check, u, new Set(t).size === t.length, t.join(" | "))
    } else if ("units" in e) {
      const t = await textsIn(c, e.where || "body")
      const missing = (e.text?.[u] || []).filter((w) => !t.some((x) => x.includes(w)))
      const found = (e.not || []).flatMap((re) => t.filter((x) => re.test(x)))
      add(name, e.check, u, !missing.length && !found.length, [missing.length ? `missing ${missing.map((m) => `'${m}'`).join(", ")}` : "", found.length ? `found ${found.map((m) => `'${m}'`).join(", ")}` : "", missing.length ? `(read: ${t.filter((x) => /\d/.test(x)).slice(0, 6).join(" | ")})` : ""].filter(Boolean).join(" "))
    }
  }
}

// A page's own checks: the video player kept, `unit=` taken out, scrolled sections below the header.
async function specialExpect(c: Page, name: string, url: string, view: string, path: string, exps: Expect[]) {
  for (const e of exps) {
    if (!("special" in e)) continue
    if (e.special === "video") {
      await load(c, `${url}#unit=us`)
      const set = await click(c, ".vid-rate:first-of-type")
      await evaluate(c, `(() => { const v = document.querySelector("video"); v.__kept = 1; })()`)
      await click(c, '[aria-label="Time unit"] button[data-unit="ms"]')
      const r = await evaluate(c, `(() => { const v = document.querySelector("video"); const b = [...document.querySelectorAll(".vid-rate")].find((x) => x.getAttribute("aria-pressed") === "true"); return { kept: v?.__kept === 1, rate: v?.playbackRate, pressed: b?.textContent, unit: [...document.querySelectorAll('[aria-label="Time unit"] button[aria-pressed="true"]')].map((x) => x.dataset.unit).join() }; })()`)
      add(name, e.check, "ms", set && r.kept && r.rate === 0.25 && r.pressed === "0.25×" && r.unit === "ms", JSON.stringify(r))
    } else if (e.special === "hash") {
      for (const def of [null, "ms"]) {
        const target = def ? withDefault(path, def) : url
        await load(c, `${target}#series=0&unit=s`)
        await evaluate(c, `new Promise((r) => { addEventListener("hashchange", () => setTimeout(r, 50), { once: true }); location.hash = "#series=0"; })`)
        const r = await evaluate(c, READ)
        const want = def || "us"
        add(name, `${e.check}${def ? ` (default_unit ${def})` : ""}`, want, r.pressed.join() === want, r.pressed.join())
      }
    } else if (e.special === "forest-click") {
      await load(c, `${url}#unit=us`)
      await evaluate(c, EXPAND)
      await evaluate(c, "window.scrollTo(0, document.body.scrollHeight)")
      await click(c, ".forest-row:not(.hd)")
      await Bun.sleep(1200)
      const p = await belowHeader(c, `document.getElementById("evidence")`)
      add(name, e.check, "us", !p, p)
    } else if (e.special === "anchor") {
      // Run for every page with a view (below), this one included.
    }
  }
}

// A copy of a page that carries `default_unit` (in place of any it has).
function withDefault(path: string, def: string): string {
  const html = readFileSync(path, "utf8")
  const m = /const (STROBE|DATA) = (.*);\r?\n/.exec(html)!
  const data = JSON.parse(m[2])
  data.default_unit = def
  const out = path.replace(/\.html$/, `.default-${def}.html`)
  writeFileSync(out, html.replace(m[0], () => `const ${m[1]} = ${JSON.stringify(data).replace(/<\//g, "<\\/")};\n`))
  return "file:///" + out.replace(/\\/g, "/")
}

async function main() {
  // The synthetic pages, as a strobe would write them.
  const src = join(outDir, "synthetic")
  mkdirSync(src, { recursive: true })
  for (const s of [...SYNTHETIC.map((x) => ({ ...x, kind: "STROBE" })), { ...HISTORY, view: undefined, kind: "DATA" }]) {
    const file = join(src, `synthetic-${s.name}.html`)
    writeFileSync(file, `<script>\nconst ${s.kind} = ${JSON.stringify(s.payload).replace(/<\//g, "<\\/")};\n</script>\n`)
    pages.push({ file, view: s.view || "", expect: s.expect })
  }
  // The shared block reads alike in both viewers.
  const block = (s: string) => s.slice(s.indexOf("/* ---------- Time units (shared"), s.indexOf("/* ---------- end of the shared time units ---------- */"))
  add("viewers", "shared time block identical", "-", block(VIEWERS.report) === block(VIEWERS.history) && block(VIEWERS.report).length > 500)
  add("viewers", "the label is the micro sign (U+00B5)", "-", block(VIEWERS.report).includes('label: "\u00b5s"'))

  const c = await open()
  const vectorsDone = { report: false, history: false }
  for (const { file, view, expect = [] } of pages) {
    const { kind, path } = retemplate(file)
    const name = basename(file, ".html") + (view ? `#${view}` : "")
    const url = "file:///" + path.replace(/\\/g, "/")
    const keys = view || "series=0"

    if (!vectorsDone[kind]) {
      vectorsDone[kind] = true
      await load(c, url)
      for (const u of UNITS) {
        const got: string[] = await evaluate(c, `(() => { TIME_UNIT = "${u}"; return ${JSON.stringify(VECTORS)}.map((x) => x.fn === "times" ? fmtTimes(x.v, true).join(" … ")
          : x.fn === "verdict" ? verdictText({ relative: false, bound: x.v, verdict: x.v > 1 ? "no worse (≤1.20 ms, faster)" : "same (±0 ms)" }, "ms")
          : ${kind === "report"
          ? `x.fn === "plain" ? fmtVal(x.v, x.unit) : x.fn === "val" ? fmtVal(x.v, "ms") : x.fn === "signed" ? fmtSigned(x.v, "ms") : x.fn === "count" ? fmtVal(x.v, "count") : fmtBound({ relative: false, bound: x.v }, "ms")`
          : `x.fn === "plain" ? val(x.v, x.unit) : x.fn === "val" ? val(x.v, "ms") : x.fn === "signed" ? fmtTimeSigned(x.v) : x.fn === "count" ? val(x.v, "count") : "±" + val(x.v, "ms")`}); })()`)
        VECTORS.forEach((x, i) => add(`${kind}.html`, `${x.fn}(${x.v} ${x.unit || "ms"})`, u, got[i] === x.want[u], got[i] === x.want[u] ? got[i] : `got ${got[i]}, want ${x.want[u]}`))
      }
      if (rust) await parity(c, kind)
    }

    // Without `#unit=`: the page data's `default_unit` (its own, then one set in its place), else µs.
    const own = JSON.parse(/const (?:STROBE|DATA) = (.*);\r?\n/.exec(readFileSync(path, "utf8"))![1])?.default_unit
    for (const def of [null, "ms", "s"]) {
      // A page written with a `default_unit` of its own has it replaced, not shadowed.
      const target = def ? withDefault(path, def) : url
      await load(c, view ? `${target}#${view}` : target)
      const r = await evaluate(c, READ)
      const want = def || own || "us"
      add(name, `opens in ${def ? `default_unit ${def}` : own ? `its default_unit ${own}` : "µs without default_unit"}`, want, r.pressed.join() === want && !new URLSearchParams(r.hash.replace(/^#/, "")).has("unit"), r.pressed.join())
    }
    // A view scrolled to starts below the sticky header (a page without that view scrolls nowhere; a
    // synthetic page always has it).
    if (VIEW_TARGET(view)) {
      await load(c, `${url}#${view}&unit=us`)
      const p = await belowHeader(c, VIEW_TARGET(view)!)
      if (p !== "nothing to scroll to" || expect.length) add(name, "the section scrolled to starts below the header", "us", !p, p)
    }

    // Fresh at each unit.
    const fresh: Record<string, any> = {}
    for (const u of UNITS) {
      await load(c, `${url}#${view ? view + "&" : ""}unit=${u}`)
      await evaluate(c, EXPAND)
      fresh[u] = await evaluate(c, READ)
      add(name, "switch shows the unit", u, fresh[u].pressed.join() === u, fresh[u].pressed.join())
      add(name, "no Greek mu", u, !fresh[u].mu)
      const ap = axisProblems(fresh[u].charts)
      add(name, "no axis prints two ticks alike or steps unevenly", u, !ap.length, ap.slice(0, 6).join("\n      "))
      await textExpect(c, name, u, expect)
    }
    await specialExpect(c, name, url, view, path, expect)
    // Switched by a click from the unit before it, with other keys in the address.
    for (const [i, u] of UNITS.entries()) {
      const from = UNITS[(i + 2) % 3]
      await load(c, `${url}#${keys}&unit=${from}`)
      await evaluate(c, EXPAND)
      await evaluate(c, "window.scrollTo(0, 0)")
      const box = await evaluate(c, `(() => { const b = document.querySelector('[aria-label="Time unit"] button[data-unit="${u}"]'); if (!b) return null; const r = b.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`)
      if (!box) { add(name, "switch present", u, false, "no switch"); continue }
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await c.cdp.send("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "left", clickCount: 1 }, c.sid)
      await evaluate(c, "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")
      await evaluate(c, EXPAND)
      const sw = await evaluate(c, READ)
      const p = new URLSearchParams(sw.hash.replace(/^#/, ""))
      add(name, "address keeps its keys", u, p.get("unit") === u && [...new URLSearchParams(keys)].every(([k, v]) => p.get(k) === v), sw.hash)
      const a = sw.items.map((x: Item) => x.t).join("\n"), b = fresh[u].items.map((x: Item) => x.t).join("\n")
      let at = 0
      while (at < a.length && a[at] === b[at]) at++
      add(name, "click reads as a fresh page", u, a === b, a === b ? `${sw.items.length} texts` : `differs at …${a.slice(Math.max(0, at - 40), at + 40)}… vs …${b.slice(Math.max(0, at - 40), at + 40)}…`)
    }

    // Every time, unit by unit and across units.
    const toks: Record<string, Tok[]> = {}
    for (const u of UNITS) toks[u] = tokens(fresh[u].items as Item[])
    const flat = (u: Unit) => toks[u].filter((t) => !t.svg)
    const skeleton = (u: Unit) => skeletonOf(fresh[u].items as Item[])
    const sameShape = skeleton("us") === skeleton("ms") && skeleton("ms") === skeleton("s")
    const firstDiff = () => {
      const [a, b, c] = UNITS.map((u) => skeleton(u).split("\n"))
      const i = a.findIndex((l, k) => l !== b[k] || l !== c[k])
      return i < 0 ? `${a.length} / ${b.length} / ${c.length} lines` : `line ${i}: '${a[i]}' / '${b[i]}' / '${c[i]}'`
    }
    add(name, "same text in every unit but its times", "all", sameShape, sameShape ? "" : firstDiff())
    if (sameShape) {
      const [A, B, C] = UNITS.map(flat)
      let converted = 0, keptN = 0
      const bad: string[] = []
      A.forEach((ta, i) => {
        const tb = B[i], tc = C[i]
        // Zero reads 0 in every unit: only its unit says it converted.
        const zero = [ta, tb, tc].every((t) => numOf(t) === 0 && !t.floor)
        if (ta.text === tb.text && tb.text === tc.text && !(zero && ta.label === "µs" && tb.label === "ms" && tc.label === "s")) {
          keptN++
          const why = kept(ta)
          if (!why) bad.push(`'${ta.text}' stays in every unit: ${ta.whole.slice(0, 80)} (${ta.ctx})`)
          keptList.push({ page: name, text: ta.text, ctx: ta.ctx, why: why || "NOT EXPLAINED" })
          return
        }
        converted++
        for (const [t, u] of [[ta, "us"], [tb, "ms"], [tc, "s"]] as [Tok, Unit][]) {
          const p = formatProblem(t, u)
          if (p) bad.push(`'${t.text}' (${u}): ${p} · ${t.whole.slice(0, 80)} (${t.ctx})`)
        }
        if (!agree(ta, "us", tb, "ms") || !agree(tb, "ms", tc, "s")) bad.push(`'${ta.text}' / '${tb.text}' / '${tc.text}' disagree (${ta.ctx})`)
      })
      add(name, "every time converted, grouped, at its decimals, agreeing", "all", !bad.length, bad.length ? bad.slice(0, 12).join("\n      ") + (bad.length > 12 ? `\n      … ${bad.length - 12} more` : "") : `${converted} times in 3 units · ${keptN} kept`)
    }
    // A chart label the same in every unit is a wall-clock one only on a video's strip or a census
    // (in seconds, and listed). Any other is in the page's unit.
    const svgIn = (u: Unit) => toks[u].filter((t) => t.svg)
    const wall = new Set(svgIn("us").filter((t) => t.label === "s" && KEPT_SVG.test(t.ctx)).map((t) => t.text).filter((x) => svgIn("ms").some((t) => t.text === x) && svgIn("s").some((t) => t.text === x)))
    for (const t of svgIn("us").filter((t) => wall.has(t.text))) keptList.push({ page: name, text: t.text, ctx: t.ctx, why: "wall-clock seconds on a video's strip or a census" })
    for (const u of UNITS) {
      const labels = svgIn(u).filter((t) => !(wall.has(t.text) && KEPT_SVG.test(t.ctx)))
      const axis = labels.map((t) => formatProblem(t, u) && `'${t.text}': ${formatProblem(t, u)} (${t.ctx})`).filter(Boolean)
      add(name, "chart labels in the unit, grouped", u, !axis.length, axis.slice(0, 6).join("\n      ") || `${labels.length} labels`)
      const clipped = (fresh[u].items as Item[]).filter((x) => x.clip).map((x) => `'${x.t.slice(0, 50)}' ${x.clip} (${x.ctx})`)
      add(name, "no time clipped", u, !clipped.length, clipped.slice(0, 8).join("\n      ") + (clipped.length > 8 ? `\n      … ${clipped.length - 8} more` : ""))
    }
    // Stress: every time 100× its size in µs (a 180 ms run reads 18,000,000 µs), so each cell, axis
    // and box is tried with numbers of eight digits and more.
    await load(c, `${url}#${view ? view + "&" : ""}unit=us`)
    await evaluate(c, `(() => { TIME_UNITS.us.perMs = 100000; render(); })()`)
    await evaluate(c, EXPAND)
    const big = await evaluate(c, READ)
    const longest = tokens(big.items as Item[]).reduce((m, t) => Math.max(m, t.num.length), 0)
    const clippedBig = (big.items as Item[]).filter((x) => x.clip).map((x) => `'${x.t.slice(0, 50)}' ${x.clip} (${x.ctx})`)
    add(name, "no time clipped at 100× (stress)", "us", !clippedBig.length, clippedBig.length ? clippedBig.slice(0, 10).join("\n      ") + (clippedBig.length > 10 ? `\n      … ${clippedBig.length - 10} more` : "") : `longest number ${longest} characters`)
    const apBig = axisProblems(big.charts)
    add(name, "no axis prints two ticks alike at 100× (stress)", "us", !apBig.length, apBig.slice(0, 6).join("\n      "))
  }
}

try {
  await main()
} catch (e) {
  add("run", "completed", "-", false, String(e))
} finally {
  clearTimeout(watchdog)
  await closeBrowser()
}

const failed = rows.filter((r) => !r.pass)
const lines = rows.map((r) => `${r.pass ? "PASS" : "FAIL"}  ${r.page.padEnd(30)} ${r.unit.padEnd(4)} ${r.check}${r.detail && (!r.pass || r.check.startsWith("every") || r.check.includes("stress") || r.check.includes("Rust")) ? `\n      ${r.detail}` : ""}`)
const keptLines = [...new Map(keptList.map((k) => [`${k.text}|${k.why}|${k.ctx}`, k])).values()].map((k) => `  ${k.page}: '${k.text}' · ${k.why} · ${k.ctx}`)
const text = `${lines.join("\n")}\n\nTimes kept in every unit (${keptLines.length}):\n${keptLines.join("\n")}\n\n${rows.length - failed.length} passed, ${failed.length} failed\n`
console.log(text)
if (outArg) {
  writeFileSync(join(outDir, "results.txt"), text)
  writeFileSync(join(outDir, "results.json"), JSON.stringify({ rows, kept: keptList }, null, 1))
}
for (let i = 0; i < 40 && (existsSync(profile) || (!outArg && existsSync(outDir))); i++) {
  cleanUp()
  if (existsSync(profile)) await Bun.sleep(500)
}
process.exit(failed.length ? 1 : 0)
