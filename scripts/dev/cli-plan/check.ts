// From the repo root: STROBE_BIN=<binary> bun scripts/dev/cli-plan/check.ts [--before].
import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, writeFileSync, rmSync, copyFileSync, mkdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, delimiter } from "node:path"
const before = process.argv.includes("--before"), win = process.platform === "win32"
const dir = mkdtempSync(join(tmpdir(), "command-plan-")), file = join(dir,"redirect"), receipts = join(dir,"receipts")
process.env.PLAN_FILE = file
const { rows } = await import("./rows.ts")
const bin = process.env.STROBE_BIN || "strobe", env = {...process.env, PATH: dir + delimiter + process.env.PATH, PLAN_X:"one", PLAN_RECEIPTS: receipts}
writeFileSync(join(dir, win ? "plan-shim.cmd" : "plan-shim"), (win ? "@" : "#!/bin/sh\nexec ") + "bun scripts/dev/cli-plan/work.mjs left\n", {mode:0o755})
let bad = 0
try {
  for (const row of rows) {
    // An old start command or file association can open a window. Do not exercise those paths.
    if (before && win && ["start", "ps1"].includes(row.key)) continue
    writeFileSync(receipts, ""); writeFileSync(file, row.key === "input" ? "left\n" : "not redirected\n")
    const r = spawnSync(bin, ["run", "--check", "--head", row.line, "--primary", "run.duration", "--metrics", "cpu.work", "--reserve-cpus", "off", "--power", "keep", "--json"], {encoding:"utf8", env, windowsHide:true, timeout:30000, maxBuffer:16<<20})
    let result: any; try { result = JSON.parse(r.stdout) } catch {}
    const series = result?.series?.[0], values = series?.values || {}, wall = values["run.duration"], cpu = values["cpu.work"]
    const legs = readFileSync(receipts,"utf8").trim().split("\n").filter(Boolean).map(l => JSON.parse(l))
    const keys = legs.map(l=>l.key).sort().join(" "), expected = row.legs?.split(" ").sort().join(" "), shell = result?.warnings?.some((w:any)=>w.id === "ShellLine")
    const min = row.key === "pipe" ? 135 : legs.reduce((sum,l)=>sum+(l.key === "right" ? 135 : 35),0)
    const text = r.stderr + r.stdout
    const ok = row.how === "refuse" ? r.status === 2 && text.includes(row.reason!) && !legs.length : row.how === "missing" ? r.status !== 0 && wall == null && (row.key !== "syntax" || /POSIX sh.*syntax error/i.test(series?.error || "")) :
      r.status === 0 && keys === expected && (row.how === "shell") === !!shell && typeof wall === "number" && wall + 2 >= min &&
      (legs.every(l=>l.key !== "right") || cpu >= 20 || JSON.stringify(series?.refused).includes("cpu.work")) &&
      (!row.args || JSON.stringify(legs[0]?.args) === JSON.stringify(row.args)) && (row.key !== "input" || legs[0]?.input === "left") &&
      (row.key !== "assignment" || legs[0]?.x === "two") && (row.key !== "redirect" || readFileSync(file,"utf8").trim() === "left")
    console.log(`${before ? "before" : ok ? "ok" : "BAD"} ${row.key}: exit ${r.status}, wall ${wall?.toFixed?.(1) ?? "—"}, CPU ${cpu?.toFixed?.(1) ?? "—"} ms, workers ${legs.map(l=>`${l.parent}>${l.pid}:${l.key}`).join(",") || "—"}`)
    if (!before && !ok) { bad++; console.log((series?.error || r.stderr || r.stdout || r.error?.message || "").slice(-800)) }
  }
  if (!before) {
    const check = (name: string, args: string[], code: number, pattern: RegExp, cwd = process.cwd()) => {
      const r = spawnSync(bin, [...args, "--check", "--reserve-cpus", "off", "--power", "keep"], {env, cwd, windowsHide:true, encoding:"utf8", timeout:30000})
      const ok = r.status === code && pattern.test(r.stdout + r.stderr)
      console.log(`${ok ? "ok" : "BAD"} ${name}: exit ${r.status}`)
      if (!ok) { bad++; console.log((r.stdout + r.stderr).slice(-800)) }
    }
    for (const [name, code, pattern] of [["default",0,/CommandPath/],["usage",2,/single `&`/],["leftover",1,/still running/]] as const)
      check(name, ["bench", "scripts/dev/cli-plan/hooks.bench.ts", "--head", ".", "--filter", name], code, pattern)
    const local = join(dir,"local"), exe = win ? ".exe" : ""
    mkdirSync(local); copyFileSync(process.execPath, join(local,"plan-only-local" + exe))
    check("only-local", ["run","--head","plan-only-local --version"], 2, /write `\.([/\\])plan-only-local/, local)
    copyFileSync(process.execPath, join(local,"plan-shadow" + exe)); copyFileSync(process.execPath, join(dir,"plan-shadow" + exe))
    check("shadow", ["run","--head","plan-shadow --version"], 0, /PathShadow/, local)
    check("asymmetric", ["run","--head","bun --version","--base","echo hi"], 0, /only base's run.duration/, local)
  }
} finally { rmSync(dir,{recursive:true,force:true}) }
process.exitCode = bad ? 1 : 0
