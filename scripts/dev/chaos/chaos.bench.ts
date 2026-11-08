// Known-answer cases for hostile-condition runs (not shipped). Every case does fixed work, so an A/A
// must read `same` and a value that moved is the condition's doing. With CHAOS_MARK set, each head run
// first writes that file, and load.ts (started pinned to the bench's CPUs) loads them only then:
// other work that lines up with one side, as an antivirus scanning a freshly built head would.
//   strobe bench scripts/dev/chaos/chaos.bench.ts --aa --head . --headless --filter page-spin
//   strobe bench scripts/dev/chaos/chaos.bench.ts --check --head . --headless
// `navigates`, `closes`, `crashes` and `child-exits` misbehave on purpose: each must refuse, never print a value.
import { writeFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { bench, cli, script, url } from "@hona/strobe"

const work = (n: number) => {
  let h = 2166136261
  for (let i = 0; i < n; i++) h = Math.imul(h ^ i, 16777619)
  return h
}
const page = "data:text/html,<title>chaos</title><h1>chaos</h1>"
const mark = (variant: string) => {
  if (process.env.CHAOS_MARK && variant === "head") writeFileSync(process.env.CHAOS_MARK, String(performance.now()))
}

export default bench({
  targets: {
    page: url(page, { headless: true }),
    harness: script("./work.ts"),
    command: cli(`bun "${fileURLToPath(new URL("spin.ts", import.meta.url))}" 300000000`),
  },
  cases: {
    "page-spin": {
      targets: ["page"],
      primary: "cpu.work",
      metrics: ["main.work", "run.duration"],
      async run({ page, variant }) {
        mark(variant)
        await page.eval(work, 300_000_000)
      },
    },
    "script-spin": {
      targets: ["harness"],
      primary: "cpu.work",
      metrics: ["run.duration"],
      async run({ call, variant }) {
        mark(variant)
        await call("spin", 300_000_000)
      },
    },
    "cli-spin": {
      targets: ["command"],
      primary: "cpu.work",
      metrics: ["run.duration"],
      async run({ exec, variant }) {
        mark(variant)
        await exec()
      },
    },
    // The work runs in a child that has exited by the run's end: cpu.work cannot count it.
    "child-exits": {
      targets: ["harness"],
      primary: "cpu.work",
      run: ({ call }) => call("spinInChild", 300_000_000),
    },
    crashes: {
      targets: ["harness"],
      primary: "cpu.work",
      run: ({ call }) => call("crash"),
    },
    navigates: {
      targets: ["page"],
      primary: "cpu.work",
      async run({ page }) {
        // Chromium refuses a top-level navigation to a data: URL; about:blank it takes.
        await page.eval(() => setTimeout(() => (location.href = "about:blank"), 50))
        await page.wait(200)
        await page.eval(work, 100_000_000)
        await page.wait(300)
      },
    },
    closes: {
      targets: ["page"],
      primary: "cpu.work",
      async run({ page }) {
        // A window a script did not open ignores window.close(); one reopened onto itself does not.
        await page.eval(() => setTimeout(() => window.open("", "_self")?.close(), 50))
        await page.wait(200)
        await page.eval(work, 100_000_000)
        await page.wait(300)
      },
    },
  },
})
