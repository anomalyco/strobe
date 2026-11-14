// On Windows: bun scripts/dev/cli-plan/builtin-stems.ts > scripts/dev/cli-plan/BUILTINS.md
import { spawnSync } from "node:child_process"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir, release } from "node:os"
import { join } from "node:path"

if (process.platform !== "win32") throw Error("needs the system cmd.exe")
const stems = "assoc break call cd chdir cls color copy date del dir dpath echo endlocal erase exit for ftype goto if keys md mkdir mklink move path pause popd prompt pushd rd rem ren rename rmdir set setlocal shift start time title type ver verify vol".split(" ")
const chars = [...".:/\\+=,;[](-_)"]
const cwd = mkdtempSync(join(tmpdir(), "cmd-stems-")), exe = join(process.env.SystemRoot!, "System32/cmd.exe")
// /? asks for help. Empty PATH and cwd exclude external tools. Do not exercise start: it can
// hand a missing name to the desktop instead of printing help, even with redirected handles.
function probe(head: string) {
  const r = spawnSync(exe, ["/d", "/v:off", "/c", `${head} /?`], {cwd, env:{SystemRoot:process.env.SystemRoot, PATH:cwd}, windowsHide:true, windowsVerbatimArguments:true, encoding:"utf8", timeout:2000})
  if (r.error || r.status == null) throw Error(`probe did not finish: ${head}`)
  return [r.status, r.stdout, r.stderr].join("\n")
}
try {
  const missing = (head:string) => probe(head).replaceAll(head,"<head>")
  const no = missing("no-such-probe")
  const rows = stems.map(stem => {
    if (stem === "start") return [stem, ...chars.map(()=>"n/a")]
    const help = probe(stem)
    return [stem, ...chars.map(c => {
      const head = stem+c+"probe", result = probe(head)
      if (result === help) return "B"
      if (result.replaceAll(head,"<head>") === no) return "–"
      // Some builtins consume the suffix before /? (echo, rem). Use a non-builtin with the
      // same delimiter as a control for a different lookup error, e.g. a missing directory.
      if (result.replaceAll(stem,"<stem>") === probe("no-such-stem"+c+"probe").replaceAll("no-such-stem","<stem>")) return "–"
      return "D"
    })]
  })
  console.log(`# Unquoted command stems on Windows ${release()}\n\nMeasured by \`builtin-stems.ts\` with an empty working folder and PATH pointing there. Each measured cell runs \`<stem><character>probe /?\` under a fresh system cmd. B: same output and status as the bare builtin's help. D: different from help and from the missing-command control. –: same as a missing external name. The last three columns are controls. The start row is not executed: start can open a window even with /?. The planner guards it like the other stems.\n`)
  for (const row of [["Stem",...chars.map(c=>`\`${c}\``)], chars.map(()=>"---").concat("---"), ...rows]) console.log(`| ${row.join(" | ")} |`)
} finally { rmSync(cwd, {recursive:true, force:true}) }
