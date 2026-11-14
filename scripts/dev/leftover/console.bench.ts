import { bench, cli } from "@hona/strobe"

export default bench({ targets: { tool: cli("bun -e 0") }, cases: { console: { primary: "cpu.work", run: ({ exec }) => exec() } } })
