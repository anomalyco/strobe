// Known answers for terminal programs that exit (dev only, not shipped): every case here fails, each with the
// refusal it should. Build known.rs first (see cli.bench.ts), then run from the repo root:
//   strobe bench scripts/dev/known/exits.bench.ts --head . --check
// Truth:
//   prints    a command that prints and exits 0 at once: RunIncomplete 'the app exited (code 0) before …', its last
//             rows, and the fix that it is a cli() target (never NotForTarget for cpu.work, which a tty target measures)
//   fails     a command that exits 2 with its usage on stderr: RunIncomplete with code 2 and that line
//   quits     a terminal app that quits on the run's `q`: RunIncomplete 'the app exited (code 0) during quits's run 1'
//   late      a windowless app that prints READY at once; setup waits for it 1 s later (since the call, the default):
//             the wait times out, and says a matching line came before the call (pass { since: "launch" })
import { bench, native, tty } from "@hona/strobe"
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")

export default bench({
  targets: {
    prints: tty([bin, "spin", "1"], { cols: 80, rows: 24 }),
    fails: tty([bin, "bogus"], { cols: 80, rows: 24 }),
    keys: tty([bin, "tty", "40", "20"], { cols: 80, rows: 24 }),
    ready: native(bin, { args: ["ready", "0", "READY"], window: false, output: true }),
  },
  setup: async ({ case: c, page, waitForLine }: any) => {
    if (c !== "late") return
    await page.wait(1000)
    await waitForLine("READY", { timeout: 2000 })
  },
  cases: {
    late: { targets: ["ready"], primary: "cpu.work", run: ({ page }: any) => page.wait(100) },
    prints: { targets: ["prints"], primary: "cpu.work", run: ({ page }: any) => page.wait(100) },
    fails: { targets: ["fails"], primary: "cpu.work", run: ({ page }: any) => page.wait(100) },
    quits: {
      targets: ["keys"],
      primary: "cpu.work",
      resetCheck: false,
      run: async ({ page }: any) => {
        await page.waitFor("text=ready")
        await page.press("q")
        await page.wait(300)
      },
    },
  },
})
