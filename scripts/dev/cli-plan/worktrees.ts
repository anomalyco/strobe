// Two real worktrees. The package script supplies PATH; neither variant overrides it in its target.
import { spawnSync } from "node:child_process"
import { mkdirSync, writeFileSync, copyFileSync } from "node:fs"
import { join } from "node:path"

export function worktrees(dir:string,bin:string,env:NodeJS.ProcessEnv) {
  const head=join(dir,"head"),base=join(dir,"base"),win=process.platform==="win32"
  mkdirSync(head)
  const go=(cwd:string,exe:string,args:string[],code=0)=>{
    const r=spawnSync(exe,args,{cwd,env,encoding:"utf8",windowsHide:true,timeout:135000,maxBuffer:16<<20})
    if(r.status!==code)throw Error(`${args[0]}: expected ${code}, got ${r.status}: ${r.stderr} ${r.stdout.slice(-500)}`)
    return r.stdout
  }
  const git=(...args:string[])=>go(head,"git",args)
  git("init","-q")
  const shim=win?"plan-leg.cmd":"plan-leg", local=`./node_modules/.bin/${shim}`
  const bench=(line:string)=>`import {bench,cli} from '@hona/strobe'; export default bench({targets:{line:cli(${JSON.stringify(line)})},cases:{work:{primary:'run.duration',metrics:['metric.plant'],run:ctx=>ctx.exec()}}})\n`
  mkdirSync(join(head,"perf"));writeFileSync(join(head,"perf/work.bench.ts"),bench("plan-leg"))
  writeFileSync(join(head,"package.json"),JSON.stringify({scripts:{measure:`"${bin.replaceAll("\\","/")}" bench perf/work.bench.ts --check --base "${base.replaceAll("\\","/")}" --head . --power keep --reserve-cpus off --max-time 2m --json`}}))
  git("add","package.json","perf/work.bench.ts");git("-c","user.name=fixture","-c","user.email=fixture@invalid","commit","-qm","fixture");git("worktree","add","--detach",base,"HEAD")
  for(const [folder,ms] of [[head,101],[base,37]] as const){
    const path=join(folder,"node_modules/.bin");mkdirSync(path,{recursive:true});writeFileSync(join(path,shim),(win?"@":"#!/bin/sh\nexec ")+`bun -e "Bun.sleepSync(${ms});console.log('strobe:metric plant=${ms}')"\n`,{mode:0o755})
  }
  const sample=(name:string,line:string,expected:number,shadow:boolean)=>{
    for(const folder of [head,base])writeFileSync(join(folder,"perf/work.bench.ts"),bench(line))
    const r=JSON.parse(go(head,process.execPath,["run","--silent","measure"])),s=r.series[0]
    const warned=r.warnings.some((w:any)=>w.id==="PathShadow" && w.text.includes("./node_modules/.bin/plan-leg"))
    if(warned!==shadow || s.base_values["metric.plant"]!==expected || s.values["metric.plant"]!==101 || s.base_values["run.duration"]<expected-2 || s.values["run.duration"]<99)throw Error(`${name}: plant or PATH warning did not match: ${JSON.stringify(r.warnings)}`)
    console.log(`ok ${name}: planted base ${expected}, head 101 ms; wall ${s.base_values["run.duration"].toFixed(1)} / ${s.values["run.duration"].toFixed(1)}; PathShadow ${warned}`)
  }
  sample("package-path","plan-leg",101,true);sample("variant-shim",local,37,false)
  if(win){
    mkdirSync(join(head,"target/release"),{recursive:true});copyFileSync(process.execPath,join(head,"target/release/app.exe"));copyFileSync(process.execPath,join(head,"tool.exe"))
    for(const line of ["./tool --version",".\\tool.exe --version","./target/release/app.exe --version",".\\target\\release\\app.exe --version > NUL",`.\\node_modules\\.bin\\${shim}`]){
      writeFileSync(join(head,"perf/work.bench.ts"),bench(line).replace("metrics:['metric.plant']","metrics:[]"));go(head,bin,["bench","perf/work.bench.ts","--head",".","--check","--power","keep","--reserve-cpus","off"]); console.log(`ok directory variant: ${line}`)
    }
  }
}
