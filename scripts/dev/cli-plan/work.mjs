// Each completed leg leaves an independent receipt outside stdout (including redirected/piped legs).
import { appendFileSync, readFileSync } from "node:fs"
const [key, ...args] = process.argv.slice(2)
const input = key === "reader" ? readFileSync(0, "utf8").trim() : undefined
const cpu = () => { const { user, system } = process.cpuUsage(); return (user + system) / 1000 }
const start = cpu()
if (key === "right") while (cpu() - start < 25) Math.sqrt(Math.random())
await Bun.sleep(key === "right" ? 110 : 35)
appendFileSync(process.env.PLAN_RECEIPTS, JSON.stringify({ key, args, pid: process.pid, parent: process.ppid, cpu: cpu() - start, input, x: process.env.PLAN_X }) + "\n")
console.log(key)
if (key === "fail") process.exitCode = 3
