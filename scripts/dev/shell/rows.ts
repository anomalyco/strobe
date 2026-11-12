// The shell rule's table (`cli::shell_line`): per OS, a command line, whether the shell runs it and stays its parent
// (Windows cmd.exe; Unix sh without exec), and the lines its programs print (argv.ts its arguments, stdin.ts what it
// read). shell.bench.ts runs every row; check.ts checks strobe's `ShellLine` notes against `shell`.
// Windows reads quotes as a program reads its arguments (a `\"` is a quote inside one) and sends to cmd only what was
// written for cmd: `&&`, `||`, `|`, `<`, `>` outside quotes, a `&` apart from a word, a caret before an operator. Unix
// reads the line as sh does. The shapes consumers of 0.32.0 had (a quoted JSON argument, a URL with `&`, a path with
// parentheses, `git rev-parse HEAD^`) run their program directly on Windows, as they did.
const P = "bun scripts/dev/shell/argv.ts"
const S = "bun scripts/dev/shell/stdin.ts"
const argv = (...a: string[]) => "argv " + JSON.stringify(a)
export type Row = { line: string; shell: boolean; out: (string | RegExp)[] }

const windows: Record<string, Row> = {
  plain: { line: `${P} a b`, shell: false, out: [argv("a", "b")] },
  and: { line: `${P} a && ${P} b`, shell: true, out: [argv("a"), argv("b")] },
  or: { line: `${P} a || ${P} b`, shell: true, out: [argv("a")] },
  "amp-apart": { line: `${P} a & ${P} b`, shell: true, out: [argv("a"), argv("b")] },
  pipe: { line: `${P} a | ${S}`, shell: true, out: ["stdin " + argv("a")] },
  redirect: { line: `${P} a > shell-out.txt && type shell-out.txt && del shell-out.txt`, shell: true, out: [argv("a")] },
  "quoted-ops": { line: `${P} "a && b" "c|d" "e>f"`, shell: false, out: [argv("a && b", "c|d", "e>f")] },
  "json-pipe": { line: `${P} "{\\"op\\":\\"a|b\\"}"`, shell: false, out: [argv('{"op":"a|b"}')] },
  "json-amp": { line: `${P} "{\\"q\\":\\"x & y\\"}"`, shell: false, out: [argv('{"q":"x & y"}')] },
  "url-quoted": { line: `${P} "http://h/?a=1&b=2"`, shell: false, out: [argv("http://h/?a=1&b=2")] },
  "url-bare": { line: `${P} http://h/?a=1&b=2`, shell: false, out: [argv("http://h/?a=1&b=2")] },
  parens: { line: `${P} "C:\\Program Files (x86)\\x" (y)`, shell: false, out: [argv("C:\\Program Files (x86)\\x", "(y)")] },
  caret: { line: `${P} HEAD^ a^b`, shell: false, out: [argv("HEAD^", "a^b")] },
  "git-caret": { line: `git rev-parse HEAD^`, shell: false, out: [/^[0-9a-f]{40}$/] },
  "caret-pipe": { line: `${P} a^|b`, shell: true, out: [argv("a|b")] },
  "caret-amp": { line: `${P} a^&b`, shell: true, out: [argv("a&b")] },
}

const unix: Record<string, Row> = {
  plain: { line: `${P} a b`, shell: false, out: [argv("a", "b")] },
  and: { line: `${P} a && ${P} b`, shell: true, out: [argv("a"), argv("b")] },
  or: { line: `${P} a || ${P} b`, shell: true, out: [argv("a")] },
  semicolon: { line: `${P} a; ${P} b`, shell: true, out: [argv("a"), argv("b")] },
  pipe: { line: `${P} a | ${S}`, shell: true, out: ["stdin " + argv("a")] },
  redirect: { line: `${P} a > shell-out.txt && cat shell-out.txt && rm shell-out.txt`, shell: true, out: [argv("a")] },
  substitution: { line: `${P} $(echo a)`, shell: true, out: [argv("a")] },
  "quoted-ops": { line: `${P} 'a && b' "c|d" 'e>f'`, shell: false, out: [argv("a && b", "c|d", "e>f")] },
  "json-pipe": { line: `${P} '{"op":"a|b"}' "{\\"op\\":\\"c|d\\"}"`, shell: false, out: [argv('{"op":"a|b"}', '{"op":"c|d"}')] },
  "json-amp": { line: `${P} '{"q":"x & y"}'`, shell: false, out: [argv('{"q":"x & y"}')] },
  "url-quoted": { line: `${P} 'http://h/?a=1&b=2'`, shell: false, out: [argv("http://h/?a=1&b=2")] },
  parens: { line: `${P} '/opt/dir (x86)/x' '(y)'`, shell: false, out: [argv("/opt/dir (x86)/x", "(y)")] },
  caret: { line: `${P} HEAD^ a^b`, shell: false, out: [argv("HEAD^", "a^b")] },
  "git-caret": { line: `git rev-parse HEAD^`, shell: false, out: [/^[0-9a-f]{40}$/] },
  "escaped-pipe": { line: `${P} a\\|b`, shell: false, out: [argv("a|b")] },
}

export const rows = process.platform === "win32" ? windows : unix
