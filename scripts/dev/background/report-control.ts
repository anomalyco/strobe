// Manual calibration: compare the same CLI and fixture with and without ONE report pass.
// No skip switch in strobe. The control is built only in a disposable source copy. Run with
// the usual build gate/priority and an explicit CARGO_TARGET_DIR. The control has its own
// build directory so it cannot replace a normal strobe binary or share its incremental state.
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { execFileSync } from 'node:child_process'

const root = resolve(import.meta.dir, '../../..')
const target = process.env.CARGO_TARGET_DIR
if (!target) throw Error('set CARGO_TARGET_DIR for the two builds')
const home = mkdtempSync(join(tmpdir(), 'report-control-'))
const src = join(home, 'source')
const artifacts = join(target,'report-control')
const exe=join(artifacts,'debug',process.platform==='win32'?'strobe.exe':'strobe')
function build() {
  execFileSync('cargo', ['build', '--offline', '--bin', 'strobe'], { cwd:src, timeout: 900000, stdio: 'inherit', windowsHide:true,
    env:{...process.env,CARGO_TARGET_DIR:artifacts, GIT_DIR:execFileSync('git',['-C',root,'rev-parse','--absolute-git-dir'],{encoding:'utf8'}).trim(),GIT_WORK_TREE:root} })
}
try {
  for (const f of ['Cargo.toml','Cargo.lock','build.rs','src','assets','sdk/bench/src','viewer','skill','calibration','.cargo']) {
    if (existsSync(join(root,f))) cpSync(join(root,f),join(src,f),{recursive:true})
  }
  writeFileSync(join(src,'src/app/report/device_ids.rs'),'pub(super) fn omit_device_ids(_: &mut serde_json::Value) {}\n')
  const words = 'mobile serial device on 1 a emulator-5554'
  const input = join(home,'result.json')
  writeFileSync(input,JSON.stringify({id:'exp_control',kind:'experiment',plan:{runs_per_block:1},variants:[{role:'base',label:'on'},{role:'head',label:'a'}],
    warnings:[{id:'Background',level:'info',text:`ordinary text: ${words}, run.duration, regression`}],
    series:[{kind:'runs',case:words,target:words,label:words,params:{mobile:'on',serial:'1',device:'a',udid:'emulator-5554',hardware_serial:'regression'},pairs:6,launches:12,
      primary:{metric:'run.duration',verdict:'regression',verdict_kind:'regression'},blocks:[{variant:'base',pair:0,values:{'metric.mobile.serial.device.on.1.a':1}},{variant:'head',pair:0,values:{'metric.mobile.serial.device.on.1.a':2}}]}]}))
  function reports() {
    return ['html','pr','summary','md','card','badge'].map(format=>{
      const out=join(home, ['card','badge'].includes(format)?'image.svg':'report.txt')
      execFileSync(exe,['report',input,'--format',format,'--out',out],{windowsHide:true,timeout:30000,env:{...process.env,LOCALAPPDATA:home,XDG_DATA_HOME:home},stdio:'pipe'})
      return {format,bytes:readFileSync(out)}
    })
  }
  build()
  const baseline=reports()
  // Prove which binary ran on each side; never accept comparing the same cached binary twice.
  const nonPhone=readFileSync(input)
  const probe=JSON.parse(nonPhone.toString()); probe.phone={serial:'control-probe-92dc'}
  const probeRendered=()=>{
    writeFileSync(input,JSON.stringify(probe)); const out=join(home,'probe.html');
    execFileSync(exe,['report',input,'--format','html','--out',out],{stdio:'pipe',windowsHide:true,timeout:30000});
    writeFileSync(input,nonPhone); return readFileSync(out).includes('control-probe-92dc')
  }
  if(!probeRendered()) throw Error('expected the control to retain the planted ID')
  // Write fresh bytes rather than copying the file's older source timestamp: cargo must
  // invalidate the no-op object it compiled a moment ago.
  writeFileSync(join(src,'src/app/report/device_ids.rs'),readFileSync(join(root,'src/app/report/device_ids.rs')))
  build()
  if(probeRendered()) throw Error('expected the candidate to omit the planted ID')
  for (const [i,{format,bytes}] of reports().entries()) {
    if (!bytes.length || !baseline[i].bytes.equals(bytes)) throw Error(`${format}: output differs from control`)
    if (format==='html' && !bytes.includes('metric.mobile.serial.device.on.1.a')) throw Error('missing viewer payload')
    console.log(`${format}: ${bytes.length} bytes identical`)
  }
} finally { rmSync(home,{recursive:true,force:true}) }
