# Which process owns the command clock?

**Rule.** Start an external program directly only when one literal program and its arguments account for the entire line, with a known executable and no shell operation left to perform. A named shell is itself measured shell work. Otherwise pass the original line, without substituting even a resolved path into its text, to the platform shell, or refuse before launch when the program or its arguments cannot be established without interpreting another language. Resolution supplies the direct executable and evidence about PATH for shell lines; it does not edit the user's script.

| Command position or environment | What examines it | Rewrite in a shell line |
|---|---|---|
| First literal word, after `&&`, `||`, pipe, or in a group | Identify literal external heads; cmd builtins precede PATH; check every known head for script and shadow hazards | None |
| After a leading redirect or cmd `if`, `for`, `call` | Refuse an implicit cmd form whose next command isn't established. Use an explicit shell script. | None |
| Unix assignment then program, program then redirect/expansion | Check the literal command head, not the arguments; sh interprets expansions | None |
| Explicit shell, Unix `exec` options | The named shell owns its script; shell startup counts. Unknown exec options belong to sh. | None |
| Directory variant, including a Windows canonical root | Resolve explicit paths relative to the variant, using normal Windows path spelling | None |
| PATH with builtin twins, extensionless Unix shims, or the other checkout's local bin | Do not shadow builtins or count extensionless stubs as Windows executables; identify the actual shim and warn on cross-checkout PATH | None |
| Arguments to batch shims | Keep the original quoting. Refuse a percent expansion or quote escape that batch reparsing cannot preserve. | None |

| Shim argument | Outcome with the forwarding shim (`%*`) |
|---|---|
| `"a b"`, `"a&b"`, `"a\|b"`, `"a^b"` | One argument each, with the quoted contents unchanged |
| `%NAME%` or `"%NAME%"` | Refuse: cmd expands percent references even inside quotes |
| `"a\"b"` | Refuse: cmd does not read a backslash as a quote escape |

This table is for **strings**: `run --head` / `--base`, `cli(line)` and `ctx.exec(line)`.
`run -- program arg…` is a different interface: the caller already split the arguments. It runs
the user's own harness; it does not produce a `cli()` measurement. Re-parsing those arguments
would lose the caller's quoting. For example, an argument `a && b` must remain one argument.

`P` is an external program, `A` and `B` are separate invocations. Direct on Unix means the shell
replaces itself with the program. Shell means **the shell owns the line**; its start is included,
even if the OS shell chooses to replace itself with its last command. The clock stops at root exit,
not at the end of a grace period. A surviving worker invalidates the measurement.

| Input | Windows before | Unix before | Windows decision | Unix decision |
|---|---|---|---|---|
| `P`, `P a b`, `P "a b"` | P, whole | sh → exec P, whole | direct | direct |
| Quoted literal `&`, `&&`, `\|`, `<`, `>`, `;`, `(`, `)` | P, literal | sh, literal | direct | direct |
| Quoted literal `*`, `?`, `$`, backtick, `'`, `^`, `%`, escaped double quote | P, literal in native argv | sh strips quotes, double quotes can expand | direct | direct when provably literal; otherwise sh |
| `A && B` | **A only**, B becomes argv | sh → A then B on success | cmd | sh |
| `A \|\| B` | **A only**, B never recovers failure | sh → A then B on failure | cmd | sh |
| `A \| B` | **A only**, no pipe | sh → A and B | cmd | sh |
| `P > out`, `P < in`, `P 2>&1` | **P only**, no redirection | sh opens/duplicates descriptor | cmd | sh |
| `A ; B` / newline | **A only**, B becomes argv | sh runs both | refuse: use `&&` | sh |
| `A &` / bare URL with `&` | P receives `&` literally | sh backgrounds A; can exit early | refuse: quote or use `&&` | refuse: quote or wait explicitly |
| `(A)` | missing program, retry via cmd | sh, grouped A | cmd | sh |
| `X=1 P` | missing program; cmd fails | sh sets X for P | refuse: target env or `set` | sh |
| `P %X%` / `P "$X"` | P, literal `%X%` | sh → exec P with expansion | direct, explain defined `%X%` | sh |
| `P *.txt` | P gets pattern | sh → exec P with matches | direct, native argv | sh |
| `shim` (`shim.cmd` via PATHEXT / executable script via PATH) | retry through cmd → interpreter | exec script's interpreter | cmd, resolved shim | direct |
| Explicit `.bat` / `.cmd` | cmd → shim | extension irrelevant; requires +x | cmd | direct if executable |
| `.ps1` | not a native executable; fallback may open an editor | extension irrelevant; requires +x | refuse: invoke interpreter | direct if executable |
| `cd`, `set`, `echo` | fallback to cmd builtin when no executable | sh builtin or exec external echo | cmd builtin unless PATH executable chosen | sh builtin |
| `P` found only via extension | implicit .exe or cmd retries lookup | no PATHEXT | PATH + PATHEXT, append to full name | refuse if absent |
| `P` absent | direct miss then cmd miss | sh exits 127 | fail, no retry | fail, no number |
| Bare local executable, with/without PATH twin | implicit lookup can pick unintended file | usually PATH only | warn shadow / refuse local-only | warn shadow / refuse local-only |
| `cmd /c "A && B"` | cmd → A, B | if installed, outer sh + cmd | explicit cmd | explicit cmd if installed |
| `sh -c 'A && B'` | single quotes not argv quotes | sh → inner sh → A, B | refuse single quotes; use double quotes | explicit sh |
| `pwsh -c "…"` | pwsh owns payload | if installed, sh → pwsh | explicit pwsh | explicit pwsh if installed |
| `P 'a b'` | **two args with literal quotes** | exec P, one argument | refuse: double quotes | direct |
| `P HEAD^` / `P "HEAD^"` | P, literal | exec P, literal | direct (note unquoted caret) | direct |
| `P` with escaped quote / unquoted caret on cmd line | P receives operators in argv, or cmd reparses | shell quoting | refuse: direct exe, quote caret or use script | sh or direct by allowlist |
| `shim 100%` | batch reparses its arguments | exec script, literal | refuse: quote argument | direct |
| `start /b P` | cmd can return before P | absent unless installed | refuse: omit start or wait | normal lookup |
| `A &> out`, `A &>> out`, `A \|& B`, `[[ … ]]`, `source f` | not portable cmd syntax | background, syntax error, or unintended fallback | use native spelling | refuse: POSIX redirect, `[`, `.`, or explicit bash |
| Unquoted parentheses in Unix argument | P, literal | sh syntax error before P | direct unless group/operator | sh; show its error |

Do not exercise an associated `.ps1` on an older Windows build: the application Windows opens
can be a desktop editor. Use a nonexistent `.ps1` for that baseline; check a real script on the
candidate only. Explicit shells own the syntax **inside** their payload; the runtime leftover
harness covers the commands text inspection cannot see.

### Restore macOS verification

On the Apple silicon host, with the shared slot free, build this revision and run each command separately:

```sh
cargo build --release
export STROBE_BIN="$PWD/target/release/strobe" KNOWN_BIN="${TMPDIR:-/tmp}/cli-plan-known"
rustc -O scripts/dev/known/known.rs -o "$KNOWN_BIN"
bun scripts/dev/cli-plan/check.ts
bun scripts/dev/leftover/check.ts
f=scripts/dev/known/planted.bench.ts
"$STROBE_BIN" bench "$f" --head . --check --runs 3 --filter cli-spin,cli-alloc,cli-io
"$STROBE_BIN" bench "$f" --head . --aa --target cli-a --filter cli-spin,cli-alloc,cli-io --pairs 8
"$STROBE_BIN" bench "$f" --base cli-a --head cli-b --filter cli-spin,cli-alloc,cli-io --pairs 8
"$STROBE_BIN" bench "$f" --head . --aa --target cli-a --filter cli-spin --primary run.duration --pairs 8
"$STROBE_BIN" bench "$f" --base cli-a --head cli-b --filter cli-spin --primary run.duration --pairs 8
"$STROBE_BIN" bench scripts/dev/known/cli.bench.ts --head . --check --filter tree,orphan,breakaway-left
```

Record the A/A interval, launch-adjusted plant and boundaries for wall, CPU and memory before restoring the labels. A cross-target compile is not evidence for any of them.

### Baseline observation (`check.ts --before`)

One headless check per row, not a speed comparison. Each leg records its own PID and parent and
its arguments in a separate file after doing its work. Left sleeps 35 ms; right spends at least
25 ms of process CPU then sleeps 110 ms. The harness prints the observed edges and wall/CPU per
row. The two roots in the explicit `cmd /c` control were siblings under the same shell.

| Shape | Windows workers; wall / CPU (ms) | Linux workers; wall / CPU (ms) |
|---|---|---|
| plain | left; 53.8 / 12.9 | left; 45.2 / 10.5 |
| `&&` | **left only; 54.5 / 13.4** | left + right; 184.4 / 44.6 |
| failing left `\|\|` | left only; failed, no value | fail + right; 183.4 / 42.8 |
| pipe | **left only; 56.3 / 15.7** | left + right; 142.4 / 44.6 |
| redirect | left with redirect as argv; 54.0 / 13.2 | left; 44.7 / 9.9 |
| shim | left; 72.0 / 26.7 | left; 43.9 / 9.2 |
| explicit shell, same `&&` | left + right; 203.2 / 47.5 | left + right; 185.7 / 44.9 |
| `;` | one worker; 54.6 / 13.6 | both; 186.0 / 45.2 |
| newline | one worker; 55.6 / 14.8 | both; 187.1 / 46.7 |
| trailing `&` | left; 53.0 / 13.0 | refused, no value |
| `[[ … ]]` with fallback | not a Windows row | **fallback only; 141.6 / 35.5** |

### Whole-line check

| Shape | Windows wall / CPU (ms) | Linux wall / CPU (ms) | Independent evidence |
|---|---:|---:|---|
| `&&` | 233.2 / 76.8 | 185.3 / 46.0 | both legs completed, same parent |
| failing left `\|\|` | 224.3 / 69.1 | 184.9 / 43.7 | left failed, right completed |
| pipe | 172.8 / 68.6 | 143.2 / 44.5 | right completed its CPU work and sleep |
| output redirect | 70.8 / 24.4 | 44.2 / 9.3 | file contents replaced |
| explicit shell, same `&&` | 235.4 / 81.4 | 185.3 / 45.1 | both legs completed |

The `check.ts` rows check argv and independent receipts, not an exact speed ratio. Shell startup
and runtime startup are work in addition to the planted work. `hooks.bench.ts` checks the target
default and a caught dynamic refusal, including a leftover hidden in an explicit shell.
