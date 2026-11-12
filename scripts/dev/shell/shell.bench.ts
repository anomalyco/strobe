// Runs the command-line meaning's table (rows.ts; `cli::plan`) as cli() cases, dev only, not shipped: each row's line,
// failing when its programs printed other than the row says, or when a row that should be refused was not (or was
// refused for another reason). Whether the shell ran a line is strobe's `ShellLine` note on its series and whether a
// direct Windows line was noted is `DirectLine`: check.ts compares both with the rows. Rows whose program is not on
// PATH are left out. Needs bun on PATH. From the repo root:
//   bun scripts/dev/shell/check.ts            (or: strobe bench scripts/dev/shell/shell.bench.ts --head . --check)
import { bench, cli } from "@hona/strobe"
import { delimiter, join } from "node:path"
import { rows, type Row } from "./rows.ts"

// The URL rows' server: it answers with the query it was sent.
const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (r) => new Response("query " + new URL(r.url).search) })
const fill = (line: string) => line.replaceAll("{port}", String(server.port))
// The .cmd and sh shims a `tsc` or `pnpm` row runs, found through PATH as an installed tool is.
const bin = join(import.meta.dir, "bin")

const matches = (got: string[], want: Row["out"] = []) =>
  got.length === want.length && want.every((w, i) => (typeof w === "string" ? got[i] === w : w.test(got[i])))
const printed = (output: unknown) =>
  String(output)
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith("strobe:"))

export const usable = Object.entries(rows).filter(([, r]) => !r.needs || Bun.which(r.needs))

export default bench({
  targets: { sh: cli(undefined, { env: { PATH: bin + delimiter + (process.env.PATH ?? ""), STROBE_SHELL_X: "xval" } }) },
  cases: Object.fromEntries(
    usable.map(([name, row]) => [
      name,
      {
        primary: "run.duration",
        resetCheck: false,
        run: async ({ exec }: any) => {
          const line = fill(row.line)
          if (row.outcome === "refused") {
            let error = ""
            try {
              await exec(line)
            } catch (e) {
              error = String((e as Error)?.message ?? e)
            }
            if (!error) throw new Error(`${name}: \`${line}\` ran; it should be refused (${row.refusal})`)
            if (!error.includes(row.refusal ?? "")) throw new Error(`${name}: \`${line}\` failed with ${error}, not the refusal ${row.refusal}`)
            // A run times a command: one plain line after the refusal.
            await exec("bun scripts/dev/shell/argv.mjs refused")
            return
          }
          const { output } = await exec(line)
          const got = printed(output)
          if (!matches(got, row.out)) throw new Error(`${name}: \`${line}\` printed ${JSON.stringify(got)}, not ${JSON.stringify((row.out ?? []).map(String))}`)
        },
      },
    ]),
  ),
})
