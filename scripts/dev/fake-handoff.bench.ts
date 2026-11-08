// Dev bench for a native app whose launcher hands over and exits at once (not shipped), windowless, so it runs
// on a locked session. From the repo root:
//   strobe bench scripts/dev/fake-handoff.bench.ts --head . --check
// Truth: the tray app outlives its launcher and is the app: `idle` measures it (cpu.work a few ms), never "the
// app exited"; `launch` is ready at its "fake ready" line, its cpu.work holding the launcher's (Linux: from the
// launch's cgroup; elsewhere refused when the launcher did not end cleanly first).
import { bench, native } from "@hona/strobe"

export default bench({
  targets: { app: native("fake", { args: [import.meta.dir + "/fake-handoff.ts"], window: false }) },
  cases: {
    idle: { primary: "cpu.work", run: ({ page }: any) => page.wait(500) },
    launch: { primary: "launch.ready", metrics: ["cpu.work"], launch: { until: { milestone: "up" }, milestones: { up: { match: "fake ready" } } } },
  },
})
