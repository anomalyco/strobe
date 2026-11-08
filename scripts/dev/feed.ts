// A cli() command that prints known values, one per block (dev, not shipped): every run of a block
// prints the same `strobe:metric` lines, the block's next value from a sequence file, so a bench's
// verdicts can be checked against numbers fixed in advance.
//
//   strobe run --base "bun scripts/dev/feed.ts base seq.json" --head "bun scripts/dev/feed.ts head seq.json" \
//     --primary metric.score --metrics metric.other --pairs 10 --runs 1
//
// seq.json: { "base": { "score": [..], "other": [..] }, "head": { … }, "direction": { "score": "higher" } }.
// A value of null prints no line for that metric in that block (strobe refuses it NoMetric).
// Blocks are told apart by STROBE_HOME, which strobe stamps per block; the state sits beside seq.json.
import { existsSync, readFileSync, writeFileSync } from "node:fs"

const [side, file] = process.argv.slice(2)
const seq = JSON.parse(readFileSync(file, "utf8"))
const stateFile = `${file}.state.json`
const state: Record<string, string[]> = existsSync(stateFile) ? JSON.parse(readFileSync(stateFile, "utf8")) : {}
const home = process.env.STROBE_HOME ?? "none"
const seen = (state[side] ??= [])
let block = seen.indexOf(home)
if (block < 0) {
  seen.push(home)
  block = seen.length - 1
  writeFileSync(stateFile, JSON.stringify(state))
}
for (const [name, values] of Object.entries(seq[side] as Record<string, (number | null)[]>)) {
  const v = values[block % values.length]
  if (v === null) continue
  const dir = seq.direction?.[name] ? ` ${seq.direction[name]}` : ""
  console.log(`strobe:metric ${name}=${v}${dir}`)
}
