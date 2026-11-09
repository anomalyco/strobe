// The script() target of main-thread.bench.ts (dev only, not shipped): each export keeps a different thread busy.
// Also a windowless program (`bun main-thread-harness.ts <mode>`) that does the same thing without end, for the
// native() targets.

/** `ms` of wall time busy on the calling thread. */
function busy(ms: number) {
  const end = performance.now() + ms
  let x = 0
  while (performance.now() < end) x += Math.sqrt(x + 1)
  return x > 0
}

const spinner = `self.onmessage = (e) => {
  const ms = e.data
  const end = performance.now() + (ms < 0 ? 1e12 : ms)
  let x = 0
  while (performance.now() < end) x += Math.sqrt(x + 1)
  self.postMessage(x > 0)
}`
const childCode = (ms: number) => `const end = performance.now() + ${ms < 0 ? 1e12 : ms}; let x = 0; while (performance.now() < end) x += Math.sqrt(x + 1); process.exit(x > 0 ? 0 : 1)`

// Started once, in the first (warmup) run: a worker's start costs the main thread too.
let worker: Worker | undefined
function spinWorker() {
  worker ??= new Worker(URL.createObjectURL(new Blob([spinner], { type: "application/javascript" })))
  return worker
}

/** The main thread busy for `ms`. */
export function mainSpin(ms: number) {
  return busy(ms)
}

/** A worker thread busy for `ms` while the main thread waits for it. */
export function workerSpin(ms: number) {
  const w = spinWorker()
  return new Promise<boolean>((resolve) => {
    w.onmessage = (e) => resolve(e.data)
    w.postMessage(ms)
  })
}

/** A child process (this runtime, a known binary) busy for `ms` while the main thread waits for it. */
export async function childSpin(ms: number) {
  const p = Bun.spawn([process.execPath, "-e", childCode(ms)], { stdout: "ignore", stderr: "ignore" })
  return (await p.exited) === 0
}

/** The main thread waiting on a timer for `ms`. */
export async function timerWait(ms: number) {
  await Bun.sleep(ms)
  return true
}

if (import.meta.main) {
  const mode = process.argv[2] ?? "timer-wait"
  console.log(`READY ${mode}`)
  if (mode === "main-spin") busy(1e12)
  else if (mode === "worker-spin") spinWorker().postMessage(-1)
  else if (mode === "child-spin") Bun.spawn([process.execPath, "-e", childCode(-1)], { stdout: "ignore", stderr: "ignore" })
  else if (mode === "timer-wait") setInterval(() => {}, 10)
  else throw new Error(`mode: main-spin, worker-spin, child-spin or timer-wait, not ${mode}`)
  // Kept alive by the worker, the child or the interval.
  setInterval(() => {}, 1 << 30)
}
