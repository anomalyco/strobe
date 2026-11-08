// Background CPU load for hostile-condition runs (not shipped). Start it pinned to the bench's CPUs
// (Windows: set the process's affinity after it starts; Linux: taskset), so strobe's CPU reservation
// leaves it there as work pinned on purpose.
//   bun load.ts <threads> always             spin every thread until killed
//   bun load.ts <threads> <ms> <markfile>    spin for <ms> each time the file changes (one side's runs)
//   bun load.ts <threads> <on-ms> <off-ms>   spin <on-ms>, rest <off-ms>, repeat (load started mid-block)
import { statSync } from "node:fs"

const [threadsArg, a, b] = process.argv.slice(2)
const threads = Number(threadsArg ?? 1)
const flag = new Int32Array(new SharedArrayBuffer(4))
const worker = `
  onmessage = (e) => {
    const flag = new Int32Array(e.data)
    let h = 1
    for (;;) {
      while (Atomics.load(flag, 0) === 1) for (let i = 0; i < 100000; i++) h = Math.imul(h ^ i, 16777619)
      Atomics.wait(flag, 0, 0, 50)
      if (h === 42) postMessage(h)
    }
  }`
const url = URL.createObjectURL(new Blob([worker], { type: "application/javascript" }))
for (let i = 0; i < threads; i++) new Worker(url).postMessage(flag.buffer)
const set = (on: boolean) => {
  Atomics.store(flag, 0, on ? 1 : 0)
  if (on) Atomics.notify(flag, 0)
}
console.log(`load pid ${process.pid}: ${threads} threads, ${a} ${b ?? ""}`)
if (a === "always") set(true)
else if (b && /^\d+$/.test(b)) {
  const [on, off] = [Number(a), Number(b)]
  for (;;) {
    set(true)
    await Bun.sleep(on)
    set(false)
    await Bun.sleep(off)
  }
} else if (b) {
  const ms = Number(a)
  let last = 0
  let until = 0
  for (;;) {
    let m = 0
    try {
      m = statSync(b).mtimeMs
    } catch {}
    const now = performance.now()
    if (m && m !== last) {
      last = m
      until = now + ms
      set(true)
    } else if (until && now > until) {
      until = 0
      set(false)
    }
    await Bun.sleep(1)
  }
}
