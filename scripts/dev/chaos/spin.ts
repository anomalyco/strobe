// Known-answer CPU work for hostile-condition runs (not shipped): N rounds of an integer hash, the same
// instructions every run, so cpu.work and run.duration of one build must read the same on both sides
// of an A/A. `--mark <file>` writes its pid to the file first (load.ts watches it to load the CPUs only
// while one side runs; an outside watcher can suspend or reprioritise that pid). `--exit <code>`,
// `--hang`, `--garbage` and `--orphan` misbehave on purpose.
const args = process.argv.slice(2)
const flag = (name: string) => {
  const i = args.indexOf(name)
  return i < 0 ? undefined : (args[i + 1] ?? "")
}
const n = Number(args.find((a) => /^\d+$/.test(a)) ?? 30_000_000)
const mark = flag("--mark")
if (mark) await Bun.write(mark, String(process.pid))
if (args.includes("--garbage")) {
  // Binary noise and lines that look like metrics but are not, before the real one.
  process.stdout.write(Buffer.from(Array.from({ length: 4096 }, (_, i) => (i * 37) & 0xff)))
  console.log("\nstrobe:metric spin_ms=NaN")
  console.log("strobe:metric spin_ms=12 sideways")
}
if (args.includes("--latin1")) {
  // A metric, a line in a legacy code page (é as the single byte 0xE9: not UTF-8), then the metric again:
  // `metric.part` must read 3.
  console.log("strobe:metric part=1")
  process.stdout.write(Buffer.from([0x63, 0x61, 0x66, 0xe9, 0x0a]))
  console.log("strobe:metric part=2")
}
if (args.includes("--orphan")) Bun.spawn([process.execPath, "-e", "setTimeout(() => {}, 60_000)"], { stdio: ["ignore", "ignore", "ignore"] }).unref()
const t0 = performance.now()
let h = 2166136261
for (let i = 0; i < n; i++) h = Math.imul(h ^ i, 16777619)
const ms = performance.now() - t0
console.log(`strobe:metric spin_ms=${ms.toFixed(3)}`)
if (args.includes("--hang")) await new Promise(() => {})
const exit = flag("--exit")
if (exit !== undefined) process.exit(Number(exit))
if (h === 42) console.log("unlikely")
