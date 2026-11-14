import { bench, cli } from "@hona/strobe"

export default bench({
  targets: { tool: cli('bash -c"echo $HOME"') },
  cases: { line: { primary: "cpu.work", run: ({exec}) => exec() } },
})
