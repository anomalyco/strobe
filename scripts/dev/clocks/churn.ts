// Thread churn for the thread timeline and wakes (`bun churn.ts [seconds] [threads]`, then
// `strobe record pid:<its pid> --for 10s` and `strobe query <rec> threads --timeline`): `threads`
// worker threads at a time (8) that each sleep 1 ms at a time for 0.4 to 1.6 s, then exit, with a
// new one started for each that ends.
// Windows hands a freed thread id to the next thread, often in the same process: a thread under a
// reused id is another thread, whose counts start from zero, never a count that went backwards
// (which, read as a 32-bit difference, was 4.29 billion wakes in a second).
const worker = `
  const life = 400 + Math.random() * 1200;
  const cell = new Int32Array(new SharedArrayBuffer(4));
  const end = performance.now() + life;
  while (performance.now() < end) Atomics.wait(cell, 0, 0, 1);
  postMessage("done");
`;
const url = URL.createObjectURL(new Blob([worker], { type: "application/javascript" }));
const seconds = Number(process.argv[2] ?? 30);
const alive = Number(process.argv[3] ?? 8);
const until = Date.now() + seconds * 1000;
let started = 0;
function start() {
  if (Date.now() > until) return;
  const w = new Worker(url);
  started++;
  w.onmessage = () => {
    w.terminate();
    start();
  };
}
for (let i = 0; i < alive; i++) start();
console.log(`pid ${process.pid}`);
setTimeout(() => {
  console.log(`started ${started} threads`);
  process.exit(0);
}, seconds * 1000 + 2000);
