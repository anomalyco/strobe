// Exercise the shipped command planner with an empty history and a prior with known spread.
// Neither side should execute. The temporary commands deliberately do not exist.
import assert from "node:assert/strict"
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const home = mkdtempSync(join(tmpdir(), "pair-plans-"))
const env = { ...process.env, LOCALAPPDATA: home, XDG_DATA_HOME: home, HOME: home }
const bin = process.env.STROBE_BIN ?? "strobe"
const plan = (n: string) => {
  const p = Bun.spawnSync([bin, "run", "--plan", "--aa", "--head", "future-command", "--pairs", n, "--json"], { env, timeout: 15000 })
  assert.equal(p.exitCode, 0, p.stdout.toString())
  const r = JSON.parse(p.stdout.toString())
  const line = r.text.split("\n").find((l: string) => l.startsWith("  expect "))
  console.log(line)
  return { s: r.series[0], line }
}
assert.match(plan("6").line, /6 kept pairs \(12 launches before retries\).*--pairs 8/)
assert.match(plan("4").line, /cannot give a 95% interval/)
const { s, line } = plan("auto")
assert.match(line, /estimated 8 kept pairs \(16 launches before retries\); spread unknown/)
const experiments = join(home, process.platform === "darwin" ? "Library/Application Support/strobe" : "strobe", "experiments")
const id = "exp_prior"
mkdirSync(join(experiments, id), { recursive: true })
const primary = "cpu.work"
const blocks = [0.96, 1.04, 0.96, 1.04].flatMap((ratio, pair) => ["base", "head"].map(variant => ({
  pair, variant, values: { [primary]: variant === "base" ? 100 : ratio * 100 }, steps: [["run", 1000]],
})))
writeFileSync(join(experiments, "index.jsonl"), JSON.stringify({ id, file: "run.bench.ts", verdicts: [{ series:s.label, primary }] }) + "\n")
writeFileSync(join(experiments, id, "result.json"), JSON.stringify({ variants:[{spec:"cmd:future-command"}], series:[{label:s.label, primary:{ metric:primary, relative:true, estimate:0 }, blocks}] }))
const noisy = plan("9")
assert.ok(noisy.s.underpowered && noisy.s.pairs_needed > 9)
assert.ok(noisy.line.includes(`--pairs ${noisy.s.pairs_needed} for an 80% chance`))
