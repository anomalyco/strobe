// The run-time rows of the command-line design (cli-exec section F, group 4) and their controls: every `refused`
// row leaves a 30 ms child (child.mjs) running at the line's exit, hidden from any text scan, and must fail
// RunIncomplete ("still running"); every `pass` row waits for what it starts and must pass. Dev only.
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const child = JSON.stringify(join(here, "child.mjs"))
const detach = JSON.stringify(join(here, "detach.mjs"))
const unix = process.platform !== "win32"

export type Row = { key: string; line: string; expect: "refused" | "pass"; proves: string }

export const rows = (crate?: string): Row[] =>
  (unix
    ? [
        { key: "rt-subst-bg", line: `echo "$(bun ${child} >/dev/null 2>&1 &)"`, expect: "refused", proves: "`&` inside a command substitution, its output closed" },
        { key: "rt-ansi-c-bg", line: `bash -c "bun ${child} $'it\\'s' >/dev/null 2>&1 &"`, expect: "refused", proves: "`&` behind an ANSI-C quote in an explicit shell" },
        { key: "rt-comment-quote-bg", line: `# client's note\nsh -c 'bun ${child} >/dev/null 2>&1 &'`, expect: "refused", proves: "an apostrophe in a comment, then a background job in `sh -c`" },
        { key: "rt-eval-bg", line: `eval "bun ${child} >/dev/null 2>&1 &"`, expect: "refused", proves: "a background job made by `eval`" },
        { key: "rt-shim-bg", line: "bg-shim", expect: "refused", proves: "a background job inside a script on PATH" },
        { key: "rt-direct-setsid", line: `bun ${detach}`, expect: "refused", proves: "a program started directly that detaches a child (`setsid`)" },
        { key: "pipeline", line: `bun ${child} | cat`, expect: "pass", proves: "a pipeline: the shell waits for every stage" },
        { key: "waited-bg", line: `sh -c 'bun ${child} & bun ${child} & wait'`, expect: "pass", proves: "background jobs the line waits for" },
      ]
    : [
        { key: "rt-cmd-start-b", line: `cmd /c "start /b bun ${child}"`, expect: "refused", proves: "`start /b` inside an explicit `cmd /c`" },
        { key: "rt-shim-bg", line: "bg-shim", expect: "refused", proves: "`start /b` inside a .cmd script on PATH" },
        { key: "rt-direct-setsid", line: `bun ${detach}`, expect: "refused", proves: "a program started directly that detaches a child" },
        { key: "pipeline", line: `cmd /c "bun ${child} | findstr ok"`, expect: "pass", proves: "a pipeline: cmd waits for every stage" },
      ]
  ).concat(crate ? [{ key: "build", line: `cargo check --offline --manifest-path ${JSON.stringify(join(crate, "Cargo.toml"))}`, expect: "pass", proves: "an ordinary build: cargo and the rustc it starts" }] : [])

export function consoleRows(gui: string, worker: string, namesake: string): Row[] {
  return [
    { key: "console", line: "bun -e 0", expect: "pass", proves: "a console command" },
    { key: "gui", line: `"${gui}" --version`, expect: "pass", proves: "a GUI-subsystem command with no window" },
    { key: "cmd", line: "cmd /d /c exit 0", expect: "pass", proves: "an explicit shell" },
    { key: "shim", line: `"${join(here, "bin", "waited.cmd")}"`, expect: "pass", proves: "a waited batch shim" },
    { key: "wait", line: `"${worker}" wait`, expect: "pass", proves: "a waited console grandchild" },
    { key: "leave", line: `"${worker}" leave "${worker}"`, expect: "refused", proves: "an unawaited worker beside the host" },
    { key: "separate", line: `"${worker}" leave "${worker}" separate`, expect: "refused", proves: "a worker with its own invisible console" },
    { key: "namesake", line: `"${worker}" leave "${namesake}"`, expect: "refused", proves: "a non-system executable with the host's basename" },
    { key: "behind", line: `"${worker}" behind`, expect: "refused", proves: "a worker below the real console host" },
  ]
}
