// Dev tool (not shipped): strobe's own cost from outside, with none of strobe's code (Windows). Run a strobe
// command under it to see what strobe and its helpers used while it worked:
//   bun scripts/dev/tree-sampler.ts <out-dir> [--also <pid>,<pid>] [--cpus <hex mask>] -- strobe bench … --blocks 3
// Spawns the command, then moves itself to --cpus (default: where it was; keep it off the target's) and samples
// until the command exits:
// - every 2 ms (high-resolution waitable timer, no change to the system timer): QPC and the cycle counters
//   (QueryProcessCycleTime) of the root and its non-target children (bun runner, trace_processor, ffmpeg, strobe);
// - every 100 ms: one NtQuerySystemInformation(SystemProcessInformation) snapshot: every process of the tree
//   (and --also pids) with its cycle time, private bytes, IO transfer counts, and per thread its context switches;
//   threads of the root and the runner also with their own cycle counters and names.
// Output: fast.bin (Float64 rows: t_ns, pid, cycles), snap.jsonl, cmd.out/cmd.err, meta.json (cycles per ns).
import { dlopen, FFIType, ptr, toArrayBuffer } from "bun:ffi"
import { mkdirSync, writeFileSync, appendFileSync, openSync, writeSync, closeSync } from "fs"

const k32 = dlopen("kernel32.dll", {
  QueryPerformanceCounter: { args: [FFIType.ptr], returns: FFIType.i32 },
  QueryPerformanceFrequency: { args: [FFIType.ptr], returns: FFIType.i32 },
  OpenProcess: { args: [FFIType.u32, FFIType.i32, FFIType.u32], returns: FFIType.ptr },
  OpenThread: { args: [FFIType.u32, FFIType.i32, FFIType.u32], returns: FFIType.ptr },
  CloseHandle: { args: [FFIType.ptr], returns: FFIType.i32 },
  QueryProcessCycleTime: { args: [FFIType.ptr, FFIType.ptr], returns: FFIType.i32 },
  QueryThreadCycleTime: { args: [FFIType.ptr, FFIType.ptr], returns: FFIType.i32 },
  GetCurrentProcess: { args: [], returns: FFIType.ptr },
  GetCurrentThread: { args: [], returns: FFIType.ptr },
  SetProcessAffinityMask: { args: [FFIType.ptr, FFIType.u64], returns: FFIType.i32 },
  CreateWaitableTimerExW: { args: [FFIType.ptr, FFIType.ptr, FFIType.u32, FFIType.u32], returns: FFIType.ptr },
  SetWaitableTimer: { args: [FFIType.ptr, FFIType.ptr, FFIType.i32, FFIType.ptr, FFIType.ptr, FFIType.i32], returns: FFIType.i32 },
  WaitForSingleObject: { args: [FFIType.ptr, FFIType.u32], returns: FFIType.u32 },
  GetThreadDescription: { args: [FFIType.ptr, FFIType.ptr], returns: FFIType.i32 },
  LocalFree: { args: [FFIType.ptr], returns: FFIType.ptr },
}).symbols
const nt = dlopen("ntdll.dll", {
  NtQuerySystemInformation: { args: [FFIType.u32, FFIType.ptr, FFIType.u32, FFIType.ptr], returns: FFIType.i32 },
}).symbols

const u64 = new BigUint64Array(1)
const u64p = ptr(u64)
const qpcFreq = (() => { k32.QueryPerformanceFrequency(u64p); return Number(u64[0]) })()
const nowNs = () => { k32.QueryPerformanceCounter(u64p); return Number(u64[0]) * (1e9 / qpcFreq) }

const argv = process.argv.slice(2)
const sep = argv.indexOf("--")
const out = argv[0]
const alsoAt = argv.indexOf("--also")
const also = alsoAt >= 0 && alsoAt < sep ? argv[alsoAt + 1].split(",").map(Number) : []
const cpusAt = argv.indexOf("--cpus")
const cpus = cpusAt >= 0 && cpusAt < sep ? BigInt(`0x${argv[cpusAt + 1].replace(/^0x/, "")}`) : 0n
const cmd = argv.slice(sep + 1)
mkdirSync(out, { recursive: true })

const child = Bun.spawn(cmd, { stdout: Bun.file(`${out}/cmd.out`), stderr: Bun.file(`${out}/cmd.err`), env: process.env })
const root = child.pid
// After the spawn: the command keeps the CPUs it was started with.
if (cpus) k32.SetProcessAffinityMask(k32.GetCurrentProcess(), cpus)

// cycles per ns, from this thread's own cycles over QPC while it spins 300 ms (cycle counters count TSC ticks);
// called once the command ended, so sampling starts with it.
const cyclesPerNsOf = () => {
  const h = k32.GetCurrentThread()
  k32.QueryThreadCycleTime(h, u64p); const c0 = Number(u64[0]); const t0 = nowNs()
  while (nowNs() - t0 < 3e8) {}
  k32.QueryThreadCycleTime(h, u64p); const c1 = Number(u64[0]); const t1 = nowNs()
  return (c1 - c0) / (t1 - t0)
}

const PROCESS_QUERY_LIMITED = 0x1000
const THREAD_QUERY_LIMITED = 0x0800
const procHandles = new Map<number, any>()
const procHandle = (pid: number) => {
  if (!procHandles.has(pid)) procHandles.set(pid, k32.OpenProcess(PROCESS_QUERY_LIMITED, 0, pid))
  return procHandles.get(pid)
}
const threadHandles = new Map<number, any>()
const threadNames = new Map<number, string>()
const wstr = (p: number, maxBytes = 512) => {
  const b = new Uint16Array(toArrayBuffer(p, 0, maxBytes))
  let s = ""
  for (const c of b) { if (!c) break; s += String.fromCharCode(c) }
  return s
}
const threadHandle = (tid: number) => {
  if (!threadHandles.has(tid)) {
    const h = k32.OpenThread(THREAD_QUERY_LIMITED, 0, tid)
    threadHandles.set(tid, h)
    if (h) {
      const pp = new BigUint64Array(1)
      if (k32.GetThreadDescription(h, ptr(pp)) >= 0 && pp[0]) {
        threadNames.set(tid, wstr(Number(pp[0])))
        k32.LocalFree(Number(pp[0]))
      }
    }
  }
  return threadHandles.get(tid)
}
const cyclesOf = (fn: any, h: any) => (h && fn(h, u64p) ? Number(u64[0]) : -1)

let buf = new Uint8Array(8 << 20)
const lenOut = new Uint32Array(1)
type Proc = { pid: number; ppid: number; name: string; create: number; cyc: number; priv: number; ws: number; rd: number; wr: number; rops: number; wops: number; threads: [number, number, number][] }
function snapshot(): Proc[] {
  for (;;) {
    const st = nt.NtQuerySystemInformation(5, ptr(buf), buf.length, ptr(lenOut))
    if (st === 0) break
    buf = new Uint8Array(Math.max(buf.length * 2, lenOut[0] + (1 << 20)))
  }
  const dv = new DataView(buf.buffer)
  const base = ptr(buf)
  const procs: Proc[] = []
  let off = 0
  for (;;) {
    const next = dv.getUint32(off, true)
    const nthreads = dv.getUint32(off + 4, true)
    const nameLen = dv.getUint16(off + 56, true)
    const namePtr = Number(dv.getBigUint64(off + 64, true))
    let name = ""
    if (namePtr && nameLen) {
      const rel = namePtr - base
      name = rel >= 0 && rel < buf.length ? new TextDecoder("utf-16le").decode(buf.subarray(rel, rel + nameLen)) : wstr(namePtr, nameLen)
    }
    const p: Proc = {
      pid: Number(dv.getBigUint64(off + 80, true)),
      ppid: Number(dv.getBigUint64(off + 88, true)),
      name,
      create: Number(dv.getBigInt64(off + 32, true)),
      cyc: Number(dv.getBigUint64(off + 24, true)),
      priv: Number(dv.getBigUint64(off + 200, true)),
      ws: Number(dv.getBigUint64(off + 144, true)),
      rops: Number(dv.getBigInt64(off + 208, true)),
      wops: Number(dv.getBigInt64(off + 216, true)),
      rd: Number(dv.getBigInt64(off + 232, true)),
      wr: Number(dv.getBigInt64(off + 240, true)),
      threads: [],
    }
    for (let i = 0; i < nthreads; i++) {
      const t = off + 256 + i * 80
      const kt = Number(dv.getBigInt64(t, true)), ut = Number(dv.getBigInt64(t + 8, true))
      p.threads.push([Number(dv.getBigUint64(t + 48, true)), dv.getUint32(t + 64, true), kt + ut])
    }
    procs.push(p)
    if (!next) break
    off += next
  }
  return procs
}

function tree(procs: Proc[]): Proc[] {
  const byPid = new Map(procs.map((p) => [p.pid, p]))
  const kids = new Map<number, Proc[]>()
  for (const p of procs) (kids.get(p.ppid) ?? kids.set(p.ppid, []).get(p.ppid)!).push(p)
  const out: Proc[] = []
  const seen = new Set<number>()
  const stack = [root, ...also].filter((pid) => byPid.has(pid)).map((pid) => byPid.get(pid)!)
  for (const p of stack) seen.add(p.pid)
  while (stack.length) {
    const p = stack.pop()!
    out.push(p)
    for (const k of kids.get(p.pid) ?? []) if (!seen.has(k.pid) && k.create >= p.create) { seen.add(k.pid); stack.push(k) }
  }
  return out
}

const helper = /^(bun|trace_processor|ffmpeg|strobe|strobew|conhost)/i
let fastPids: number[] = [root]
const fastFd = openSync(`${out}/fast.bin`, "w")
const ROWS = 4096
let fast = new Float64Array(ROWS * 3), fastN = 0
const flushFast = () => { writeSync(fastFd, new Uint8Array(fast.buffer, 0, fastN * 24)); fastN = 0 }
const snapPath = `${out}/snap.jsonl`
writeFileSync(snapPath, "")
const detail = new Set<number>()
const firstRow = new Map<number, any>(), lastRow = new Map<number, any>(), peak = new Map<number, number>()

const timer = k32.CreateWaitableTimerExW(null, null, 0x2, 0x1f0003)
const due = new BigInt64Array([-20000n]) // 2 ms, relative, 100 ns units
let exited = false
child.exited.then(() => { exited = true })
let lastSnap = 0
let snapCost = 0, snapN = 0, tick = 0
const t0 = nowNs()
while (!exited) {
  k32.SetWaitableTimer(timer, ptr(due), 0, null, null, 0)
  k32.WaitForSingleObject(timer, 1000)
  const t = nowNs()
  for (const pid of fastPids) {
    const c = cyclesOf(k32.QueryProcessCycleTime, procHandle(pid))
    if (c < 0) continue
    if (fastN === ROWS) flushFast()
    fast[fastN * 3] = t; fast[fastN * 3 + 1] = pid; fast[fastN * 3 + 2] = c; fastN++
  }
  if (t - lastSnap >= 1e8) {
    lastSnap = t
    const s0 = nowNs()
    const procs = tree(snapshot())
    fastPids = procs.filter((p) => p.pid === root || (p.ppid === root && helper.test(p.name)) || also.includes(p.pid)).map((p) => p.pid)
    for (const p of procs) if (p.pid === root || (p.ppid === root && /^bun/i.test(p.name))) detail.add(p.pid)
    const rows = procs.map((p) => ({
      pid: p.pid, ppid: p.ppid, name: p.name, cyc: p.cyc, priv: p.priv, ws: p.ws, rd: p.rd, wr: p.wr, rops: p.rops, wops: p.wops,
      th: detail.has(p.pid) ? p.threads.map(([tid, cs, kut]) => [tid, cs, cyclesOf(k32.QueryThreadCycleTime, threadHandle(tid)), threadNames.get(tid) ?? ""]) : p.threads.length,
      cs: p.threads.reduce((a, x) => a + x[1], 0),
    }))
    appendFileSync(snapPath, JSON.stringify({ t, procs: rows }) + "\n")
    for (const r of rows) {
      if (!firstRow.has(r.pid)) firstRow.set(r.pid, r)
      lastRow.set(r.pid, r)
      peak.set(r.pid, Math.max(peak.get(r.pid) ?? 0, r.priv))
    }
    snapCost += nowNs() - s0; snapN++
  }
  // Yield to the event loop now and then so the child's exit promise can settle.
  if (++tick % 25 === 0) await Bun.sleep(0)
}
flushFast(); closeSync(fastFd)
const t1 = nowNs()
const cyclesPerNs = cyclesPerNsOf()
writeFileSync(`${out}/meta.json`, JSON.stringify({ root, also, cyclesPerNs, qpcFreq, t0, t1, code: await child.exited, snapMsMean: snapCost / Math.max(1, snapN) / 1e6, snaps: snapN }))
console.log(`tree-sampler: root ${root} exit ${await child.exited}, ${snapN} snapshots (${(snapCost / Math.max(1, snapN) / 1e6).toFixed(2)} ms each), ${cyclesPerNs.toFixed(3)} cycles/ns`)

// What each process of the tree used between its first and last snapshot (a process seen once shows 0),
// and the root's threads.
const wall = (t1 - t0) / 1e6
const mb = (b: number) => (b / 1048576).toFixed(1)
console.log(`over ${(wall / 1000).toFixed(1)} s: CPU ms (% of one core), private peak, read, written`)
for (const [pid, l] of [...lastRow].sort((a, b) => b[1].cyc - firstRow.get(b[0]).cyc - (a[1].cyc - firstRow.get(a[0]).cyc))) {
  const f = firstRow.get(pid)
  const ms = (l.cyc - f.cyc) / cyclesPerNs / 1e6
  if (ms < 1 && pid !== root) continue
  console.log(`  ${`${l.name} ${pid}${pid === root ? " (root)" : ""}`.padEnd(32)} ${ms.toFixed(0).padStart(7)} ms ${((100 * ms) / wall).toFixed(2).padStart(6)}%  ${mb(peak.get(pid) ?? 0).padStart(7)} MB  ${mb(l.rd - f.rd)} MB  ${mb(l.wr - f.wr)} MB`)
}
const r0 = firstRow.get(root), r1 = lastRow.get(root)
if (Array.isArray(r0?.th) && Array.isArray(r1?.th)) {
  const before = new Map(r0.th.map((t: any) => [t[0], t]))
  const rows = r1.th.map(([tid, cs, cyc, name]: any) => {
    const b: any = before.get(tid) ?? [tid, 0, cyc, name]
    return { tid, name, ms: (cyc - b[2]) / cyclesPerNs / 1e6, cs: cs - b[1] }
  })
  console.log("root's threads (alive at the end), CPU ms and context switches:")
  for (const t of rows.sort((a: any, b: any) => b.ms - a.ms).slice(0, 10)) console.log(`  ${String(t.tid).padStart(6)} ${(t.name || "-").padEnd(20)} ${t.ms.toFixed(1).padStart(9)} ms ${String(t.cs).padStart(7)}`)
}
