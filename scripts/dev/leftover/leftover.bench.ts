// The rows of rows.ts as cli() cases (dev only): run through check.ts, which states what each must do.
import { bench, cli } from "@hona/strobe"
import { delimiter, dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { rows } from "./rows.ts"

const bin = join(dirname(fileURLToPath(import.meta.url)), "bin")
const cases = Object.fromEntries(
  rows(process.env.LEFTOVER_CRATE).map((r) => [r.key, { primary: "run.duration", metrics: ["cpu.work"], resetCheck: false, timeout: 120_000, run: ({ exec }: any) => exec(r.line) }]),
)

export default bench({ targets: { line: cli(undefined, { env: { PATH: `${bin}${delimiter}${process.env.PATH ?? ""}` } }) }, cases })
