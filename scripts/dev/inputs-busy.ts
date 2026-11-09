// Known-answer fixture for `strobe query <rec> inputs` (dev only, not shipped): a recording whose inputs,
// changes and presents are planted, so each input's outcome, and each reason an input gave no sample
// (`BusyWhy`), is known by construction. It writes the recording into a state folder of its own (strobe's
// own is untouched), runs the query and checks every line:
//   bun scripts/dev/inputs-busy.ts [path/to/strobe]
// Display 60 Hz: the app must rest two refreshes (33.33 ms) before an input counts.
//   A +100   at rest, its frame at +20 ms                       timed 20 ms
//   B +400   its last frame ended 10 ms before it               mid-drawing, ago 10, needed 23.33
//   C +700   at rest, its frame begun after D, shown at +40     timed 40 ms
//   D +710   C's frame showed 30 ms after it                    behind C, needed 30 + 33.33 = 63.33
//   F +1000  at rest, its change late (known by +1205)          timed 15 ms
//   E +1200 press, +1250 release, its change between them while F's could still come: ambiguous press
//   G +1500  pixels proved between +1500 and +1560, two frames there: multi-frame pixel (2)
//   I +2200  a frame begun before it still on its way (+15 ms)  mid-drawing, ago -15, needed 48.33
//   H +2300  nothing changed after it                           unchanged
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const strobe = process.argv[2] ?? process.env.STROBE_BIN ?? "strobe"
const T = 10_000_000_000
const ms = (x: number) => T + Math.round(x * 1e6)

const present = (start: number, screen: number) => ({
  pid: 100, tid: 101, start_ns: ms(start), stop_ns: ms(start + 1), ready_ns: 0, screen_ns: ms(screen),
  swapchain: 1, sync_interval: 1, mode: "Composed_Flip", state: "displayed",
})
const presents = {
  presents: [
    present(108, 120), // A's
    present(370, 390), // ended 10 ms before B
    present(408, 425), // B's
    present(715, 740), // C's, begun after D
    present(732, 760), // D's
    present(1002, 1015), // F's
    present(1510, 1525), // G: one of two between its captures
    present(1530, 1542), // G: the other
    present(2190, 2215), // on its way at I
    present(2220, 2235), // I's
  ],
  vsyncs: [], events: 0, events_lost: 0, consumer_cpu_ms: null, t0_ns: ms(0), t1_ns: ms(2500),
}
const input = (at: number, extra: object = {}) => ({ t_ns: ms(at), kind: "discrete", first: false, ...extra })
const change = (lo: number, hi: number) => [ms(lo), ms(hi), false]
const frames = {
  pids: [100],
  display_hz: 60,
  runs: {
    "run:0": {
      t0_ns: ms(0), t1_ns: ms(2500), untimed: [], marks: [],
      inputs: [
        input(100), input(400), input(700), input(710), input(1000),
        input(1250, { press_ns: ms(1200) }),
        input(1500, { pixel: [ms(1500), ms(1560)] }),
        input(2200), input(2300),
        { t_ns: ms(50), kind: "pointer", first: false },
      ],
      changes: [change(105, 105), change(405, 405), change(701, 701), change(730, 730), change(1001, 1205), change(1210, 1220), change(2205, 2205)],
    },
  },
}

const home = mkdtempSync(join(tmpdir(), "strobe-inputs-"))
const id = "rec_inputsfixture"
const dir = join(home, "strobe", "recordings", id)
mkdirSync(dir, { recursive: true })
writeFileSync(join(dir, "meta.json"), JSON.stringify({ id, kind: "block", native: true, probes: "timing", runs: 1, measured: ["run:0"] }))
writeFileSync(join(dir, "frames.json"), JSON.stringify(frames))
writeFileSync(join(dir, "presents.json.gz"), Bun.gzipSync(new TextEncoder().encode(JSON.stringify(presents))))

const env = { ...process.env, LOCALAPPDATA: home, XDG_DATA_HOME: home }
const run = (args: string[]) => {
  const p = Bun.spawnSync([strobe, "query", id, "inputs", ...args], { env, stdout: "pipe", stderr: "pipe" })
  if (p.exitCode !== 0) throw new Error(`strobe query ${id} inputs ${args.join(" ")} exited ${p.exitCode}: ${p.stderr.toString()}`)
  return p.stdout.toString()
}

type Want = { at: number; outcome: string; latency?: number; why?: string; ago?: number; needed?: number | null; frames?: number; prev?: number }
const want: Want[] = [
  { at: 100, outcome: "timed", latency: 20 },
  { at: 400, outcome: "busy", why: "mid_drawing", ago: 10, needed: 23.333 },
  { at: 700, outcome: "timed", latency: 40 },
  { at: 710, outcome: "busy", why: "behind", prev: 700, needed: 63.333 },
  { at: 1000, outcome: "timed", latency: 15 },
  { at: 1250, outcome: "busy", why: "ambiguous_press", needed: null },
  { at: 1500, outcome: "busy", why: "multi_frame_pixel", frames: 2, needed: null },
  { at: 2200, outcome: "busy", why: "mid_drawing", ago: -15, needed: 48.333 },
  { at: 2300, outcome: "unchanged" },
]

let failed = 0
try {
  const out = JSON.parse(run(["--json"]))
  const rows: any[] = out.inputs ?? out.json?.inputs ?? []
  const near = (a: number | undefined | null, b: number) => typeof a === "number" && Math.abs(a - b) < 0.01
  for (const w of want) {
    const r = rows.find((x) => near(x.at_ms, w.at))
    const bad: string[] = []
    if (!r) bad.push("no line")
    else {
      if (r.outcome !== w.outcome) bad.push(`outcome ${r.outcome}`)
      if (w.latency !== undefined && !near(r.latency_ms, w.latency)) bad.push(`latency ${r.latency_ms}`)
      if (w.why !== undefined && r.busy?.why !== w.why) bad.push(`why ${r.busy?.why}`)
      if (w.ago !== undefined && !near(r.busy?.prev_frame_ago_ms, w.ago)) bad.push(`ago ${r.busy?.prev_frame_ago_ms}`)
      if (w.prev !== undefined && !near(r.busy?.prev_input_at_ms, w.prev)) bad.push(`prev ${r.busy?.prev_input_at_ms}`)
      if (w.frames !== undefined && r.busy?.frames !== w.frames) bad.push(`frames ${r.busy?.frames}`)
      if (w.needed === null && r.needed_ms !== null) bad.push(`needed ${r.needed_ms}`)
      if (typeof w.needed === "number" && !near(r.needed_ms, w.needed)) bad.push(`needed ${r.needed_ms}`)
    }
    if (bad.length) failed++
    console.log(`${bad.length ? "FAIL" : "ok  "} +${w.at} ms ${w.why ?? w.outcome}${bad.length ? ": " + bad.join(", ") : ""}`)
  }
  if (rows.length !== want.length) {
    failed++
    console.log(`FAIL ${rows.length} lines, want ${want.length} (the pointer move is no measured input)`)
  }
  console.log(run([]))
} finally {
  rmSync(home, { recursive: true, force: true })
}
console.log(failed ? `${failed} FAILED` : "all inputs as planted")
process.exit(failed ? 1 : 0)
