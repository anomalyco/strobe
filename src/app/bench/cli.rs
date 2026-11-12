//! `cli()` targets (SPEC §9.6): a command run to its exit, once per `ctx.exec()`, in the variant's
//! folder (or a `cmd:` variant's own command line), measured from outside: what the OS counted for it
//! and everything it started (`host::spawn::Owned::run_to_exit`), and the `strobe:metric` lines it
//! printed. Nothing of strobe's runs in the command; the runner and strobe stay on the other CPUs.

use super::*;
use std::io::Write;

/// A `metric.<name>`'s direction, as the command's lines give it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Direction {
    Lower,
    Higher,
}

impl Direction {
    pub(super) fn name(self) -> &'static str {
        match self {
            Direction::Lower => "lower",
            Direction::Higher => "higher",
        }
    }
}

/// One command run: wall ms (spawn to exit; on Windows the process's creation plus its first
/// instruction to its exit, `Owned::spawn_span`), what the OS counted, the metrics it printed with
/// their directions (a name printed in both directions is `None`), and its launch's CPU on strobe's
/// thread (ms, NaN where thread cycles are not time).
struct Ran {
    wall: f64,
    usage: host::spawn::Usage,
    metrics: BTreeMap<String, (f64, Option<Direction>)>,
    launch: f64,
    /// Why the shell ran the line and stayed its parent (`Run::Shell`), or None: its commands are the
    /// shell's children.
    shelled: Option<String>,
    /// A line started directly that holds what a cmd user would expect cmd to change (`Run::Direct`).
    direct_note: Option<String>,
}

/// A run's commands summed, or None when one of them has no value.
fn sum_of(r: &[Ran], f: impl Fn(&Ran) -> Option<f64>) -> Option<f64> {
    r.iter().map(f).sum()
}

/// The line `ctx.exec(line?)` runs: the call's own, else the variant's (`cmd:`), else the target's.
fn line_for(asked: &Value, v: &Variant, target: &Value) -> Result<String> {
    let line = asked.as_str().map(str::to_string).or_else(|| match &v.kind {
        Kind::Cmd(l) => Some(l.clone()),
        _ => None,
    });
    let line = line.or_else(|| target["run"].as_str().map(str::to_string)).filter(|l| !l.trim().is_empty());
    line.ok_or_else(|| Error::usage("a cli target needs a command: cli(\"<line>\"), a cmd:<line> variant, or ctx.exec(\"<line>\")"))
}

/// How a command line runs. Its meaning (the table is `scripts/dev/shell/rows.ts`: `shell.bench.ts` runs
/// every row and `check.ts` checks each on its OS): strobe never measures something other than what the line
/// says without saying so. A line whose meaning is clear runs; one that two reasonable readings would run
/// differently is refused before anything starts (`Err`: why, and what to write instead).
/// - Unix: every line is an `sh` line. One simple command is `exec`ed (sh replaced by it); an operator, a
///   substitution, a variable assignment in front or a shell builtin keeps sh as its parent. Refused: a
///   single `&` outside quotes (not `&&`, not `>&`, `<&` or `2>&1`), which backgrounds the command so strobe
///   would time a shell that exits at once, and `&>` (bash's redirection, which sh on Debian and Ubuntu,
///   dash, reads as a background `&` then `>`).
/// - Windows: a line with no shell syntax is started directly, its first word the program and the rest its
///   argument string as written (`^`, `%NAME%` and single quotes literal, as in every release; `DirectLine`
///   says so where a cmd user would expect otherwise). A line with `&&`, `||`, `|`, `<`, `>` (`2>&1`,
///   `2>nul`) or a caret-escaped operator outside quotes, a leading `(`, or whose program is a `.cmd`/`.bat`
///   by PATH and PATHEXT (or no program at all: a builtin such as `echo`) is a cmd line: cmd runs all of it by
///   its rules and expands `%NAME%` (`ShellLine` says both). Refused: a single `&`; `;` and a new line outside
///   quotes (PowerShell's separators, which neither cmd nor a direct start honours); `NAME=value command`;
///   and in a cmd line a `\"` (cmd toggles on every quote), a `^` that escapes nothing (cmd removes it) and a
///   new line anywhere. Quotes are a program's: `"…"`, a `"` after an odd run of `\` is a literal quote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Run {
    /// The program is started directly: Unix `sh -c "exec <line>"` (`exec`: the line starts with `exec`
    /// itself and runs as written). `note`: on Windows, what a cmd user would expect cmd to change, passed as
    /// written (`DirectLine`).
    Direct { exec: bool, note: Option<String> },
    /// The shell runs the whole line and stays its parent: why (`ShellLine`).
    Shell { why: String },
}

/// How Windows finds a line's program (`resolve_program`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Program {
    /// An `.exe` or `.com`: started directly.
    Exe,
    /// A `.cmd` or `.bat` (its file name): only cmd runs it.
    Script(String),
    /// Another file PATHEXT names (`.js`, `.vbs`): cmd opens it by its file type.
    Other(String),
    /// No file of that name: a cmd builtin (`echo`), or nothing.
    Missing,
}

/// `Run` for this OS (`unix_run`, `windows_run`). `env`: the target's own environment, over strobe's.
pub(super) fn plan(line: &str, root: &Path, env: &Value) -> std::result::Result<Run, String> {
    if cfg!(windows) {
        let var = |name: &str| env_var(env, name);
        let path = var("PATH").unwrap_or_default();
        let pathext = var("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        windows_run(line, |p| resolve_program(p, root, &path, &pathext), |n| var(n).is_some())
    } else {
        unix_run(line)
    }
}

/// A variable of the command's environment: the target's `env` (a name in any case, as Windows reads
/// them), else strobe's own.
fn env_var(env: &Value, name: &str) -> Option<String> {
    env.as_object().and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).and_then(|(_, v)| v.as_str().map(str::to_string))).or_else(|| std::env::var(name).ok())
}

/// The words sh runs itself, which `exec` cannot replace it with (keywords, builtins with no program of their
/// own, and `echo`/`printf`/`pwd`/`kill`/`true`/`false`, whose builtin and program differ).
const SHELL_WORDS: &[&str] = &[
    "if", "for", "while", "until", "case", "{", "!", "[", "[[", "cd", "export", "set", "unset", ".", "source", "eval", "test", "alias", "ulimit", "umask", "time", ":", "command", "type", "hash",
    "read", "wait", "exit", "shift", "trap", "times", "getopts", "readonly", "local", "return", "echo", "pwd", "printf", "kill", "true", "false", "break", "continue",
];

/// A first word `NAME=value`: the variable it sets.
fn assignment(word: &str) -> Option<&str> {
    let (name, _) = word.split_once('=')?;
    let mut c = name.chars();
    (c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_') && c.all(|x| x.is_ascii_alphanumeric() || x == '_')).then_some(name)
}

/// A single `&`: refused on both OSes, in their own words.
fn single_amp(windows: bool) -> String {
    format!(
        "a single `&` outside quotes {}, so strobe would time something other than the line's commands: quote the argument if the `&` belongs to a URL or a string (\"http://h/?a=1&b=2\"); use {} to run commands in turn; for parallel jobs, start them and wait for them yourself ({})",
        if windows {
            "splits the line in cmd (an unquoted URL's query becomes a second command) and is an error or a background job in PowerShell"
        } else {
            "runs the command before it in the background, and sh exits at once"
        },
        if windows { "`&&`" } else { "`&&` or `;`" },
        if windows { "a script file that does" } else { "sh -c '… & … & wait'" }
    )
}

/// `Run` of a Unix line (`plan`).
pub(super) fn unix_run(line: &str) -> std::result::Result<Run, String> {
    let c: Vec<char> = line.chars().collect();
    let (mut single, mut double, mut shell) = (false, false, false);
    let mut i = 0;
    while i < c.len() {
        let (prev, next) = (i.checked_sub(1).map(|p| c[p]), c.get(i + 1).copied());
        match c[i] {
            '\\' if !single => i += 1,
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '`' if !single => shell = true,
            '$' if !single && next == Some('(') => shell = true,
            _ if single || double => {}
            '&' if next == Some('&') => {
                shell = true;
                i += 1;
            }
            '&' if matches!(prev, Some('>' | '<')) => shell = true,
            '&' if next == Some('>') => {
                return Err(
                    "`&>` is bash's redirection, but sh (dash on Debian and Ubuntu) reads it as a background `&` and then `>`, so strobe would time a shell that exits at once: write `> file 2>&1`"
                        .into(),
                );
            }
            '&' => return Err(single_amp(false)),
            '|' | '<' | '>' | ';' | '(' | ')' | '\n' => shell = true,
            _ => {}
        }
        i += 1;
    }
    let first = line.split_whitespace().next().unwrap_or("");
    Ok(if shell {
        Run::Shell { why: "it has a shell operator or a substitution outside quotes".into() }
    } else if first == "exec" {
        Run::Direct { exec: true, note: None }
    } else if let Some(name) = assignment(first) {
        Run::Shell { why: format!("it sets {name} for the command (a variable assignment in front)") }
    } else if SHELL_WORDS.contains(&first) {
        Run::Shell { why: format!("it starts with the shell's own `{first}`") }
    } else {
        Run::Direct { exec: false, note: None }
    })
}

/// The `%NAME%` references in a line, in or out of quotes, each once (cmd expands both).
fn percent_names(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = line;
    while let Some(a) = rest.find('%') {
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) => {
                let name = &after[..b];
                let mut c = name.chars();
                let ok = c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_') && c.all(|x| x.is_ascii_alphanumeric() || matches!(x, '_' | '(' | ')'));
                if ok {
                    if !out.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                        out.push(name.to_string());
                    }
                    rest = &after[b + 1..];
                } else {
                    rest = after;
                }
            }
            None => break,
        }
    }
    out
}

/// `Run` of a Windows line (`plan`): `resolve` finds its program, `defined` says whether a variable exists.
pub(super) fn windows_run(line: &str, resolve: impl Fn(&str) -> Program, defined: impl Fn(&str) -> bool) -> std::result::Result<Run, String> {
    let t = line.trim();
    let c: Vec<char> = t.chars().collect();
    let (mut double, mut slashes) = (false, 0usize);
    let (mut op, mut escaped_quote, mut quoted_newline, mut lone_caret, mut caret_pair, mut single_quote) = (false, false, false, false, false, false);
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' {
            slashes += 1;
            i += 1;
            continue;
        }
        let literal = ch == '"' && slashes % 2 == 1;
        slashes = 0;
        let (prev, next) = (i.checked_sub(1).map(|p| c[p]), c.get(i + 1).copied());
        match ch {
            '"' if literal => escaped_quote = true,
            '"' => double = !double,
            '\n' | '\r' if double => quoted_newline = true,
            _ if double => {}
            '\n' | '\r' => return Err(WIN_NEWLINE.into()),
            ';' => {
                return Err("`;` outside quotes: PowerShell runs the commands on either side of it in turn, but neither cmd nor a direct start splits a line there, so the command after it would not run as a command: use `&&` to run commands in turn, or quote the `;` if it belongs to an argument".into());
            }
            '^' => match next {
                Some('&' | '|' | '<' | '>') => {
                    op = true;
                    i += 1;
                }
                Some('^') => {
                    caret_pair = true;
                    i += 1;
                }
                _ => lone_caret = true,
            },
            '&' if next == Some('&') => {
                op = true;
                i += 1;
            }
            '&' if matches!(prev, Some('>' | '<')) => op = true,
            '&' => return Err(single_amp(true)),
            '|' | '<' | '>' => op = true,
            '\'' => single_quote = true,
            _ => {}
        }
        i += 1;
    }
    let program = match t.strip_prefix('"') {
        Some(q) => q.split('"').next().unwrap_or(""),
        None => t.split(char::is_whitespace).next().unwrap_or(""),
    };
    if !t.starts_with('"')
        && let Some(name) = assignment(program)
    {
        return Err(format!(
            "`{name}=…` in front of a command is sh's: Windows has no such form (cmd would look for a program named `{program}`): set {name} in the target's env (cli(\"…\", {{ env: {{ {name}: \"…\" }} }})), or before strobe runs (cmd: set {name}=…; PowerShell: $env:{name} = \"…\")"
        ));
    }
    let vars: Vec<String> = percent_names(t).into_iter().filter(|n| defined(n)).map(|n| format!("%{n}%")).collect();
    let why = if op {
        Some("it has a cmd operator outside quotes: &&, ||, |, <, > or one a ^ escapes".to_string())
    } else if t.starts_with('(') {
        Some("it starts with `(`, a cmd group".into())
    } else {
        match resolve(program) {
            Program::Exe => None,
            Program::Script(f) => Some(format!("its program is {f}, a script only cmd runs")),
            Program::Other(f) => Some(format!("its program {f} is no executable, so cmd opens it")),
            Program::Missing => Some(format!("`{program}` is no program on PATH: a cmd builtin such as echo, or nothing")),
        }
    };
    match why {
        Some(why) => {
            if escaped_quote {
                return Err(format!(
                    "this line goes to cmd ({why}), and cmd does not read `\\\"` as a quote inside quotes: it toggles on every `\"`, so an operator inside can come loose and a second command be swallowed: put the command in a script file (a .cmd, or bun run x.ts), or write the argument without escaped quotes"
                ));
            }
            if quoted_newline {
                return Err(format!("this line goes to cmd ({why}), and {WIN_NEWLINE}"));
            }
            if lone_caret {
                return Err(format!(
                    "this line goes to cmd ({why}), which removes a `^` that escapes nothing (`HEAD^` becomes `HEAD`): quote the argument (\"HEAD^\" is literal for cmd and for a direct start) or double the caret (`HEAD^^`)"
                ));
            }
            let why = if vars.is_empty() { why } else { format!("{why}; cmd expands {} in it", vars.join(", ")) };
            Ok(Run::Shell { why })
        }
        None => {
            let mut kept = vars;
            if lone_caret || caret_pair {
                kept.push("^".into());
            }
            if single_quote {
                kept.push("'".into());
            }
            let note = (!kept.is_empty()).then(|| {
                format!(
                    "a command line is started directly, without a shell: {} {} passed as written (no shell strips a caret, reads ^^ as one, expands a %NAME% or groups words in single quotes); to have cmd read the line, write cmd /c \"…\"",
                    match kept.as_slice() {
                        [one] => format!("`{one}`"),
                        [rest @ .., last] => format!("{} and `{last}`", rest.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ")),
                        [] => String::new(),
                    },
                    if kept.len() == 1 { "is" } else { "are" }
                )
            });
            Ok(Run::Direct { exec: false, note })
        }
    }
}

const WIN_NEWLINE: &str = "a new line outside quotes: cmd runs a line only up to its first new line, and a direct start passes the rest to the program as arguments, so the commands after it would not run: use `&&` to run commands in turn, or one ctx.exec per command";

/// Where Windows finds `program`, as cmd does: a path (from `root` when relative) as named, else `root` then
/// each folder of `path`; a name without an extension with each of `pathext`'s.
pub(super) fn resolve_program(program: &str, root: &Path, path: &str, pathext: &str) -> Program {
    if program.is_empty() {
        return Program::Missing;
    }
    let exts: Vec<String> = pathext.split(';').map(|e| e.trim().to_ascii_lowercase()).filter(|e| e.starts_with('.')).collect();
    let p = Path::new(program);
    let bases: Vec<PathBuf> = if program.contains(['/', '\\', ':']) {
        vec![if p.is_relative() { root.join(p) } else { p.to_path_buf() }]
    } else {
        std::iter::once(root.to_path_buf()).chain(path.split(';').filter(|d| !d.trim().is_empty()).map(|d| PathBuf::from(d.trim().trim_matches('"')))).map(|d| d.join(p)).collect()
    };
    let classify = |f: &Path| {
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match f.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("exe" | "com") => Program::Exe,
            Some("cmd" | "bat") => Program::Script(name),
            _ => Program::Other(name),
        }
    };
    for base in bases {
        if p.extension().is_some() {
            if base.is_file() {
                return classify(&base);
            }
            continue;
        }
        for e in &exts {
            let f = PathBuf::from(format!("{}{e}", base.display()));
            if f.is_file() {
                return classify(&f);
            }
        }
    }
    Program::Missing
}

/// `ShellLine`, once per series: an info line when the shell ran a command line of a side (its own
/// start-up, a few ms, is inside `run.duration` and `cpu.work`), a warning instead once one side of a
/// comparison ran its lines through the shell and the other did not. `side`: how notes name the side.
pub(super) fn shell_note(ctx: &mut Ctx, s: &Series, side: &str, why: Option<&str>) {
    let label = s.label();
    let shell = if cfg!(windows) { "cmd.exe" } else { "sh" };
    let sides = ctx.shells.entry(label.clone()).or_default();
    *sides.entry(side.to_string()).or_insert(false) |= why.is_some();
    let shelled: Vec<String> = sides.iter().filter(|(_, on)| **on).map(|(r, _)| r.clone()).collect();
    let direct: Vec<String> = sides.iter().filter(|(_, on)| !**on).map(|(r, _)| r.clone()).collect();
    let prefix = format!("{label}: ");
    let said = |ctx: &Ctx, level: &str| ctx.warnings.iter().any(|w| w.id == "ShellLine" && w.level == level && w.text.starts_with(&prefix));
    if !shelled.is_empty() && !direct.is_empty() {
        if said(ctx, "warn") {
            return;
        }
        ctx.warnings.retain(|w| !(w.id == "ShellLine" && w.text.starts_with(&prefix)));
        let w = validity::warn(
            "ShellLine",
            format!(
                "{label}: {}'s command line runs through {shell} and {}'s does not, so only {}'s run.duration and cpu.work hold the shell's own start-up (a few ms a command line): the sides are not measured alike",
                shelled.join(", "),
                direct.join(", "),
                shelled.join(", ")
            ),
        );
        progress(format!("  ! ShellLine  {}", w.text));
        ctx.warnings.push(w);
    } else if let Some(why) = why.filter(|_| !said(ctx, "info") && !said(ctx, "warn")) {
        let w = validity::info("ShellLine", format!("{label}: a command line runs through {shell} ({why}), so the shell's own start-up (a few ms) is inside run.duration and cpu.work"));
        progress(format!("  i ShellLine  {}", w.text));
        ctx.warnings.push(w);
    }
}

/// `DirectLine`, once per series: a Windows line started directly that holds what a cmd user would expect
/// cmd to change (`Run::Direct`'s note).
fn direct_note(ctx: &mut Ctx, s: &Series, note: &str) {
    let text = format!("{}: {note}", s.label());
    if !ctx.warnings.iter().any(|w| w.id == "DirectLine" && w.text.starts_with(&format!("{}: ", s.label()))) {
        let w = validity::info("DirectLine", text);
        progress(format!("  i DirectLine  {}", w.text));
        ctx.warnings.push(w);
    }
}

/// The command for `line` as `run` says, in `root`. Unix: `sh -c`, as `exec <line>` when the program is
/// started directly. Windows: the program itself with the rest of the line as its argument string (a
/// relative path from `root`: Windows would resolve it from strobe's own folder), or the line under cmd.
fn command(line: &str, run: &Run, root: &Path) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let line = line.trim();
        if let Run::Shell { .. } = run {
            return crate::target::command::shell(line);
        }
        let (program, rest) = match line.strip_prefix('"') {
            Some(q) => q.split_once('"').map(|(p, r)| (p.to_string(), r.to_string())).unwrap_or((q.to_string(), String::new())),
            None => line.split_once(char::is_whitespace).map(|(p, r)| (p.to_string(), r.to_string())).unwrap_or((line.to_string(), String::new())),
        };
        let path = Path::new(&program);
        let program = if path.is_relative() && program.contains(['/', '\\']) {
            let joined = root.join(path);
            let exe = joined.with_extension("exe");
            if path.extension().is_none() && exe.is_file() { exe } else { joined }.display().to_string()
        } else {
            program
        };
        let mut c = Command::new(program);
        if !rest.trim().is_empty() {
            c.raw_arg(rest.trim());
        }
        c
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        match run {
            Run::Direct { exec: false, .. } => crate::target::command::shell(&format!("exec {line}")),
            _ => crate::target::command::shell(line),
        }
    }
}

/// `strobe:metric <name>=<value> [lower|higher]` at the start of a line (after spaces): Some(Err)
/// for such a line that does not parse, None for any other line.
fn metric_line(line: &str) -> Option<std::result::Result<(String, f64, Direction), String>> {
    let rest = line.trim_start().strip_prefix("strobe:metric ")?;
    let parse = || -> Option<(String, f64, Direction)> {
        let (name, tail) = rest.trim().split_once('=')?;
        let name = name.trim();
        let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        let mut words = tail.split_whitespace();
        let value = words.next()?.parse::<f64>().ok().filter(|v| v.is_finite())?;
        let dir = match words.next() {
            None | Some("lower") => Direction::Lower,
            Some("higher") => Direction::Higher,
            Some(_) => return None,
        };
        (ok && words.next().is_none()).then(|| (name.to_string(), value, dir))
    };
    Some(parse().ok_or_else(|| line.trim().to_string()))
}

/// What one `ctx.exec` needs from its block.
struct Spawn<'a> {
    root: &'a Path,
    target: &'a Value,
    cpus: Option<u64>,
    log: &'a Path,
    /// `HOME_VAR` of the block's commands: what leaves their group is still found.
    stamp: &'a Path,
    timeout: Duration,
}

/// Runs one command to its exit, its output read line by line (into the experiment's cli.log). With
/// `cost`, strobe's own CPU counts while the command runs: from just after its launch (its pipes
/// handed to the readers) to its exit.
fn exec(line: &str, at: &Spawn, cost: Option<&SelfCost>) -> Result<(Ran, String)> {
    // What the line means, before anything starts: a line two readings would run differently is refused.
    let run = plan(line, at.root, &at.target["env"]).map_err(|why| Error::usage(format!("`{line}`: {why}")))?;
    // The two readers start before the command and wait for its pipes, so creating their threads is
    // strobe's work before the command runs, not beside it.
    type Pipe = Box<dyn std::io::Read + Send>;
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (pipes, readers): (Vec<_>, Vec<_>) = (0..2)
        .map(|_| {
            let (tx, rx) = std::sync::mpsc::channel::<Pipe>();
            let lines = lines.clone();
            let reader = std::thread::spawn(move || {
                let Ok(r) = rx.recv() else { return };
                host::spawn::each_line(r, |l| lines.lock().unwrap_or_else(|e| e.into_inner()).push(l));
            });
            (tx, reader)
        })
        .unzip();
    // The launch's CPU on this thread (`cli.launch`): the OS creating the process, which Windows does
    // in the launcher. NaN where thread cycles are not time.
    let start = |run: &Run| -> std::io::Result<(host::spawn::Owned, Instant, f64)> {
        let mut cmd = command(line, run, at.root);
        for (k, val) in at.target["env"].as_object().into_iter().flatten() {
            cmd.env(k, val.as_str().unwrap_or(""));
        }
        cmd.env(host::spawn::HOME_VAR, at.stamp).env(host::spawn::OWNER_VAR, host::spawn::owner_stamp());
        cmd.current_dir(at.root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let (c0, k0, t0) = (host::thread_cycles(), host::tsc(), Instant::now());
        let child = host::spawn_owned(&mut cmd, at.cpus, 0, None)?;
        let (t1, k1, c1) = (Instant::now(), host::tsc(), host::thread_cycles());
        Ok((child, t0, super::script::cpu_ms((c0, k0), (c1, k1), t1.duration_since(t0).as_secs_f64() * 1e3)))
    };
    // Windows: a program the OS would not start after all (NotFound, InvalidInput) runs under cmd, as one
    // that is no program does.
    let mut run = run;
    let (mut child, t0, launch) = match start(&run) {
        Err(e) if cfg!(windows) && matches!(run, Run::Direct { .. }) && matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput) => {
            run = Run::Shell { why: "Windows would not start its program directly".into() };
            start(&run)
        }
        r => r,
    }
    .map_err(|e| Error::msg(format!("start `{line}`: {e}")))?;
    // Windows: the OS creating the process and the command's run from its first instruction, as a
    // person's launch of it; its job, the completion port's registration and its CPUs are strobe's
    // setup while it was suspended, in neither `run.duration` nor `cli.launch`.
    let (t0, launch) = match child.spawn_span() {
        Some((from, before, after, ms)) => (from, super::script::cpu_ms(before, after, ms)),
        None => (t0, launch),
    };
    let (out, err) = child.output();
    for (tx, pipe) in pipes.into_iter().zip([out.map(|o| Box::new(o) as Pipe), err.map(|e| Box::new(e) as Pipe)]) {
        if let Some(p) = pipe {
            let _ = tx.send(p);
        }
    }
    if let Some(c) = cost {
        c.resume();
    }
    let exited = || {
        if let Some(c) = cost {
            c.pause();
        }
    };
    let usage = child.run_to_exit(at.timeout, Some(at.stamp), exited).map_err(|e| Error::msg(format!("`{line}`: {e}")))?;
    let wall = usage.exited.unwrap_or_else(Instant::now).duration_since(t0).as_secs_f64() * 1e3;
    // What it left running was ended with it, so its pipes are closed.
    for r in readers {
        let _ = r.join();
    }
    let lines = std::mem::take(&mut *lines.lock().unwrap_or_else(|e| e.into_inner()));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(at.log) {
        let _ = writeln!(f, "$ {line}\n{}", lines.join("\n"));
    }
    let tail = lines.iter().rev().take(10).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    if usage.timed_out {
        return Err(Error::refused(
            "RunIncomplete",
            format!("`{line}` ran past the case's timeout ({} s): strobe ended it and what it started", at.timeout.as_secs()),
            "give the case a longer `timeout`, or a command that ends",
        ));
    }
    if usage.code != 0 {
        return Err(Error::msg(format!("`{line}` exited with {}{}", usage.code, if tail.is_empty() { String::new() } else { format!(":\n{tail}") })));
    }
    if !usage.left.is_empty() {
        let names: Vec<String> = usage.left.iter().take(5).map(|(p, n)| format!("{n} (pid {p})")).collect();
        return Err(Error::refused(
            "RunIncomplete",
            format!(
                "`{line}` exited and left {} running: {}; {} not counted whole, so strobe ended {}",
                if usage.left.len() == 1 { "a process" } else { "processes" },
                names.join(", "),
                if usage.left.len() == 1 { "what it did is" } else { "what they did is" },
                if usage.left.len() == 1 { "it" } else { "them" }
            ),
            "make the command wait for what it starts (no background jobs or daemons)",
        ));
    }
    let mut metrics: BTreeMap<String, (f64, Option<Direction>)> = BTreeMap::new();
    for m in lines.iter().filter_map(|l| metric_line(l)) {
        let (name, value, dir) = m.map_err(|l| {
            Error::from(
                Refusal::new(ValidatorId::NoMetric, format!("`{line}` printed a metric line strobe cannot read: {l}"))
                    .fix("print `strobe:metric <name>=<number>` with an optional `lower` or `higher` after it"),
            )
        })?;
        let e = metrics.entry(name).or_insert((0.0, Some(dir)));
        e.0 += value;
        if e.1 != Some(dir) {
            e.1 = None;
        }
    }
    let output: String = lines.join("\n");
    let output = if output.len() > 64 * 1024 { output[output.floor_char_boundary(output.len() - 64 * 1024)..].to_string() } else { output };
    let (shelled, direct_note) = match run {
        Run::Shell { why } => (Some(why), None),
        Run::Direct { note, .. } => (None, note),
    };
    Ok((Ran { wall, usage, metrics, launch, shelled, direct_note }, output))
}

/// Child processes' CPU from Windows' clock ticks needs this many ticks (15.6 ms each) per run before
/// it stands beside the cycles: fewer, and one tick more or less is several percent of the run.
const TICKS_PER_RUN: f64 = 64.0;

pub(super) fn block(ctx: &mut Ctx, s: &Series, v: &Variant, pair: usize, x: &Exec) -> Result<Block> {
    if s.launch.is_object() {
        return Err(Error::usage(format!("{}: a cli target's case runs commands (run: (ctx) => ctx.exec()); a launch case needs an app", s.case)));
    }
    if x.set == Set::Attribution || x.set == Set::Memory {
        return Err(Error::usage("a cli target has no attribution or memory blocks: its commands end inside each run (profile a long-running command with strobe record pid:<n> --probes deep)"));
    }
    let target = ctx.describe["targets"][v.target_of(s)].clone();
    let root = v.root.clone().ok_or_else(|| Error::msg(format!("{}: a cli target runs in a variant's folder", v.role)))?;
    let log = store::experiment_dir(&ctx.exp)?.join("cli.log");
    let stamp = host::paths::run().join(format!("cli-{}", store::new_id("c")));
    let at = Spawn { root: &root, target: &target, cpus: ctx.pin, log: &log, stamp: &stamp, timeout: s.run_timeout() };
    let fixture = ctx.fixture_running.clone();
    let mut steps: Vec<(String, f64)> = Vec::new();
    let hooks = super::block::Hooks::default();
    let ran: RefCell<Vec<Ran>> = RefCell::new(Vec::new());
    // The CPU of every command of the run, untimed ones too (NaN once one has none): what is not the
    // target's on its CPUs over the run is other work (`cpu.foreign`).
    let own_ms: Cell<f64> = Cell::new(0.0);
    let cost = SelfCost::new(ctx.runner.pid());
    // The command lines the case's `ctx.exec` calls gave (`HarnessOverlay`).
    let lines: RefCell<Vec<Value>> = RefCell::new(Vec::new());
    let serve = |op: &str, a: &Value| -> Result<Value> {
        match op {
            "exec" => {
                if a["line"].is_string() {
                    lines.borrow_mut().push(a["line"].clone());
                }
                let line = line_for(&a["line"], v, &target)?;
                // `{fixture.url}` and the like, in a line the call or the variant gave (the target's
                // own `run` and `env` are filled with the block).
                let line = fixture.as_ref().and_then(|f| f.fill(&json!(line)).as_str().map(str::to_string)).unwrap_or(line);
                // Only a run's commands count; setup's and reset's prepare, as do a run's untimed ones.
                let counted = hooks.in_run() && !hooks.untimed_now();
                let (r, output) = exec(&line, &at, counted.then_some(&cost))?;
                let code = r.usage.code;
                if hooks.in_run() {
                    own_ms.set(own_ms.get() + r.usage.cpu_ns.map_or(f64::NAN, |c| c as f64 / 1e6));
                }
                if counted {
                    ran.borrow_mut().push(r);
                }
                Ok(json!({ "code": code, "output": output }))
            }
            "count" => {
                hooks.count(a);
                Ok(Value::Bool(true))
            }
            "phase" => hooks.phase(a),
            // Commands in an untimed stretch are not the run's (above): a cli run's totals are its
            // commands', and strobe's cost counts only while a counted one runs.
            "untimed" => hooks.untimed(a, |_, _| Ok(())),
            "call" => Err(Error::usage("ctx.call is a script() target's; a cli target runs commands with ctx.exec()")),
            "driver" => Err(Error::msg("a cli target has no page: run its commands with ctx.exec()")),
            "line" => Err(Error::usage("waitForLine reads a native app's output; ctx.exec() resolves with the command's output")),
            other => Err(Error::msg(format!("unknown runner request {other}"))),
        }
    };
    // The runner's call outlasts the command's own deadline, so the command's timeout is the one said.
    let t = s.run_timeout() + Duration::from_secs(10);
    let root_text = host::paths::plain(&root);
    ctx.runner.call("block", json!({ "case": s.case, "params": s.params, "target": v.target_of(s), "variant": v.role, "url": "", "root": root_text }), t, &serve)?;
    let mut lapped = Instant::now();
    ctx.runner.call("setup", json!({}), s.hook_timeout(), &serve).map_err(|e| crate::runner::incomplete("setup", "setup", e, "", "fix the bench file's setup"))?;
    steps.push(("setup".into(), lapped.elapsed().as_secs_f64() * 1e3));
    lapped = Instant::now();
    /// One run: its commands, its counts, its phase marks, its span and the other work on the target's
    /// CPUs meanwhile (% of one core).
    type CliRun = (Vec<Ran>, BTreeMap<String, f64>, Vec<(String, u64)>, metrics::Timed, Option<f64>);
    let mut runs: Vec<CliRun> = Vec::new();
    let mut warmup = super::block::Warmup::new(s, x);
    let wall = |r: &[Ran]| -> f64 { r.iter().map(|x| x.wall).sum() };
    let cpu = |r: &[Ran]| sum_of(r, |x| x.usage.cpu_ns.map(|c| c as f64 / 1e6));
    // Who else used the CPUs, from two snapshots outside the runs, for a block with other work.
    let procs0 = ctx.pin.map(|_| (host::now_ns(), host::all_processes()));
    while warmup.more(runs.len()) {
        hooks.begin(&format!("run:{}", runs.len()));
        own_ms.set(0.0);
        let busy = ctx.pin.and_then(|_| validity::CpuBusy::start());
        cost.begin();
        let t0 = host::now_ns();
        let r = ctx.runner.call("run", json!({ "case": s.case }), t, &serve);
        let t1 = host::now_ns();
        cost.end();
        let foreign = busy.zip(ctx.pin).and_then(|(b, cpus)| b.stop(cpus)).and_then(|b| b.other_ms(own_ms.get()));
        hooks.end();
        r.map_err(|e| crate::runner::incomplete(&format!("{} run", s.case), "run", e, "", "the run threw, timed out, or a command failed"))?;
        let done = std::mem::take(&mut *ran.borrow_mut());
        if done.is_empty() {
            return Err(Error::refused("RunIncomplete", format!("{} run: it ran no command outside ctx.untimed", s.case), "call ctx.exec() in the case's run"));
        }
        runs.push((done, hooks.counts(), hooks.take_marks(), metrics::Timed { t0, t1, untimed: hooks.untimed_stretches() }, foreign));
        if s.reset {
            ctx.runner.call("reset", json!({ "case": s.case }), t, &serve).map_err(|e| crate::runner::incomplete(&format!("{} reset", s.case), "reset", e, "", "fix the case's reset"))?;
        }
        // Without CPU (cycles that are not time), steady by wall time.
        // A command's whole tree is its own (no process of it ends unseen): nothing is lost.
        warmup.after(&runs.iter().map(|r| cpu(&r.0).unwrap_or_else(|| wall(&r.0))).collect::<Vec<_>>(), &[], &[]);
    }
    let first = warmup.first().unwrap_or(0);
    let range = first..warmup.measured().end.clamp(first, runs.len());
    let (strobe_pct, runner_pct) = cost.share(range.clone());
    steps.push(("runs".into(), lapped.elapsed().as_secs_f64() * 1e3));
    let measured = &runs[range.clone()];
    let key = |j: usize| format!("run:{}", first + j);
    let per = |f: &dyn Fn(&[Ran]) -> Option<f64>| -> BTreeMap<String, f64> { measured.iter().enumerate().filter_map(|(j, r)| Some((key(j), f(&r.0)?))).collect() };
    let first_why = |f: &dyn Fn(&host::spawn::Usage) -> Option<String>| measured.iter().flat_map(|r| r.0.iter()).find_map(|x| f(&x.usage));
    let mut extra: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut refused: BTreeMap<String, Refusal> = BTreeMap::new();
    // A reading this OS or run did not give is refused only where the case declared it: one it never
    // asked for is left out, not said.
    let declared = declared_metrics(s);
    let mut put = |name: &str, values: BTreeMap<String, f64>, why: &dyn Fn() -> Refusal| {
        if values.len() == measured.len() {
            extra.insert(name.to_string(), values);
        } else if declared.iter().any(|d| *d == name) {
            refused.insert(name.to_string(), why());
        }
    };
    put("run.duration", per(&|r| Some(wall(r))), &|| Refusal::new(ValidatorId::RunIncomplete, "run.duration: a run had no command"));
    put("cli.launch", per(&|r| sum_of(r, |x| Some(x.launch).filter(|l| l.is_finite()))), &|| {
        Refusal::new(ValidatorId::NotOnThisOs, format!("cli.launch: {}", host::cycles_refused().unwrap_or_else(|| "strobe's thread cycles over the cycle clock gave no time for a launch".into())))
    });
    // Windows: children's CPU from clock ticks stands only with enough ticks per run.
    let ticks_ms: Vec<f64> = measured.iter().map(|r| r.0.iter().map(|x| x.usage.ticks_ns as f64 / 1e6).sum()).collect();
    let tick_ms = 15.625;
    // A process the command started counts in whole ticks: under a tick it reads 0, so a run that
    // started any needs enough ticks for its count to stand.
    let started = measured.iter().any(|r| r.0.iter().any(|x| x.usage.started > 0));
    let few_ticks = started && metrics::median(&ticks_ms) < TICKS_PER_RUN * tick_ms;
    let shelled = measured.iter().flat_map(|r| r.0.iter()).find_map(|x| x.shelled.clone());
    let kept_note = measured.iter().flat_map(|r| r.0.iter()).find_map(|x| x.direct_note.clone());
    let cpu_why = || {
        let why = if let Some(why) = shelled.as_ref().filter(|_| few_ticks) {
            format!(
                "cmd.exe runs the command line ({why}), so its commands are cmd's children, whose CPU Windows counts only in 15.6 ms clock ticks; they took {} a run by that count, under the {TICKS_PER_RUN:.0} ticks a CPU time needs here (one tick more or less is several percent)",
                fmt::value(metrics::median(&ticks_ms), "ms")
            )
        } else if few_ticks {
            format!(
                "the command started other processes (a shell around it, a .cmd, a child), whose CPU Windows counts only in 15.6 ms clock ticks, and they took {} a run by that count (under {TICKS_PER_RUN:.0} ticks: one more or less, or work under a tick that reads 0, is several percent)",
                fmt::value(metrics::median(&ticks_ms), "ms")
            )
        } else {
            first_why(&|u| u.cpu_refused.clone()).unwrap_or_else(|| "this OS gave no CPU time for the command's processes".into())
        };
        let fix = if few_ticks && shelled.is_some() {
            "time each command alone (a case's ctx.exec per command, or strobe run with one command a side), or give the run more work; run.duration still compares"
        } else {
            "run the program itself (an .exe, not a .cmd or a shell line), or longer runs; run.duration still compares"
        };
        Refusal::new(ValidatorId::NotOnThisOs, why).fix(fix)
    };
    let cpu_ok = |r: &[Ran]| cpu(r).filter(|_| !few_ticks);
    put("cpu.work", per(&cpu_ok), &|| Refusal { summary: format!("cpu.work: {}", cpu_why().summary), ..cpu_why() });
    put("cpu.app.total", per(&|r| cpu_ok(r).map(|c| 100.0 * c / wall(r).max(1e-9))), &|| Refusal { summary: format!("cpu.app.total: {}", cpu_why().summary), ..cpu_why() });
    // Once per series: each series' share is its own, and one said for another series leaves this
    // one's cpu.work reading as exact.
    let label = format!("{}: ", s.label());
    if started && !few_ticks && !ctx.warnings.iter().any(|w| w.id == "TickSampledCpu" && w.text.starts_with(&label)) {
        let share = ticks_ms.iter().sum::<f64>() / measured.iter().filter_map(|r| cpu(&r.0)).sum::<f64>().max(1e-9);
        ctx.warnings.push(validity::warn(
            "TickSampledCpu",
            format!(
                "{label}{:.0}% of cpu.work is the processes the command started, which Windows counts in 15.6 ms clock ticks (sampled, not exact); the command's own process is exact",
                share * 100.0
            ),
        ));
    }
    let side = super::side_of(ctx, v).to_string();
    shell_note(ctx, s, &side, shelled.as_deref());
    if let Some(note) = kept_note.as_deref() {
        direct_note(ctx, s, note);
    }
    put("cpu.wakes", per(&|r| sum_of(r, |x| x.usage.switches.map(|n| n as f64)).map(|n| n / (wall(r) / 1e3).max(1e-9))), &|| {
        Refusal::new(ValidatorId::NotOnThisOs, "cpu.wakes: Windows keeps no context-switch count of a process that has ended (its job counts CPU and IO only)")
    });
    // Linux: a command's peak no larger than what it carried from strobe (`peak_masked`) is only an
    // upper bound of its own; the run's peak stands when a command's unmasked peak exceeds every one.
    let peak = |r: &[Ran]| -> Option<f64> {
        let bytes: Vec<(u64, bool)> = r.iter().map(|x| x.usage.peak_bytes.map(|b| (b, x.usage.peak_masked))).collect::<Option<_>>()?;
        let open = bytes.iter().filter(|b| !b.1).map(|b| b.0).max();
        let masked = bytes.iter().filter(|b| b.1).map(|b| b.0).max();
        match (open, masked) {
            (_, None) => open,
            (Some(o), Some(m)) if o > m => Some(o),
            _ => None,
        }
        .map(|b| b as f64 / (1024.0 * 1024.0))
    };
    put("memory.peak", per(&peak), &|| {
        let floor = measured.iter().flat_map(|r| r.0.iter()).filter(|x| x.usage.peak_masked).filter_map(|x| x.usage.peak_floor).max();
        match floor {
            Some(f) => Refusal::new(
                ValidatorId::NotOnThisOs,
                format!(
                    "memory.peak: Linux carries the resident set a command had before its exec ({} of strobe's, which it was started from) into its ru_maxrss, and a run's commands peaked no higher, so their own peak is at most that, not known",
                    crate::fmt::bytes(f as f64)
                ),
            )
            .fix("print the command's own peak (VmHWM in /proc/self/status) as a strobe:metric line, give it work that peaks above that, or measure memory.peak on Windows or macOS"),
            None => Refusal::new(ValidatorId::NotOnThisOs, "memory.peak: this OS gave no peak memory for the command's processes"),
        }
    });
    for (m, pick) in [("io.read", true), ("io.write", false)] {
        put(m, per(&|r| sum_of(r, |x| if pick { x.usage.read_bytes } else { x.usage.write_bytes }.map(|b| b as f64))), &|| {
            Refusal::new(ValidatorId::NotOnThisOs, format!("{m}: {}", first_why(&|u| u.io_refused.clone()).unwrap_or_else(|| "this OS gave no IO bytes for the command's processes".into())))
        });
    }
    let hooked: Vec<super::block::Hooked> = measured.iter().map(|r| (&r.1, r.2.as_slice(), &r.3)).collect();
    extra.extend(super::block::per_run_hooks(s, &key, &hooked));
    // `metric.<name>`: every command of every measured run must print it (a run with a command that
    // printed none is not 0), and every line of a name says the same direction.
    let mut names: Vec<String> = measured.iter().flat_map(|r| r.0.iter().flat_map(|x| x.metrics.keys().cloned())).collect();
    names.extend(declared_metrics(s).into_iter().filter_map(|m| m.strip_prefix("metric.").map(str::to_string)));
    names.sort();
    names.dedup();
    let mut directions: BTreeMap<String, String> = BTreeMap::new();
    for name in names {
        let m = format!("metric.{name}");
        let missed = measured.iter().filter(|r| r.0.iter().any(|x| !x.metrics.contains_key(&name))).count();
        let dirs: std::collections::BTreeSet<Option<Direction>> = measured.iter().flat_map(|r| r.0.iter().filter_map(|x| x.metrics.get(&name).map(|e| e.1))).collect();
        if missed > 0 {
            let why = format!("{m}: in {missed} of {} measured runs a command printed no `strobe:metric {name}=<value>` line", measured.len());
            refused.insert(m, Refusal::new(ValidatorId::NoMetric, why).fix("print the line from every command the run executes"));
        } else if dirs.len() != 1 || dirs.contains(&None) {
            refused.insert(m.clone(), Refusal::new(ValidatorId::NoMetric, format!("{m}: its lines disagree on its direction (lower and higher)")).fix("print one direction for a name"));
        } else {
            let dir = dirs.into_iter().next().flatten().unwrap_or(Direction::Lower);
            extra.insert(m.clone(), measured.iter().enumerate().map(|(j, r)| (key(j), r.0.iter().map(|x| x.metrics[&name].0).sum())).collect());
            directions.insert(m, dir.name().to_string());
        }
    }
    let mut values = metrics::block_values(&metrics::BlockData::default(), "run", &extra);
    super::untimed_values(&measured.iter().map(|r| &r.3).collect::<Vec<_>>(), &mut values);
    // Other work on the target's CPUs during the runs: kept and flagged (`Background`, SPEC §10.2).
    let foreign: Vec<f64> = measured.iter().filter_map(|r| r.4).collect();
    let mut background = Vec::new();
    if !foreign.is_empty() {
        let most = foreign.iter().copied().fold(0.0, f64::max);
        values.insert("cpu.foreign".into(), metrics::median(&foreign));
        values.insert("cpu.foreign.max".into(), most);
        if most >= BACKGROUND_PCT
            && let Some(p0) = &procs0
        {
            background = busiest_others(p0, &(host::now_ns(), host::all_processes()));
        }
    }
    warmup.values(&mut values);
    for m in declared_metrics(s) {
        if values.contains_key(m) || refused.contains_key(m) || m.starts_with("count.") || m.starts_with("phase.") {
            continue;
        }
        let r = if probe::wakes::COUNTER_METRICS.contains(&m.as_str()) {
            Refusal::new(ValidatorId::NoCounter, format!("{m}: a command's counters are not read (it ends inside the run)"))
        } else {
            Refusal::new(
                ValidatorId::NotForTarget,
                format!("{m}: a cli target is measured from outside its commands (cpu.work, run.duration, cli.launch, cpu.wakes, memory.peak, io.*, metric.<name>, and cpu.foreign when pinned)"),
            )
        };
        refused.insert(m.clone(), r);
    }
    refuse_not_on_this_os(s, &mut values, &mut refused);
    let _ = ctx.runner.call("close", json!({}), t, &serve);
    // A harness file the case's own command lines run, over the variant's own copy.
    let ran: Vec<PathBuf> = lines.take().iter().flat_map(|l| super::line_files(Path::new(""), l)).collect();
    let notes = super::harness_overlay(v, &ran, v.target_of(s), &ctx.harness.from);
    super::say_overlay(ctx, notes);
    Ok(Block {
        variant: v.role.to_string(),
        pair,
        values,
        run_ms: measured.iter().map(|r| wall(&r.0)).collect(),
        warmup: first,
        speed_ratio: None,
        nodes: Vec::new(),
        recording: None,
        phases: BTreeMap::new(),
        frames: Vec::new(),
        intervals: Vec::new(),
        steps,
        background,
        leak: None,
        refused,
        locked: false,
        display: None,
        screen: None,
        reserve: None,
        shown: None,
        overhead: Some(Overhead { strobe_pct, runner_pct, ..Overhead::default() }),
        process_set: None,
        fixture_connected: None,
        client: None,
        frame: false,
        browser: None,
        spread: super::run_spread(x, &extra),
        directions,
        shelled: shelled.is_some(),
        ..Block::default()
    })
}
