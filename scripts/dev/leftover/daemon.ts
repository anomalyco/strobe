// Run on a disposable host. This is a private daemon, not the installed one. Its usual
// stale-owner maintenance is machine-wide even with a private state folder.
import { strict as assert } from "node:assert"
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"

export async function daemon(exe: string, old = false) {
  const local = mkdtempSync(join(tmpdir(), "console-daemon-")), dir = join(local, "strobe")
  mkdirSync(dir)
  const listener = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } })
  const port = listener.port; listener.stop(true)
  writeFileSync(join(dir, "state.json"), JSON.stringify({ mcp_port: port }))
  const p = Bun.spawn([exe, "daemon"], { env: { ...process.env, LOCALAPPDATA: local }, stdin: "ignore", stdout: "ignore", stderr: "pipe", timeout: 90000 })
  try {
    const until = Date.now() + 10000
    while (!existsSync(join(dir, "mcp-token")) && p.exitCode === null && Date.now() < until) await Bun.sleep(50)
    const token = readFileSync(join(dir, "mcp-token"), "utf8").trim()
    const msg = { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "bench", arguments: { file: resolve(import.meta.dir, "console.bench.ts"), head: process.cwd(), check: true, power: "keep", "reserve-cpus": "off" } } }
    const reply = await fetch(`http://127.0.0.1:${port}/mcp`, { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: JSON.stringify(msg), signal: AbortSignal.timeout(60000) })
    assert.equal(reply.status, 200)
    const result = (await reply.json()).result
    const id = JSON.stringify(result).match(/\bexp_[a-z0-9]+\b/)?.[0]
    assert.ok(id, result?.content?.[0]?.text)
    const file = join(dir, "experiments", id, "result.json"), done = Date.now() + 60000
    while (!existsSync(file) && Date.now() < done) await Bun.sleep(50)
    const row = JSON.parse(readFileSync(file, "utf8")).series[0]
    console.log(`daemon: ${row.passed ? "pass" : row.error}`)
    assert.ok(row.passed || old && /still running: conhost\.exe \(pid \d+\)/i.test(row.error))
  } finally {
    if (p.exitCode === null) p.kill()
    await p.exited
  }
}

if (import.meta.main) await daemon(process.argv[2])
