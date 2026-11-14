import { bench, script } from "@hona/strobe"

export default bench({
  targets: { worker: script("./worker.ts") },
  cases: {
    ok: { primary: "count.items", run: ({ count }) => count("items", 3) },
    secondary: { primary: "count.items", metrics: ["frames.dropped.pct"], run: ({ count }) => count("items", 3) },
    worse: { primary: "count.items", run: ({ variant, count }) => count("items", variant === "base" ? 1 : 4) },
    basefail: { primary: "count.items", run: ({ variant, count }) => { if (variant === "base") throw new Error("base failed its run"); count("items", 1) } },
    defect: { primary: "cpu.work", run: () => { throw new Error("fixture incomplete") } },
    phase: { primary: "phase.absent", run: ({ call }) => call("idle") },
    frames: { primary: "frames.dropped.pct", run: ({ call }) => call("idle") },
    gap: { primary: "cpu.work", unsupported: { worker: "no adapter" }, run: () => {} },
  },
})
