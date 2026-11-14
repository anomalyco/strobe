// In a disposable checkout, apply dark-block.patch and rebuild. It substitutes only
// the final display reading, after a real headless block collected its primary.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

const exe = process.env.STROBE_BIN ?? "strobe"
for (const [filter, code] of [["phases", 4], ["spin100,phases", 0]] as const) {
  const run = (args: string[]) => Bun.spawnSync([exe, ...args], { timeout: 120000 })
  const p = run(["bench", "scripts/dev/known-web.bench.ts", "--head", ".", "--filter", filter, "--check", "--headless", "--json"])
  const r = JSON.parse(p.stdout.toString())
  assert.equal(p.exitCode, code, r.error)
  assert.deepEqual(JSON.parse(readFileSync(r.result, "utf8")).series, r.series)
  assert.equal(r.passed, code === 0)
  const dark = r.series.find((s: any) => s.case === "phases")
  assert.deepEqual([dark.state, dark.passed, dark.decided, dark.refusal.validator], ["refused",false,false,"Throttled"])
  assert.equal(dark.values?.["phase.b"], undefined)
  assert.match(r.text, /^refused\s+phases\b.*Throttled/m)
  assert.ok(r.text.includes(code ? "nothing checked" : "1 of 1 cases passed, 1 refused"))
  const pr = run(["report", "experiment", r.id, "--format", "summary"])
  assert.equal(pr.exitCode, 0)
  assert.match(pr.stdout.toString(), /\| phases \|[^\n]+\| refused \|/)
  console.log(filter, dark.state, dark.refusal.validator, r.passed, p.exitCode)
}
