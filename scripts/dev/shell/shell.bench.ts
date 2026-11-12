// Runs the shell rule's table (rows.ts; `cli::shell_line`) as cli() cases, dev only, not shipped: each row's line, failing
// when its programs printed other than the row says. Whether the shell ran it is strobe's `ShellLine` note on the case's
// series, which check.ts compares with the row. Needs bun on PATH. From the repo root:
//   bun scripts/dev/shell/check.ts            (or: strobe bench scripts/dev/shell/shell.bench.ts --head . --check)
import { bench, cli } from "@hona/strobe"
import { rows, type Row } from "./rows.ts"

const matches = (got: string[], want: Row["out"]) =>
  got.length === want.length && want.every((w, i) => (typeof w === "string" ? got[i] === w : w.test(got[i])))

export default bench({
  targets: { sh: cli() },
  cases: Object.fromEntries(
    Object.entries(rows).map(([name, row]) => [
      name,
      {
        primary: "run.duration",
        resetCheck: false,
        run: async ({ exec }: any) => {
          const { output } = await exec(row.line)
          const got = String(output)
            .split(/\r?\n/)
            .map((l) => l.trim())
            .filter((l) => l && !l.startsWith("strobe:"))
          if (!matches(got, row.out)) throw new Error(`${name}: \`${row.line}\` printed ${JSON.stringify(got)}, not ${JSON.stringify(row.out.map(String))}`)
        },
      },
    ]),
  ),
})
