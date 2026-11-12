// The HTML viewers' time units, checked in a headless browser: every measured time on a report or
// history page in µs (the default), ms or s, switched by clicking the header's switch.
//
//   bun scripts/dev/viewer-units.ts <page.html>... [--out <dir>] [--browser <path>] [--width 1280]
//     [--rust <strobe>]
//
// Each page is any `strobe report … --format html` or `strobe report history …` file: its data is
// put into this tree's viewer/report.html or viewer/history.html, so a page written by an installed
// strobe checks the viewer being edited. Checks, per page and unit:
//   - the formatters against the table below (and the shared block alike in both viewers);
//   - a page opened at `#unit=X` and one switched to X by a click read the same, and the address keeps
//     its other keys;
//   - every time reads in X with grouped digits at its magnitude's decimals; the same time in the
//     three units agrees within rounding; a time that does not change with the unit is wall-clock or
//     stored text (listed, and failed when it says ms or µs outside the known places);
//   - no time is clipped: a number wider than its cell or box, an axis label outside its chart or on
//     its neighbour.
// Prints a pass/fail table; `--out` also writes it (results.md, results.json) with the pages.
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
const outDir = resolve(opt("--out") || mkdtempSync(join(tmpdir(), "viewer-units-")))
const browserArg = opt("--browser")
const width = Number(opt("--width") || 1280)
// `--rust <strobe>`: a strobe whose `stats` prints times (op value, signed, times, verdict): every
// vector and a sweep of values across magnitudes is checked against it, character for character.
const rust = opt("--rust")
// `page.html#scope=host` checks the page in that view too (its keys go before `unit=`).
const pages = args.map((p) => ({ file: resolve(p.split("#")[0]), view: p.includes("#") ? p.slice(p.indexOf("#") + 1) : "" }))
if (!pages.length) {
  console.error("usage: bun scripts/dev/viewer-units.ts <page.html>... [--out <dir>] [--browser <path>]")
  process.exit(2)
}
mkdirSync(outDir, { recursive: true })

const root = join(import.meta.dir, "..", "..")
const VIEWERS = { report: readFileSync(join(root, "viewer/report.html"), "utf8"), history: readFileSync(join(root, "viewer/history.html"), "utf8") }
const UNITS = ["us", "ms", "s"] as const
type Unit = (typeof UNITS)[number]
const LABEL: Record<Unit, string> = { us: "µs", ms: "ms", s: "s" }
const PER_MS: Record<Unit, number> = { us: 1000, ms: 1, s: 0.001 }
// The most decimals a time shows in each unit (fmt::TimeUnit::finest).
const FINEST: Record<Unit, number> = { us: 2, ms: 4, s: 6 }

// The expected text of each formatter for a value in ms, as the Rust seam prints it (fmt::time,
// time_signed, bound, times; the table us-A left at cleanup/usa, then more): `val` fmtVal(v, "ms"),
// `signed` fmtSigned, `bound` fmtBound of an absolute bound, `times` values printed together.
const VECTORS: { fn: string; v: number | number[]; want: Record<Unit, string> }[] = [
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
  { fn: "times", v: [-0.0032, -0.0012, 0.0124], want: { us: "−3.2 µs … −1.2 µs … +12.4 µs", ms: "−0.0032 ms … −0.0012 ms … +0.0124 ms", s: "−0.000003 s … −0.000001 s … +0.000012 s" } },
  { fn: "times", v: [-3837.429, -4012.11, -3661.804], want: { us: "−3,837,429 µs … −4,012,110 µs … −3,661,804 µs", ms: "−3,837 ms … −4,012 ms … −3,662 ms", s: "−3.84 s … −4.01 s … −3.66 s" } },
  { fn: "times", v: [0.12, 0.05, 2.5], want: { us: "+120 µs … +50 µs … +2,500 µs", ms: "+0.12 ms … +0.05 ms … +2.50 ms", s: "+0.00012 s … +0.00005 s … +0.00250 s" } },
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
]

// A time as a page shows it: sign, `<` floor, number, unit. `us` (ASCII) and `μs` (Greek mu) are
// never right in a page: they are caught to fail.
const TOKEN = /(^|[^\w.,])([−+-]?)(<?)(\d[\d,]*(?:\.\d+)?)\s?(µs|μs|us|ms|s)(?!\w)/g

// Times that do not change with the unit, and why that is right. Anything else that keeps `ms` or
// `µs` in every unit fails.
const KEPT: [RegExp, string][] = [
  [/^Long tasks \(>50 ms\)$|tasks over 50 ms/, "a metric's name or definition: a long task is one over 50 ms"],
  [/[\w-]\d+(ms|µs)\b|\d(ms|µs)-/, "a name the bench file gave (`stall-750ms`): no space, never a formatted time"],
  [/every \d+ ms · timeout \d+ ms/, "the HTTP probe's configuration, as given"],
  [/timed out at \d+ ms/, "the HTTP probe's timeout, as configured"],
  [/^Note: /, "a note stored with the result (its warnings' words)"],
  [/^Series runtime: |^Duration: |One cold run, no warmup|\(~\d[\d.]* (ms|s|min|hr)\)/, "a wall-clock stretch (fmt::ms_duration: `850 ms` in every unit)"],
]
const KEPT_CTX: [RegExp, string][] = [
  [/plan-v|plan-seg|eyebrow-meta/, "a wall-clock stretch (fmt::ms_duration: `850 ms` in every unit)"],
  [/cli-pre/, "the stored CLI text (drawn again by the Rust side)"],
  [/caveat-text|ledger|ins-title|insight-card|bnode|bs-verdict|hero-prose/, "words stored with the result (verdicts, warnings, insights)"],
]

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
const profile = mkdtempSync(join(tmpdir(), "viewer-units-profile-"))
const watchdog = setTimeout(() => {
  console.error("viewer-units: no result in 10 min, stopping")
  browser?.kill()
  process.exit(3)
}, 600000)

async function open(): Promise<{ cdp: Cdp; sid: string }> {
  browser = Bun.spawn([findBrowser(), "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars", "--mute-audio", "--allow-file-access-from-files", "about:blank"], { stdout: "ignore", stderr: "ignore" })
  const portFile = join(profile, "DevToolsActivePort")
  for (let i = 0; i < 150 && !existsSync(portFile); i++) await Bun.sleep(100)
  const [port, path] = readFileSync(portFile, "utf8").split(/\r?\n/)
  const ws = new WebSocket(`ws://127.0.0.1:${port}${path}`)
  await new Promise((ok, fail) => ((ws.onopen = ok), (ws.onerror = fail)))
  const cdp = new Cdp(ws)
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
  const ctxOf = (el) => {
    const parts = [];
    for (let e = el; e && e !== document.body && parts.length < 4; e = e.parentElement) {
      const c = (e.getAttribute("class") || "").trim().split(/\\s+/).filter(Boolean)[0];
      parts.push(e.tagName.toLowerCase() + (e.id ? "#" + e.id : "") + (c ? "." + c : ""));
    }
    return parts.join("<");
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
    const item = { t: n.data, ctx: ctxOf(p), svg: !!svg, clip: null };
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
        }
      }
    }
    items.push(item);
  }
  for (const el of document.querySelectorAll("[title],[aria-label]")) {
    for (const a of ["title", "aria-label"]) {
      const v = el.getAttribute(a);
      if (v) items.push({ t: v, ctx: "@" + a + ":" + ctxOf(el), svg: false, clip: null });
    }
  }
  const pressed = [...document.querySelectorAll('[aria-label="Time unit"] button[aria-pressed="true"]')].map((b) => b.dataset.unit);
  return { items, hash: location.hash, pressed, mu: document.body.innerText.includes("\\u03bc") };
})()`

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

// A time's text against its unit's rules: grouping, and (outside axes and intervals) its size's
// decimals.
function formatProblem(t: Tok, u: Unit): string | null {
  if (t.label !== LABEL[u]) return `in ${t.label}, not ${LABEL[u]}`
  if (!grouped(t.num)) return "digits not grouped"
  if (t.svg || t.floor) return null
  const a = Math.abs(numOf(t))
  const d = decimals(t)
  const half = 0.5 * 10 ** -d
  const own = [ownDigits(a, FINEST[u]), ownDigits(Math.max(0, a - half), FINEST[u]), ownDigits(a + half, FINEST[u])]
  // Printed together (fmt::times): at the decimals of the largest beside it, or its own first digit's.
  // (An estimate's cell or an interval's end in its own span shows no `…`: the rows that print them.)
  const shared = d <= FINEST[u] && (/…|\[/.test(t.whole) || /forest-row|phase-trow|pil-|sc-verdict|ruler|hero/.test(t.ctx))
  const ok = a === 0 ? d === 0 : own.includes(d) || shared
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
      : r.unit === "count" ? [${kind === "report" ? `fmtVal(r.v, "count")` : `val(r.v, "count")`}]
      : r.op === "signed" ? [${kind === "report" ? `fmtSigned(r.v, "ms")` : `fmtTimeSigned(r.v)`}]
      : [${kind === "report" ? `fmtVal(r.v, "ms")` : `val(r.v, "ms")`}]); })()`)
    const bad: string[] = []
    reqs.forEach((r: any, i) => {
      const w = want[i]?.texts || [want[i]?.text]
      if (JSON.stringify(w) !== JSON.stringify(js[i])) bad.push(`${r.op} ${JSON.stringify(r.v ?? r.c?.bound)}${r.op === "verdict" ? ` '${r.c.verdict}'` : ""}: Rust ${JSON.stringify(w)}, page ${JSON.stringify(js[i])}`)
    })
    add(`${kind}.html`, `same text as Rust's stats (${reqs.length} values)`, u, !bad.length && want.length === reqs.length, bad.length ? bad.slice(0, 10).join("\n      ") + (bad.length > 10 ? `\n      … ${bad.length - 10} more` : "") : `${reqs.length} alike`)
  }
}

type Row = { page: string; check: string; unit: string; pass: boolean; detail: string }
const rows: Row[] = []
const add = (page: string, check: string, unit: string, pass: boolean, detail = "") => rows.push({ page, check, unit, pass, detail })
const keptList: { page: string; text: string; ctx: string; why: string }[] = []

async function main() {
  // The shared block reads alike in both viewers.
  const block = (s: string) => s.slice(s.indexOf("/* ---------- Time units (shared"), s.indexOf("/* ---------- end of the shared time units ---------- */"))
  add("viewers", "shared time block identical", "-", block(VIEWERS.report) === block(VIEWERS.history) && block(VIEWERS.report).length > 500)
  add("viewers", "the label is the micro sign (U+00B5)", "-", block(VIEWERS.report).includes('label: "\u00b5s"'))

  const c = await open()
  const vectorsDone = { report: false, history: false }
  for (const { file, view } of pages) {
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
          ? `x.fn === "val" ? fmtVal(x.v, "ms") : x.fn === "signed" ? fmtSigned(x.v, "ms") : x.fn === "count" ? fmtVal(x.v, "count") : fmtBound({ relative: false, bound: x.v }, "ms")`
          : `x.fn === "val" ? val(x.v, "ms") : x.fn === "signed" ? fmtTimeSigned(x.v) : x.fn === "count" ? val(x.v, "count") : "±" + val(x.v, "ms")`}); })()`)
        VECTORS.forEach((x, i) => add(`${kind}.html`, `${x.fn}(${x.v} ms)`, u, got[i] === x.want[u], got[i] === x.want[u] ? got[i] : `got ${got[i]}, want ${x.want[u]}`))
      }
      if (rust) await parity(c, kind)
    }

    // Without `#unit=`: the page data's `default_unit`, else µs.
    for (const def of [null, "ms", "s"]) {
      let target = url
      if (def) {
        const p = path.replace(/\.html$/, `.default-${def}.html`)
        writeFileSync(p, readFileSync(path, "utf8").replace(/const (STROBE|DATA) = \{/, (m) => `${m}"default_unit":"${def}",`))
        target = "file:///" + p.replace(/\\/g, "/")
      }
      await load(c, view ? `${target}#${view}` : target)
      const r = await evaluate(c, READ)
      const want = def || "us"
      add(name, `opens in ${def ? `default_unit ${def}` : "µs without default_unit"}`, want, r.pressed.join() === want && !new URLSearchParams(r.hash.replace(/^#/, "")).has("unit"), r.pressed.join())
    }

    // Fresh at each unit.
    const fresh: Record<string, any> = {}
    for (const u of UNITS) {
      await load(c, `${url}#${view ? view + "&" : ""}unit=${u}`)
      await evaluate(c, EXPAND)
      fresh[u] = await evaluate(c, READ)
      add(name, "switch shows the unit", u, fresh[u].pressed.join() === u, fresh[u].pressed.join())
      add(name, "no Greek mu", u, !fresh[u].mu)
    }
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
          if (ta.label !== "s" && !why) bad.push(`'${ta.text}' stays in every unit: ${ta.whole.slice(0, 80)} (${ta.ctx})`)
          keptList.push({ page: name, text: ta.text, ctx: ta.ctx, why: why || (ta.label === "s" ? "wall-clock seconds" : "NOT EXPLAINED") })
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
    // A chart label the same in every unit is a wall-clock one (a video's clock, a census's time): in
    // seconds, and listed. Any other is in the page's unit.
    const svgIn = (u: Unit) => toks[u].filter((t) => t.svg)
    const wall = new Set(svgIn("us").map((t) => t.text).filter((x) => svgIn("ms").some((t) => t.text === x) && svgIn("s").some((t) => t.text === x)))
    for (const t of svgIn("us").filter((t) => wall.has(t.text))) keptList.push({ page: name, text: t.text, ctx: t.ctx, why: t.label === "s" ? "wall-clock seconds on a chart" : "NOT EXPLAINED" })
    for (const u of UNITS) {
      const labels = svgIn(u).filter((t) => !(wall.has(t.text) && t.label === "s"))
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
  }
}

try {
  await main()
} catch (e) {
  add("run", "completed", "-", false, String(e))
} finally {
  clearTimeout(watchdog)
  browser?.kill()
  await browser?.exited
  try { rmSync(profile, { recursive: true, force: true }) } catch {}
}

const failed = rows.filter((r) => !r.pass)
const lines = rows.map((r) => `${r.pass ? "PASS" : "FAIL"}  ${r.page.padEnd(22)} ${r.unit.padEnd(4)} ${r.check}${r.detail && (!r.pass || r.check.startsWith("every") || r.check.includes("stress") || r.check.includes("Rust")) ? `\n      ${r.detail}` : ""}`)
const keptLines = [...new Map(keptList.map((k) => [`${k.text}|${k.why}|${k.ctx}`, k])).values()].map((k) => `  ${k.page}: '${k.text}' · ${k.why} · ${k.ctx}`)
const text = `${lines.join("\n")}\n\nTimes kept in every unit (${keptLines.length}):\n${keptLines.join("\n")}\n\n${rows.length - failed.length} passed, ${failed.length} failed\n`
console.log(text)
writeFileSync(join(outDir, "results.txt"), text)
writeFileSync(join(outDir, "results.json"), JSON.stringify({ rows, kept: keptList }, null, 1))
process.exit(failed.length ? 1 : 0)
