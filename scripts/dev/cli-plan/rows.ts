const win = process.platform === "win32"
const p = "bun scripts/dev/cli-plan/work.mjs"
const a = `${p} left`, b = `${p} right`
export type Row = { key: string; line: string; how: string; legs?: string; args?: string[]; reason?: string }
const row = (key: string, line: string, how: string, legs = "", args?: string[]): Row => ({key, line, how, legs, args})
const refuse = (key: string, line: string, reason: string): Row => ({key, line, how: "refuse", reason})
const literals = ["a b", "a&&b", "a||b", "a|b", "a<b", "a>b", "a&b", "a;b", "(a)", "a*b", "a?b", "a^b", "don't", "100%"]
const file = `"${process.env.PLAN_FILE}"`
export const rows = [
  row("bare", win ? "whoami" : "id", "direct"), row("plain", `${a} a b`, "direct", "left", ["a","b"]),
  row("literal", `${a} ${literals.map(x => `"${x}"`).join(" ")}`, "direct", "left", literals),
  row("and", `${a} && ${b}`, "shell", "left right"), row("or", `${p} fail || ${b}`, "shell", "fail right"),
  row("pipe", `${a} | ${b}`, "shell", "left right"), row("redirect", `${a} > ${file}`, "shell", "left"),
  row("input", `${p} reader < ${file}`, "shell", "reader"), row("dup", `${a} 2>&1`, "shell", "left"),
  row("group", `(${a})`, "shell", "left"), row("shim", "plan-shim", win ? "shell" : "direct", "left"),
  row("echo", "echo hello", "shell"), row("cd", "cd .", "shell"), row("set", "set PLAN_X=one", "shell"),
  row("glob", `${a} scripts/dev/cli-plan/*.mjs`, win ? "direct" : "shell", "left", [win ? "scripts/dev/cli-plan/*.mjs" : "scripts/dev/cli-plan/work.mjs"]),
  row("variable", `${a} ${win ? "%PLAN_X%" : '"$PLAN_X"'}`, win ? "direct" : "shell", "left", [win ? "%PLAN_X%" : "one"]),
  row("caret", `${a} HEAD^`, "direct", "left", ["HEAD^"]), row("missing", "plan-program-absent-74", "missing"),
  refuse("amp", `${a} &`, "single `&`"),
  ...(win ? [row("explicit", `cmd /c "${a} && ${b}"`, "shell", "left right"), row("explicit-pwsh", `pwsh -NoProfile -c "${a}"`, "shell", "left"),
    refuse("semicolon", `${a}; ${b}`, "`;` outside"), refuse("newline", `${a}\n${b}`, "newline"), refuse("assignment", `PLAN_X=two ${a}`, "POSIX"),
    refuse("single", `${a} 'a b'`, "single quotes"), refuse("ps1", ".\\absent.ps1", "-File"), refuse("start", `start /b ${a}`, "/wait"),
    refuse("cmd-caret", `${a} HEAD^ > NUL`, "unquoted `^`"), refuse("shim-percent", "plan-shim 100%", "%*"),
    refuse("cmd-quote", `${a} "{\\"x\\":1}" > NUL`, "escaped quote"), row("json", `${a} "{\\"x\\":\\"a|b\\"}"`, "direct", "left", ['{"x":"a|b"}'])]
    : [row("explicit", `sh -c '${a} && ${b}'`, "shell", "left right"), row("semicolon", `${a}; ${b}`, "shell", "left right"),
    row("newline", `${a}\n${b}`, "shell", "left right"), row("assignment", `PLAN_X=two ${a}`, "shell", "left"),
    row("nested", `${a} "$(echo "a&b")"`, "shell", "left", ["a&b"]), row("comment", `${a} # don't skip\n${b}`, "shell", "left right"),
    row("exec", `exec ${a}`, "direct", "left"), row("syntax", `${a} unquoted(x)`, "missing"),
    refuse("amp-out", `${a} &> ${file}`, "> file 2>&1"), refuse("amp-append", `${a} &>> ${file}`, ">> file 2>&1"),
    refuse("pipe-amp", `${a} |& ${b}`, "2>&1 |"), refuse("bash-bracket", `[[ x ]] || ${b}`, "bash keyword"), refuse("source", "source absent", "not in POSIX")])
]
