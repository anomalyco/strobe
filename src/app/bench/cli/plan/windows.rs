use super::*;
use std::os::windows::process::CommandExt;

fn extension(p: &Path) -> String {
    p.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase()
}
pub(super) fn find(p: &Path, env: &Value) -> Option<PathBuf> {
    let ext = extension(p);
    if !ext.is_empty() && p.is_file() {
        return Some(p.into());
    }
    if matches!(ext.as_str(), "exe" | "com" | "bat" | "cmd" | "ps1" | "js" | "msc" | "vbs" | "wsf") {
        return None;
    }
    env_var(env, "PATHEXT")
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
        .to_string_lossy()
        .split(';')
        .map(str::trim)
        .filter(|e| e.starts_with('.') && e[1..].bytes().all(|c| c.is_ascii_alphanumeric()))
        .find_map(|e| {
            let mut path = p.as_os_str().to_owned();
            path.push(e);
            let path = PathBuf::from(path);
            path.is_file().then_some(path)
        })
}

#[derive(Default)]
pub(super) struct Shape {
    // Offsets only: the real shell, not this scan, interprets operators and arguments.
    heads: Vec<std::ops::Range<usize>>,
    op: bool,
    caret: bool,
    percent: bool,
    escaped: bool,
    newline: bool,
}

fn token(line: &str, start: usize, grouped: bool) -> std::ops::Range<usize> {
    let s = &line[start..];
    let s = s.trim_start_matches([' ', '\t', '(']);
    let at = line.len() - s.len();
    let group = grouped || s.len() != line[start..].trim_start_matches([' ', '\t']).len();
    let len = if let Some(q) = s.strip_prefix('"') {
        q.find('"').map_or(s.len(), |end| end + 2)
    } else {
        s.find(|c| matches!(c, ' ' | '\t' | '&' | '|' | '<' | '>') || group && c == ')').unwrap_or(s.len())
    };
    at..at + len
}
fn name<'a>(line: &'a str, r: &std::ops::Range<usize>) -> &'a str {
    &line[r.clone()]
}
fn explicit(program: &str) -> bool {
    matches!(Path::new(program).file_stem().unwrap_or_default().to_string_lossy().to_ascii_lowercase().as_str(), "cmd" | "pwsh" | "powershell" | "sh" | "bash" | "dash" | "ksh" | "zsh")
}
fn builtin(program: &str) -> bool {
    if program.eq_ignore_ascii_case("echo.") || program.get(..5).is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "echo:" | "echo/")) {
        return true;
    }
    matches!(
        program.to_ascii_lowercase().as_str(),
        "assoc"
            | "break"
            | "call"
            | "cd"
            | "chdir"
            | "cls"
            | "color"
            | "copy"
            | "date"
            | "del"
            | "dir"
            | "dpath"
            | "echo"
            | "endlocal"
            | "erase"
            | "exit"
            | "for"
            | "ftype"
            | "goto"
            | "if"
            | "keys"
            | "md"
            | "mkdir"
            | "mklink"
            | "move"
            | "path"
            | "pause"
            | "popd"
            | "prompt"
            | "pushd"
            | "rd"
            | "rem"
            | "ren"
            | "rename"
            | "rmdir"
            | "set"
            | "setlocal"
            | "shift"
            | "start"
            | "time"
            | "title"
            | "type"
            | "ver"
            | "verify"
            | "vol"
    )
}
fn ps1(program: &str) -> String {
    format!("`{program}` is a PowerShell script: invoke its interpreter with `pwsh -NoProfile -File \"{program}\" …` or `powershell -NoProfile -File \"{program}\" …`")
}
const NEWLINE: &str = "a newline in a Windows command line: cmd.exe can stop at the first newline; join commands with `&&`, or call ctx.exec() once per command";
impl Shape {
    fn cmd_rules(&self, batch: bool) -> std::result::Result<(), String> {
        if self.escaped {
            Err("this line runs through cmd.exe, which does not read `\\\"` as an escaped quote: invoke the .exe directly without shell operators, or pass the value via a file".into())
        } else if self.caret {
            Err("this line runs through cmd.exe, which strips unquoted `^`: wrap the argument in double quotes (`\"HEAD^\"`, `\"a|b\"`)".into())
        } else if self.newline {
            Err(NEWLINE.into())
        } else if batch && self.percent {
            Err("a batch script re-parses its arguments (`%*`): even double quotes permit percent expansion; invoke the underlying executable directly for a literal percent sign".into())
        } else {
            Ok(())
        }
    }
}
pub(super) fn syntax(line: &str) -> std::result::Result<Shape, String> {
    let line = line.trim_matches([' ', '\t']);
    let bytes = line.as_bytes();
    let mut s = Shape { heads: vec![token(line, 0, false)], op: line.starts_with('('), ..Default::default() };
    let first = name(line, &s.heads[0]).trim_matches('"');
    if first.is_empty() && !line.starts_with(['(', '>', '<']) {
        return Err("missing program name: give the executable or shell command to measure".into());
    }
    // An explicitly named shell owns its payload, including nested quotes (cmd /c "a "x" && b").
    let already = !s.op && explicit(first) && Path::new(first).file_stem().is_some_and(|n| n.eq_ignore_ascii_case("cmd"));
    let (mut quoted, mut slashes, mut i, mut depth) = (false, 0usize, 0, 0usize);
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' if !already && slashes % 2 == 1 => s.escaped = true,
            b'"' => quoted = !quoted,
            b'\n' | b'\r' if !quoted || already => return Err(NEWLINE.into()),
            b'\n' | b'\r' => s.newline = true,
            0..=8 | 11..=12 | 14..=31 => return Err("control character in command line: pass the data in a file".into()),
            b'\'' if !already && !quoted => {
                return Err("single quotes `'…'` do not quote arguments on Windows: use double quotes or `pwsh -NoProfile -c \"…\"`".into());
            }
            b'%' => s.percent = true,
            _ if quoted || already => {}
            b'(' if s.op => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b';' => return Err("`;` outside double quotes: use `&&` between commands, or wrap `;`-separated arguments in double quotes".into()),
            b'&' if i > 0 && matches!(bytes[i - 1], b'>' | b'<') => s.op = true,
            b'&' if bytes.get(i + 1) != Some(&b'&') => {
                return Err(
                    "a single `&` outside quotes splits the line in cmd: wrap the argument in double quotes, use `&&` to run commands in turn, or run a script that waits for parallel jobs".into()
                );
            }
            b'&' | b'|' => {
                s.op = true;
                if bytes.get(i + 1) == Some(&b) {
                    i += 1;
                }
                s.heads.push(token(line, i + 1, depth > 0));
            }
            b'<' | b'>' => s.op = true,
            b'^' => s.caret = true,
            _ => {}
        }
        slashes = if b == b'\\' { slashes + 1 } else { 0 };
        i += 1;
    }
    if quoted {
        return Err("unclosed double quote `\"` in command line: close the quoted argument".into());
    }
    if s.op && !line.starts_with('(') && explicit(first) {
        return Err("put the operators inside the explicitly named shell's command string, or run the commands in separate ctx.exec() calls".into());
    }
    for r in &s.heads {
        let raw = name(line, r);
        let p = raw.trim_matches('"');
        let ext = extension(Path::new(p));
        if raw.starts_with('"') && line[r.end..].starts_with(|c: char| !matches!(c, ' ' | '\t' | ')' | '&' | '|' | '<' | '>')) {
            return Err("put the complete program path in one pair of double quotes, then separate its arguments with a space".into());
        }
        if p.is_empty() || p.bytes().all(|b| b.is_ascii_digit()) && line[r.end..].starts_with(['<', '>']) || matches!(p.to_ascii_lowercase().as_str(), "if" | "for" | "call") {
            return Err("move a leading redirection after the command (`tool > out`), drop `call` (`a.cmd && b`), or run `if`/`for` in an explicit shell (`cmd /c \"…\"`, `pwsh -NoProfile -c \"…\"`) or script file".into());
        }
        if let Some(k) = assignment(raw) {
            return Err(format!("`{k}=…` before a command is POSIX sh syntax: set `{k}` in the target env, or write `set \"{k}=…\" && <command>`"));
        }
        if ext == "ps1" {
            return Err(ps1(p));
        }
        if p.eq_ignore_ascii_case("start")
            && !line[r.end..].trim_start().split_ascii_whitespace().take_while(|w| w.starts_with('/')).any(|w| w.eq_ignore_ascii_case("/wait") || w.eq_ignore_ascii_case("/w"))
        {
            return Err("`start` launches a command in the background: run it directly, or use `start /wait …`".into());
        }
        let batch = matches!(ext.as_str(), "cmd" | "bat");
        if s.op || batch {
            s.cmd_rules(batch)?;
            cmd_head(raw)?;
        }
    }
    Ok(s)
}

fn cmd_head(p: &str) -> std::result::Result<(), String> {
    if !p.starts_with('"') && !builtin(p) {
        if p.split_once(['.', ':', '/', '\\', '+', '=', ',', ';', '[', ']', '(']).is_some_and(|(stem, _)| builtin(stem)) {
            return Err(format!("quote `\"{p}\"` on a cmd.exe line so cmd does not run its builtin instead"));
        }
        if p.contains('/') {
            return Err(format!("cmd does not reliably accept `/` in an unquoted program path: write `{}`", p.replace('/', "\\")));
        }
    }
    Ok(())
}

pub(super) fn plan(text: &str, root: &Path, checkout: &Path, env: &Value, shape: Shape) -> Result<Planned> {
    let line = text.trim_matches([' ', '\t']);
    let mut evidence = Evidence::default();
    let mut why = "shell operators";
    let mut cwd = Some(root);
    for r in &shape.heads {
        let program = name(line, r).trim_matches('"');
        if program.is_empty() {
            continue;
        }
        if !name(line, r).starts_with('"') && builtin(program) {
            if matches!(program.to_ascii_lowercase().as_str(), "cd" | "chdir" | "pushd" | "popd") {
                cwd = None;
            }
            why = "cmd builtin";
            continue;
        }
        let path = program.contains(['/', '\\', ':']);
        let written_ext = extension(Path::new(program));
        if shape.op && path && matches!(written_ext.as_str(), "exe" | "com" | "bat" | "cmd") {
            // The shell may first change folder or build this file. The extension fixes its type.
            continue;
        }
        if shape.op && path && (cwd.is_none() || find(&root.join(program), env).is_none()) {
            return Err(Error::usage(format!("give `{program}` its executable or batch extension on a cmd.exe line, or invoke its interpreter explicitly")));
        }
        let hit = if shape.op && !path { bare(program, root, cwd.is_some(), checkout, env)? } else { resolve(program, root, checkout, env)? };
        let Some(hit) = hit else {
            if shape.op && (written_ext.is_empty() || matches!(written_ext.as_str(), "exe" | "com" | "bat" | "cmd")) {
                continue;
            }
            return Err(Error::msg(format!("`{program}` is not on PATH: install it or give its executable path")));
        };
        let ext = extension(&hit.program);
        let batch = matches!(ext.as_str(), "bat" | "cmd");
        if !batch && !matches!(ext.as_str(), "exe" | "com") {
            return Err(Error::usage(if ext == "ps1" { ps1(program) } else { format!("`{program}` is not a native executable or batch script: invoke its interpreter explicitly") }));
        }
        if !shape.op && !batch {
            let mut cmd = Command::new(&hit.program);
            let args = line[r.end..].trim_start_matches([' ', '\t']);
            if !args.is_empty() {
                cmd.raw_arg(args);
            }
            let shell = explicit(program).then(|| format!("{program} (explicit shell)"));
            let percent = line.match_indices('%').any(|(at, _)| line[at + 1..].split_once('%').is_some_and(|(k, _)| !k.is_empty() && !k.contains('=') && env_var(env, k).is_some()));
            let literal = (shell.is_none() && (shape.caret || percent)).then(|| "started directly without cmd.exe, so carets and %NAME% are passed literally; use cmd /c for expansion".into());
            return Ok(Planned { cmd, how: Evidence { shell, literal, ..hit } });
        }
        shape.cmd_rules(batch).map_err(Error::usage)?;
        cmd_head(name(line, r)).map_err(Error::usage)?;
        if batch {
            why = "batch script";
        }
        // Never substitute the resolved path into cmd source. In a cwd collision cmd may prefer
        // the local executable; neither a PATH claim nor rewriting the command would be honest.
        if !path && cwd.and_then(|r| find(&r.join(program), env)).is_some() && hit.shadow.is_some() {
            return Err(Error::usage(format!("cmd may run the local `{program}` instead of PATH's copy: give its explicit path")));
        }
        if batch && cwd.is_some() && evidence.program.as_os_str().is_empty() {
            evidence.program = hit.program;
        }
        if evidence.shadow.is_none() {
            evidence.shadow = hit.shadow;
        }
    }
    shape.cmd_rules(false).map_err(Error::usage)?;
    let sys = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| Error::msg("cannot locate the system cmd.exe: SystemRoot is not absolute"))?
        .join("System32")
        .join("cmd.exe");
    let mut cmd = Command::new(&sys);
    cmd.raw_arg(format!("/d /v:off /s /c \"{text}\""));
    evidence.shell = Some(format!("cmd.exe ({why})"));
    Ok(Planned { cmd, how: evidence })
}
