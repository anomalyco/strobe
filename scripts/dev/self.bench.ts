// Dev bench: strobe measuring its own commands as a consumer would (not shipped), windowless, so it runs
// on a locked session. Each variant is a folder holding a strobe executable (strobe.exe on Windows):
//   strobe bench scripts/dev/self.bench.ts --head <folder> --check
//   strobe bench scripts/dev/self.bench.ts --base <folder of a release> --head <folder of a build> --filter query-hotspots
// What the commands read comes from the environment (this device's own handles):
//   STROBE_SELF_REC   a large attribution recording (rec_…): query summary, hotspots, heap, memory
//   STROBE_SELF_EXP   an experiment (exp_…): report
// SPEC §19's budgets beside them: `status` ≤ 4 s (3 of them the idle-load sample, which --fast skips),
// the binary ≤ 25 MB; the daemon's idle cost is `strobe record pid:<daemon> --for 60s`, not a case here.
import { bench, cli } from "@hona/strobe"

const rec = process.env.STROBE_SELF_REC ?? ""
const exp = process.env.STROBE_SELF_EXP ?? ""
const page = import.meta.dir + "/pages/click.html"
const exe = process.platform === "win32" ? "strobe.exe" : "strobe"

const need = (name: string, value: string) => {
  if (!value) throw new Error(`set ${name} to a handle on this device`)
  return value
}

// The variant's own executable, by its absolute path: a bare name could resolve to the strobe running the bench.
const strobe = (root: string, args: string) => `"${root}/${exe}" ${args}`

const timed = { primary: "run.duration", metrics: ["cpu.work", "memory.peak", "io.read"] }

export default bench({
  targets: { strobe: cli() },
  cases: {
    help: { ...timed, run: (ctx) => ctx.exec(strobe(ctx.root, "--help")) },
    "status-fast": { ...timed, run: (ctx) => ctx.exec(strobe(ctx.root, "status --fast")) },
    "query-summary": { ...timed, timeout: 120_000, run: (ctx) => ctx.exec(strobe(ctx.root, `query ${need("STROBE_SELF_REC", rec)} summary`)) },
    "query-hotspots": { ...timed, run: (ctx) => ctx.exec(strobe(ctx.root, `query ${need("STROBE_SELF_REC", rec)} hotspots`)) },
    "query-heap": { ...timed, run: (ctx) => ctx.exec(strobe(ctx.root, `query ${need("STROBE_SELF_REC", rec)} heap`)) },
    "query-memory": { ...timed, run: (ctx) => ctx.exec(strobe(ctx.root, `query ${need("STROBE_SELF_REC", rec)} memory`)) },
    report: { ...timed, run: (ctx) => ctx.exec(strobe(ctx.root, `report experiment ${need("STROBE_SELF_EXP", exp)} --out "${ctx.root}/self-report.html"`)) },
    look: { ...timed, timeout: 120_000, run: (ctx) => ctx.exec(strobe(ctx.root, `look "${page}" --out "${ctx.root}/self-look.png"`)) },
  },
})
