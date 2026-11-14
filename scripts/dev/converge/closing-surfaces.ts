// Built background-dev and strobe binaries as arguments. Render the same closed row on every surface.
import assert from "node:assert/strict"
import { mkdtempSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { rendered } from "../endings/rendered"

const [harness, exe] = process.argv.slice(2)
const dir = mkdtempSync(join(tmpdir(), "closing-row-"))
const run = (bin: string, ...args: string[]) => {
  const p = Bun.spawnSync([bin, ...args], { timeout: 30000 })
  assert.equal(p.exitCode, 0, p.stderr.toString() || p.stdout.toString())
  return p.stdout.toString()
}
const result = JSON.parse(run(harness, "--closing-result"))
assert.equal(result.series[0].primary.needed_pairs, null)
for (const need of [null, 36, 40, 49]) {
  const r = structuredClone(result)
  r.series[0].primary.needed_pairs = need
  const file = join(dir, "result.json")
  writeFileSync(file, JSON.stringify(r))
  const card = JSON.parse(run(exe, "report", file, "--format", "card", "--out", join(dir, "card.svg"), "--json"))
  const surfaces = {
    terminal: run(exe, "query", file, "summary"),
    pr: run(exe, "report", file, "--format", "pr"),
    summary: run(exe, "report", file, "--format", "summary"),
    card: card.alt,
    html: rendered(exe, file),
  }
  for (const [surface, text] of Object.entries(surfaces)) {
    assert.match(text, /unclear|inconclusive|not decided|undecided/i, `${surface}: no inconclusive result: ${text}`)
    const hints = [...text.matchAll(/(?:--pairs\s+|~\s*(?=\d+\s+(?:kept )?pairs)|needs?\s+(?=\d+\s+pairs))(\d+)/gi)].map(m => Number(m[1]))
    assert.ok(hints.every(n => n > 40), `${surface}: stale pair hint ${hints}`)
    if (need === 49 && (surface === "pr" || surface === "html")) assert.ok(hints.includes(49), `${surface}: missing larger-pair control`)
    console.log(`needed_pairs=${need}, ${surface}: ${hints.length ? hints.join(", ") : "no pair hint"}`)
  }
}
