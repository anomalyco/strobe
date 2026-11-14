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
  row("shim-argv", 'plan-shim "a b" "a&b" "a|b" "a^b"', win ? "shell" : "direct", "left", ["a b", "a&b", "a|b", "a^b"]),
  row("echo", "echo hello", "shell"), row("cd", "cd .", "shell"), row("set", "set PLAN_X=one", "shell"),
  row("glob", `${a} scripts/dev/cli-plan/*.mjs`, win ? "direct" : "shell", "left", [win ? "scripts/dev/cli-plan/*.mjs" : "scripts/dev/cli-plan/work.mjs"]),
  row("variable", `${a} ${win ? "%PLAN_X%" : '"$PLAN_X"'}`, win ? "direct" : "shell", "left", [win ? "%PLAN_X%" : "one"]),
  row("caret", `${a} HEAD^`, "direct", "left", ["HEAD^"]), row("missing", "plan-program-absent-74", "missing"),
  refuse("amp", `${a} &`, "single `&`"),
  ...(win ? [row("explicit", `cmd /c "${a} && ${b}"`, "shell", "left right"), ...(Bun.which("pwsh") ? [row("explicit-pwsh", `pwsh -NoProfile -c "${a}"`, "shell", "left")] : []),
    refuse("pwsh-single", `pwsh -NoProfile -c '${a}'`, "single quotes"),
    refuse("shim-env", 'plan-shim "%PLAN_X%"', "%*"), refuse("shim-quote", 'plan-shim "a\\"b"', "escaped quote"),
    refuse("leading-redirect", `2>nul .\\script.ps1`, "tool > out"), refuse("if-head", `if exist cfg.json plan-shim`, "cmd /c"),
    refuse("for-head", `for %i in (one) do plan-shim`, "cmd /c"), refuse("call-head", `call plan-shim 100%`, "a.cmd && b"),
    row("echo-twin-and", `echo hello && ${a}`, "shell", "left"),
    ...["path\\tool.exe", "ver+1", "ver:tool", "ver,tool", "ver[tool", "ver]tool", "ver(tool", "keys.exe", "dpath.cmd"].map(name=>refuse(`builtin-stem-${name}`, `echo ok && ${name} > nul`, `quote \`"${name}"`)),
    row("keys", "keys", "shell"), row("dpath", "dpath", "shell"),
    row("echo-dot", `echo. && ${a}`, "shell", "left"), row("echo-colon", "echo:hello", "shell"), row("echo-slash", "echo/hello", "shell"),
    ...["echo.exe", "rem.com"].flatMap(name => [row(`${name}-plain`, `${name} scripts/dev/cli-plan/work.mjs left`, "direct", "left"),
      refuse(`${name}-compound`, `echo ok && ${name} --version`, `quote \`"${name}"`),
      row(`${name}-quoted`, `"${name}" scripts/dev/cli-plan/work.mjs left && ${b}`, "shell", "left right")]),
    ...["echo.cmd", "ver.bat"].flatMap(name => [refuse(`${name}-plain`, `${name} build`, `quote \`"${name}"`), row(`${name}-quoted`, `"${name}"`, "shell", "left")]),
    row("cmd-inner-quote", `cmd /d /s /c "${a} a\\"b\\"c"`, "shell", "left", ['a"b"c']),
    row("cmd-inner-apostrophe", `cmd /d /s /c ""bun.exe" scripts/dev/cli-plan/work.mjs left 'x'"`, "shell", "left", ["'x'"]),
    row("guarded-miss", `where plan-absent-tool >nul 2>nul && plan-absent-tool || ${a}`, "shell", "left"),
    refuse("semicolon", `${a}; ${b}`, "`;` outside"), refuse("newline", `${a}\n${b}`, "newline"), refuse("assignment", `PLAN_X=two ${a}`, "POSIX"),
    refuse("single", `${a} 'a b'`, "single quotes"), refuse("ps1", ".\\absent.ps1", "-File"), refuse("start", `start /b ${a}`, "/wait"),
    refuse("cmd-caret", `${a} HEAD^ > NUL`, "unquoted `^`"), refuse("shim-percent", "plan-shim 100%", "%*"),
    refuse("cmd-quote", `${a} "{\\"x\\":1}" > NUL`, "escaped quote"), row("json", `${a} "{\\"x\\":\\"a|b\\"}"`, "direct", "left", ['{"x":"a|b"}'])]
    : [row("explicit", `sh -c '${a} && ${b}'`, "shell", "left right"), row("semicolon", `${a}; ${b}`, "shell", "left right"),
    row("newline", `${a}\n${b}`, "shell", "left right"), row("assignment", `PLAN_X=two ${a}`, "shell", "left"),
    row("nested", `${a} "$(echo "a&b")"`, "shell", "left", ["a&b"]), row("comment", `${a} # don't skip\n${b}`, "shell", "left right"),
    row("exec", `exec ${a}`, "direct", "left"), row("syntax", `${a} unquoted(x)`, "missing"),
    row("exec-dash", `exec -- ${a}`, "direct", "left"),row("shell-option", `sh -o errexit -c '${a}'`, "shell", "left"),
     row("shell-file", `bash --norc scripts/dev/cli-plan/leg.sh`, "shell", "left"),refuse("outer-expansion", 'bash -c "echo $HOME"', "single-quote the script"),
     row("shell-assignment", `PLAN_X=two sh -c '${a} "$PLAN_X"'`, "shell", "left", ["two"]),
     refuse("attached-c-quote", 'bash -c"echo $HOME"', "separate argument"), refuse("attached-c-variable", 'sh -ec$PLAN_X', "separate argument"),
     refuse("attached-c-substitution", 'sh -c`echo word`', "separate argument"),
     row("shell-redirect", `bash scripts/dev/cli-plan/leg.sh > /dev/null`, "shell", "left"), row("shell-leading-redirect", `</dev/null sh -c '${a}'`, "shell", "left"),
     row("shell-and", `sh -c '${a}' && sh -c '${b}'`, "shell", "left right"), row("shell-or", `sh -c 'exit 1' || ${a}`, "shell", "left"),
     row("shell-group", `(sh -c '${a}')`, "shell", "left"), row("shell-pipe", `sh -c '${a}' | ${p} reader`, "shell", "left reader"),
     row("shell-sequence", `sh -c ':'; sh -c ':'\n${a}`, "shell", "left"), row("shell-multiline", `sh -c '${a}\n${b}'`, "shell", "left right"),
     row("shell-apostrophe", `sh -c '${a} '\\''hi'\\'''`, "shell", "left", ["hi"]),
     row("guarded-miss", `[ -x ./plan-absent-tool ] && ./plan-absent-tool || ${a}`, "shell", "left"),
    refuse("amp-out", `${a} &> ${file}`, "> file 2>&1"), refuse("amp-append", `${a} &>> ${file}`, ">> file 2>&1"),
    refuse("pipe-amp", `${a} |& ${b}`, "2>&1 |"), refuse("bash-bracket", `[[ x ]] || ${b}`, "bash keyword"), refuse("source", "source absent", "not in POSIX")])
]
