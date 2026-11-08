// A throwaway git repository whose bench primary drifts a little at every commit (dev, not shipped):
// the known answer for `--bisect` when a change built up over many commits and no one commit did it.
//
//   bun scripts/dev/drift-repo.ts <dir> [--commits 20] [--step 0.004] [--noise 0.01]
//   strobe bench <dir>/drift.bench.ts --bisect c0..c20 --pairs 10 --runs 1
//
// Commit ck holds level.txt = 100 × (1 + step)^k. print.ts prints `strobe:metric score=<level × (1 +
// noise × z)>` with z a seeded normal draw per block (STROBE_HOME tells blocks apart; the draws sit
// in <dir>.noise.json, outside the repository, so every checkout shares them), the same for every run
// of a block. Each commit moves the score by `step` (under the 2% bound, inside the noise); twenty
// move it by 8%. Tags c0…cN name the commits.
import { execFileSync } from "node:child_process"
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"

const args = process.argv.slice(2)
const dir = resolve(args[0] ?? "")
const opt = (name: string, d: number) => {
  const i = args.indexOf(`--${name}`)
  return i >= 0 ? Number(args[i + 1]) : d
}
const commits = opt("commits", 20), step = opt("step", 0.004), noise = opt("noise", 0.01)
rmSync(dir, { recursive: true, force: true })
rmSync(`${dir}.noise.json`, { force: true })
mkdirSync(dir, { recursive: true })
const git = (...a: string[]) => execFileSync("git", ["-C", dir, ...a], { stdio: "pipe" }).toString()
git("init", "-q", "-b", "main")
git("config", "user.email", "drift@example.invalid")
git("config", "user.name", "drift")
const state = `${dir}.noise.json`.replaceAll("\\", "/")
writeFileSync(
  `${dir}/print.ts`,
  `import { existsSync, readFileSync, writeFileSync } from "node:fs"
const level = Number(readFileSync("level.txt", "utf8"))
const file = ${JSON.stringify(state)}
const s = existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : { homes: [] }
const home = process.env.STROBE_HOME ?? "none"
let i = s.homes.indexOf(home)
if (i < 0) { s.homes.push(home); i = s.homes.length - 1; writeFileSync(file, JSON.stringify(s)) }
// A seeded normal draw for block i (Box–Muller on an LCG), the same on every run of the block.
let x = 2463534242 + i * 2654435761
const u = () => ((x = (Math.imul(x, 1103515245) + 12345) >>> 0) + 0.5) / 2 ** 32
for (let k = 0; k < 3; k++) u()
const z = Math.sqrt(-2 * Math.log(u())) * Math.cos(2 * Math.PI * u())
console.log("strobe:metric score=" + (level * (1 + ${noise} * z)).toFixed(4))
`,
)
writeFileSync(
  `${dir}/drift.bench.ts`,
  `import { bench, cli } from "@hona/strobe"

export default bench({
  targets: { app: cli("bun print.ts") },
  cases: { score: { targets: ["app"], primary: "metric.score", run: (ctx) => ctx.exec() } },
})
`,
)
writeFileSync(`${dir}/strobe.json`, "{}\n")
for (let k = 0; k <= commits; k++) {
  writeFileSync(`${dir}/level.txt`, (100 * (1 + step) ** k).toFixed(4))
  git("add", "-A")
  git("commit", "-q", "-m", `c${k}: level ${(100 * (1 + step) ** k).toFixed(2)}`)
  git("tag", `c${k}`)
}
console.log(`${dir}: ${commits + 1} commits, ${(step * 100).toFixed(2)}% a commit (${(((1 + step) ** commits - 1) * 100).toFixed(1)}% over the range), noise ${(noise * 100).toFixed(1)}% a block`)
