// Both cases must refuse before this nonexistent executable could start.
import { bench, native } from "@hona/strobe"
export default bench({
  targets: { app: native("./unused-launch-fixture", { window: false }) },
  cases: {
    visible: { primary: "launch.ready", launch: { until: { window: "visible" } } },
    gpu: { primary: "gpu.process.3d", launch: { until: { milestone: "ready" }, milestones: { ready: { match: "READY" } } } },
  },
})
