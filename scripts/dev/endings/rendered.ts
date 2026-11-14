import assert from "node:assert/strict"
import { existsSync, mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { pathToFileURL } from "node:url"

export function rendered(exe:string,id:string,env=process.env) {
  const dir=mkdtempSync(join(tmpdir(),"ending-report-")), file=join(dir,"report.html")
  const report=Bun.spawnSync([exe,"report","experiment",id,"--out",file],{env,timeout:60000})
  assert.equal(report.exitCode,0,report.stderr.toString())
  const chrome=[process.env.STROBE_BROWSER,Bun.which("chromium"),Bun.which("google-chrome"),join(process.env["ProgramFiles(x86)"]??"", "Microsoft/Edge/Application/msedge.exe")].find(p=>p && existsSync(p))
  assert.ok(chrome,"a headless Chromium browser is required")
  const dom=Bun.spawnSync([chrome,"--headless","--no-sandbox","--disable-gpu","--disable-background-networking",`--user-data-dir=${join(dir,"profile")}`,"--virtual-time-budget=3000","--dump-dom",pathToFileURL(file).href],{timeout:60000,stdout:"pipe",stderr:"pipe"})
  assert.equal(dom.exitCode,0,dom.stderr.toString())
  return dom.stdout.toString().replace(/<(script|style)\b[^>]*>[\s\S]*?<\/\1>/gi,"").replace(/<[^>]*>/g," ").replace(/\s+/g," ")
}
