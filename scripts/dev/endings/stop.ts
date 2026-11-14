// Stop only the unique check started here, by experiment ID, after its header.
import assert from "node:assert/strict"
const exe = process.env.STROBE_BIN ?? "strobe"
const id = `exp_end_${crypto.randomUUID()}`
const proc = Bun.spawn([exe,"bench","scripts/dev/endings/rows.bench.ts","--id",id,"--head",".","--check","--filter","ok,slow","--json"],{stdout:"pipe",stderr:"pipe",timeout:60000})
const result = new Response(proc.stdout).text()
let log = "", stopped = false
for await (const chunk of proc.stderr) {
  log += new TextDecoder().decode(chunk)
  if (!stopped && log.includes(id)) {
    const request = Bun.spawnSync([exe,"bench","--stop",id], {timeout:10000})
    assert.equal(request.exitCode,0,request.stderr.toString())
    stopped=true
  }
}
assert.ok(stopped)
assert.equal(await proc.exited,130)
const r=JSON.parse(await result)
assert.equal(r.passed,false)
assert.equal(r.stopped.code,130)
assert.match(r.text,/stopped/)
const step=Bun.spawnSync([exe,"report","experiment",id,"--format","summary"],{timeout:20000})
assert.match(step.stdout.toString(),/\*\*stopped\*\* \(exit 130\)/)
console.log("stop: exit 130; result and step summary stopped")
