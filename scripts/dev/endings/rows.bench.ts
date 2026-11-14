import { bench, script } from "@hona/strobe"

export default bench({
  targets: { worker: script("./worker.ts") },
  cases: {
    ok: { primary: "count.items", run: ({ count }) => count("items", 3) },
    defect: { primary: "cpu.work", run: () => { throw new Error("fixture incomplete") } },
    phase: { primary: "phase.absent", run: ({ call }) => call("idle") },
    frames: { primary: "frames.dropped.pct", run: () => { throw new Error("must not launch") } },
    gap: { primary: "cpu.work", unsupported: { worker: "no adapter" }, run: () => {} },
  },
})
