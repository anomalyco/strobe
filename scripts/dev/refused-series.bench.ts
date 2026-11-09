// Known-answer bench for a series that refuses as it runs (dev only, not shipped): the refusal ends that
// series alone, and the rest of the file runs on. Run from the repo root (timed, noisy machines are fine):
//   strobe bench scripts/dev/refused-series.bench.ts --head . --blocks 1
//   strobe bench scripts/dev/refused-series.bench.ts --head . --blocks 1 --filter no-phase   (every series refused: exit 4)
//   strobe bench scripts/dev/refused-series.bench.ts --head . --blocks 2 --filter late-refusal,spin-10
//   REFUSE_AFTER=4 strobe bench scripts/dev/refused-series.bench.ts --aa --filter late-refusal,spin-10
//   strobe bench scripts/dev/refused-series.bench.ts --head . --check --runs 3 --filter spin-20
//   strobe bench scripts/dev/refused-series.bench.ts --head . --check --repeat 3 --filter spin-20
// Truth:
//   no-phase      its primary is a phase its runs never enter: refused NoPhase in its first block (exit 4 alone)
//   late-refusal  enters its phase in the first REFUSE_AFTER launches only (default 2: --blocks' discarded warm-up
//                 block and block 1; 4 for --aa: both warm-up blocks and pair 1), then refuses NoPhase: the series
//                 ends refused with its fix and no numbers, the blocks it finished kept, and the rest runs on
//   spin-20       the main thread busy for 20 ms a run: cpu.work ~20 ms, run.duration ~20 ms
//   spin-10       the same for 10 ms: cpu.work ~10 ms, after a refused series
import { bench, script } from "@hona/strobe"

const spin = (ms: number) => ({ targets: ["harness"], primary: "cpu.work", metrics: ["run.duration"], run: ({ call }: any) => call("mainSpin", ms) })
// late-refusal's launches so far (its setup runs once per launch, in this process across the bench).
let launches = 0
const refuseAfter = Number(process.env.REFUSE_AFTER ?? 2)

export default bench({
  setup: ({ case: name }: any) => {
    if (name === "late-refusal") launches += 1
  },
  targets: { harness: script("./main-thread-harness.ts") },
  cases: {
    "no-phase": { targets: ["harness"], primary: "phase.never", metrics: ["run.duration"], run: ({ call }: any) => call("mainSpin", 20) },
    "late-refusal": {
      targets: ["harness"],
      primary: "phase.spin",
      metrics: ["run.duration"],
      run: ({ call, phase }: any) => (launches <= refuseAfter ? phase("spin", () => call("mainSpin", 20)) : call("mainSpin", 20)),
    },
    "spin-20": spin(20),
    "spin-10": spin(10),
  },
})
