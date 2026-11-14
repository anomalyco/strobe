import { bench, cli } from "@hona/strobe"

export default bench({
  targets: { tool: cli("bun -v & bun -v") },
  cases: { line: { primary: "cpu.work", run: ({exec}) => exec() } },
})
