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
        .filter(|e| e.starts_with('.') && e[1..].bytes().all(|c| c.is_ascii_alphanumeric()))
        .find_map(|e| {
            let mut path = p.as_os_str().to_owned();
            path.push(e);
            let path = PathBuf::from(path);
            path.is_file().then_some(path)
        })
        .or_else(|| p.is_file().then(|| p.into()))
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

fn token(line: &str, start: usize) -> std::ops::Range<usize> {
    let s = &line[start..];
    let s = s.trim_start_matches([' ', '\t', '(']);
    let at = line.len() - s.len();
    let len = if let Some(q) = s.strip_prefix('"') { q.find('"').map_or(s.len(), |end| end + 2) } else { s.find([' ', '\t', '&', '|', '<', '>', ')']).unwrap_or(s.len()) };
    at..at + len
}
fn name<'a>(line: &'a str, r: &std::ops::Range<usize>) -> &'a str {
    &line[r.clone()]
}
fn explicit(program: &str) -> bool {
    matches!(Path::new(program).file_stem().unwrap_or_default().to_string_lossy().to_ascii_lowercase().as_str(), "cmd" | "pwsh" | "powershell" | "sh" | "bash" | "dash" | "ksh" | "zsh")
}
fn builtin(program: &str) -> bool {
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
            | "echo"
            | "endlocal"
            | "erase"
            | "exit"
            | "for"
            | "ftype"
            | "goto"
            | "if"
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
            Err("a batch script re-parses its arguments (`%*`): wrap the argument in double quotes, or use the executable directly for literal percent signs".into())
        } else {
            Ok(())
        }
    }
}
pub(super) fn syntax(line: &str) -> std::result::Result<Shape, String> {
    let line = line.trim_matches([' ', '\t']);
    let bytes = line.as_bytes();
    let mut s = Shape { heads: vec![token(line, 0)], op: line.starts_with('('), ..Default::default() };
    let first = name(line, &s.heads[0]).trim_matches('"');
    // An explicitly named shell owns its payload, including nested quotes (cmd /c "a "x" && b").
    let already = !s.op && explicit(first);
    let (mut quoted, mut slashes, mut i) = (false, 0usize, 0);
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' if slashes % 2 == 1 => s.escaped = true,
            b'"' => quoted = !quoted,
            b'\n' | b'\r' if !quoted || already => return Err(NEWLINE.into()),
            b'\n' | b'\r' => s.newline = true,
            0..=8 | 11..=12 | 14..=31 => return Err("control character in command line: pass the data in a file".into()),
            b'\'' if !quoted && !matches!(first.to_ascii_lowercase().as_str(), "pwsh" | "powershell") => {
                return Err("single quotes `'…'` do not quote arguments on Windows: use double quotes or `pwsh -NoProfile -c \"…\"`".into());
            }
            _ if quoted || already => {}
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
                s.heads.push(token(line, i + 1));
            }
            b'<' | b'>' => s.op = true,
            b'^' => s.caret = true,
            b'%' => s.percent = true,
            _ => {}
        }
        slashes = if b == b'\\' { slashes + 1 } else { 0 };
        i += 1;
    }
    if quoted {
        return Err("unclosed double quote `\"` in command line: close the quoted argument".into());
    }
    for r in &s.heads {
        let raw = name(line, r);
        let p = raw.trim_matches('"');
        let ext = extension(Path::new(p));
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
        }
    }
    Ok(s)
}

pub(super) fn plan(line: &str, root: &Path, env: &Value) -> Result<Planned> {
    let line = line.trim_matches([' ', '\t']);
    let shape = syntax(line).map_err(Error::usage)?;
    let mut text = line.to_string();
    let mut evidence = Evidence::default();
    let mut why = "shell operators";
    for r in shape.heads.iter().rev() {
        let program = name(line, r).trim_matches('"');
        if program.is_empty() {
            continue;
        }
        let Some(hit) = resolve(program, root, env)? else {
            if builtin(program) {
                why = "cmd builtin";
                continue;
            }
            if shape.op {
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
        if batch {
            why = "batch script";
        }
        let path = hit
            .program
            .to_str()
            .filter(|p| !p.contains(['\r', '\n', '%', '"']))
            .ok_or_else(|| Error::usage("cmd.exe cannot quote the resolved executable path losslessly: use a different PATH directory or a script"))?;
        text.replace_range(r.clone(), &format!("\"{path}\""));
        if hit.shadow.is_some() {
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
    evidence.program = sys;
    evidence.shell = Some(format!("cmd.exe ({why})"));
    Ok(Planned { cmd, how: evidence })
}
