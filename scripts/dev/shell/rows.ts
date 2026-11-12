// What a cli() command line means (`cli::plan`), row by row: the executable form of the meaning SPEC 9.6 states.
// strobe never measures something other than what the line says without saying so: a line whose meaning is clear
// runs, and one that two reasonable readings would run differently is refused before anything starts.
//   direct   the program is started without a shell around it (Unix: sh -c "exec <line>"; Windows: the program
//            itself, the rest of the line its argument string as written)
//   shell    the shell runs the whole line and stays its parent (Unix sh, Windows cmd.exe): strobe notes
//            `ShellLine`, as its start-up is inside the time and CPU
//   refused  strobe refuses the line before starting anything; `refusal` is a part of its text
// `out` is what the line's programs print (argv.mjs its arguments, stdin.mjs what it read, env.mjs FOO), `note`
// that strobe notes `DirectLine` (Windows: a direct line holding a ^, a %NAME% that exists or single quotes,
// passed as written). `needs` names a program the row runs (left out where it is not on PATH). {port} is
// shell.bench.ts's own server, whose answer is `query <the URL's query>`. shell.bench.ts runs every row as a case
// and check.ts checks the outcomes; both run on each OS from the repo root.
const P = "bun scripts/dev/shell/argv.mjs"
const S = "bun scripts/dev/shell/stdin.mjs"
const argv = (...a: string[]) => "argv " + JSON.stringify(a)
const HASH = /^[0-9a-f]{40}$/
export type Row = {
  line: string
  outcome: "direct" | "shell" | "refused"
  out?: (string | RegExp)[]
  refusal?: string
  note?: true
  needs?: string
}
const AMP = "a single `&` outside quotes"

const windows: Record<string, Row> = {
  // Started directly: no shell syntax, so ^, %NAME% and single quotes are the program's characters.
  plain: { line: `${P} a b`, outcome: "direct", out: [argv("a", "b")] },
  "quoted-ops": { line: `${P} "a && b" "c|d" "e>f"`, outcome: "direct", out: [argv("a && b", "c|d", "e>f")] },
  "json-pipe": { line: `${P} "{\\"op\\":\\"a|b\\"}"`, outcome: "direct", out: [argv('{"op":"a|b"}')] },
  "json-amp": { line: `${P} "{\\"q\\":\\"x & y\\"}"`, outcome: "direct", out: [argv('{"q":"x & y"}')] },
  "url-quoted": { line: `${P} "http://h/?a=1&b=2"`, outcome: "direct", out: [argv("http://h/?a=1&b=2")] },
  "curl-quoted": { line: `curl -s "http://127.0.0.1:{port}/?a=1&b=2"`, outcome: "direct", out: ["query ?a=1&b=2"], needs: "curl" },
  parens: { line: `${P} "C:\\Program Files (x86)\\x" (y)`, outcome: "direct", out: [argv("C:\\Program Files (x86)\\x", "(y)")] },
  caret: { line: `${P} HEAD^ a^b`, outcome: "direct", out: [argv("HEAD^", "a^b")], note: true },
  "caret-pair": { line: `${P} a^^b`, outcome: "direct", out: [argv("a^^b")], note: true },
  "git-caret": { line: `git rev-parse HEAD^`, outcome: "direct", out: [HASH], note: true, needs: "git" },
  "git-caret-quoted": { line: `git rev-parse "HEAD^"`, outcome: "direct", out: [HASH], needs: "git" },
  percent: { line: `${P} %STROBE_SHELL_X%`, outcome: "direct", out: [argv("%STROBE_SHELL_X%")], note: true },
  "percent-unset": { line: `${P} %STROBE_SHELL_UNSET%`, outcome: "direct", out: [argv("%STROBE_SHELL_UNSET%")] },
  "single-quotes": { line: `${P} 'a b'`, outcome: "direct", out: [argv("'a", "b'")], note: true },
  "node-version": { line: `node --version`, outcome: "direct", out: [/^v\d+\./], needs: "node" },
  // cmd lines: an operator outside quotes, a leading (, a .cmd/.bat program (by PATH and PATHEXT), no program.
  and: { line: `${P} a && ${P} b`, outcome: "shell", out: [argv("a"), argv("b")] },
  or: { line: `${P} a || ${P} b`, outcome: "shell", out: [argv("a")] },
  pipe: { line: `${P} a | ${S}`, outcome: "shell", out: ["stdin " + argv("a")] },
  redirect: { line: `${P} a > shell-out.txt && type shell-out.txt && del shell-out.txt`, outcome: "shell", out: [argv("a")] },
  "stderr-to-stdout": { line: `${P} a 2>&1`, outcome: "shell", out: [argv("a")] },
  "stderr-to-nul": { line: `${P} a 2>nul`, outcome: "shell", out: [argv("a")] },
  "node-nul": { line: `node scripts/dev/shell/argv.mjs a > NUL`, outcome: "shell", out: [], needs: "node" },
  "leading-paren": { line: `(${P} a)`, outcome: "shell", out: [argv("a")] },
  "caret-pipe": { line: `${P} a^|b`, outcome: "shell", out: [argv("a|b")] },
  "caret-amp": { line: `${P} a^&b`, outcome: "shell", out: [argv("a&b")] },
  "cmd-caret-pair": { line: `${P} a^^b | ${S}`, outcome: "shell", out: ["stdin " + argv("a^b")] },
  "cmd-percent": { line: `${P} %STROBE_SHELL_X% | ${S}`, outcome: "shell", out: ["stdin " + argv("xval")] },
  echo: { line: `echo hi`, outcome: "shell", out: ["hi"] },
  "npm-run": { line: `npm run -s --prefix scripts/dev/shell/npmrun x`, outcome: "shell", out: [argv("x")], needs: "npm" },
  "npx-flag": { line: `npx --version`, outcome: "shell", out: [/^\d+\.\d+\.\d+/], needs: "npx" },
  "tsc-like": { line: `tsc-shim`, outcome: "shell", out: [argv()] },
  "tsc-like-args": { line: `tsc-shim --noEmit`, outcome: "shell", out: [argv("--noEmit")] },
  "pnpm-and-node": { line: `pnpm-shim -s build && node scripts/dev/shell/argv.mjs x`, outcome: "shell", out: [argv("-s", "build"), argv("x")], needs: "node" },
  // Refused: two readings would run different things.
  "amp-apart": { line: `${P} a & ${P} b`, outcome: "refused", refusal: AMP },
  "amp-tight": { line: `${P} a&${P} b`, outcome: "refused", refusal: AMP },
  "amp-after-quote": { line: `${P} "x"&${P} y`, outcome: "refused", refusal: AMP },
  "url-bare": { line: `${P} http://h/?a=1&b=2`, outcome: "refused", refusal: AMP },
  "curl-bare": { line: `curl -s http://127.0.0.1:{port}/?a=1&b=2`, outcome: "refused", refusal: AMP },
  "swallowed-second": { line: `bun -e "console.log(1); // \\"" & bun -e "console.log(2)"`, outcome: "refused", refusal: AMP },
  semicolon: { line: `${P} a; ${P} b`, outcome: "refused", refusal: "`;` outside quotes" },
  "semicolon-arg": { line: `${P} a;b`, outcome: "refused", refusal: "`;` outside quotes" },
  newline: { line: `${P} a\n${P} b`, outcome: "refused", refusal: "a new line outside quotes" },
  "cmd-escaped-quote": { line: `${P} "{\\"op\\":\\"a|b\\"}" > shell-out.txt`, outcome: "refused", refusal: "does not read `\\\"` as a quote" },
  "npx-escaped-quote": { line: `npx tool "{\\"op\\":\\"a|b\\"}"`, outcome: "refused", refusal: "does not read `\\\"` as a quote" },
  "cmd-caret": { line: `git rev-parse HEAD^ > NUL`, outcome: "refused", refusal: "removes a `^` that escapes nothing" },
  assignment: { line: `FOO=1 node scripts/dev/shell/env.mjs`, outcome: "refused", refusal: "Windows has no such form" },
}

const unix: Record<string, Row> = {
  // exec'd: one simple command, quoted as sh quotes.
  plain: { line: `${P} a b`, outcome: "direct", out: [argv("a", "b")] },
  "quoted-ops": { line: `${P} 'a && b' "c|d" 'e>f'`, outcome: "direct", out: [argv("a && b", "c|d", "e>f")] },
  "json-pipe": { line: `${P} '{"op":"a|b"}' "{\\"op\\":\\"c|d\\"}"`, outcome: "direct", out: [argv('{"op":"a|b"}', '{"op":"c|d"}')] },
  "json-amp": { line: `${P} '{"q":"x & y"}'`, outcome: "direct", out: [argv('{"q":"x & y"}')] },
  "url-quoted": { line: `${P} 'http://h/?a=1&b=2'`, outcome: "direct", out: [argv("http://h/?a=1&b=2")] },
  "curl-quoted": { line: `curl -s "http://127.0.0.1:{port}/?a=1&b=2"`, outcome: "direct", out: ["query ?a=1&b=2"], needs: "curl" },
  parens: { line: `${P} '/opt/dir (x86)/x' '(y)'`, outcome: "direct", out: [argv("/opt/dir (x86)/x", "(y)")] },
  caret: { line: `${P} HEAD^ a^b`, outcome: "direct", out: [argv("HEAD^", "a^b")] },
  "git-caret": { line: `git rev-parse HEAD^`, outcome: "direct", out: [HASH], needs: "git" },
  "git-caret-quoted": { line: `git rev-parse "HEAD^"`, outcome: "direct", out: [HASH], needs: "git" },
  "escaped-pipe": { line: `${P} a\\|b`, outcome: "direct", out: [argv("a|b")] },
  "exec-prefix": { line: `exec ${P} a`, outcome: "direct", out: [argv("a")] },
  "node-version": { line: `node --version`, outcome: "direct", out: [/^v\d+\./], needs: "node" },
  "npm-run": { line: `npm run -s --prefix scripts/dev/shell/npmrun x`, outcome: "direct", out: [argv("x")], needs: "npm" },
  "npx-flag": { line: `npx --version`, outcome: "direct", out: [/^\d+\.\d+\.\d+/], needs: "npx" },
  "tsc-like": { line: `tsc-shim`, outcome: "direct", out: [argv()] },
  "tsc-like-args": { line: `tsc-shim --noEmit`, outcome: "direct", out: [argv("--noEmit")] },
  // sh stays: an operator, a substitution, an assignment in front, a builtin.
  and: { line: `${P} a && ${P} b`, outcome: "shell", out: [argv("a"), argv("b")] },
  or: { line: `${P} a || ${P} b`, outcome: "shell", out: [argv("a")] },
  semicolon: { line: `${P} a; ${P} b`, outcome: "shell", out: [argv("a"), argv("b")] },
  newline: { line: `${P} a\n${P} b`, outcome: "shell", out: [argv("a"), argv("b")] },
  pipe: { line: `${P} a | ${S}`, outcome: "shell", out: ["stdin " + argv("a")] },
  redirect: { line: `${P} a > shell-out.txt && cat shell-out.txt && rm shell-out.txt`, outcome: "shell", out: [argv("a")] },
  "stderr-to-stdout": { line: `${P} a 2>&1`, outcome: "shell", out: [argv("a")] },
  "to-stderr": { line: `${P} a >&2`, outcome: "shell", out: [argv("a")] },
  "node-null": { line: `node scripts/dev/shell/argv.mjs a > /dev/null`, outcome: "shell", out: [], needs: "node" },
  substitution: { line: `${P} $(echo a)`, outcome: "shell", out: [argv("a")] },
  echo: { line: `echo hi`, outcome: "shell", out: ["hi"] },
  assignment: { line: `FOO=1 node scripts/dev/shell/env.mjs`, outcome: "shell", out: ["env FOO=1"], needs: "node" },
  "pnpm-and-node": { line: `pnpm-shim -s build && node scripts/dev/shell/argv.mjs x`, outcome: "shell", out: [argv("-s", "build"), argv("x")], needs: "node" },
  // Refused: a single & backgrounds the command and strobe would time a shell that exits at once.
  "url-bare": { line: `${P} http://h/?a=1&b=2`, outcome: "refused", refusal: AMP },
  "curl-bare": { line: `curl -s http://127.0.0.1:{port}/?a=1&b=2`, outcome: "refused", refusal: AMP },
  "trailing-amp": { line: `${P} a &`, outcome: "refused", refusal: AMP },
  "amp-apart": { line: `${P} a & ${P} b`, outcome: "refused", refusal: AMP },
  // bash's &> is a background & and a > for sh on Debian and Ubuntu (dash): refused, not taken as a redirection.
  "amp-redirect": { line: `${P} a &> shell-out.txt`, outcome: "refused", refusal: "`&>` is bash's redirection" },
}

export const rows = process.platform === "win32" ? windows : unix
