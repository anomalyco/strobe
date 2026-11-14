use super::*;
use std::os::unix::fs::PermissionsExt;

pub(super) fn find(p: &Path, _: &Value) -> Option<PathBuf> {
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).then(|| p.to_owned())
}

pub(super) struct Shape {
    starts: Vec<usize>,
}

/// Friendly diagnostics only. At opaque syntax stop inspecting: no guess about the quoting inside
/// a substitution, a here document, or another language can make a direct launch safe.
pub(super) fn syntax(line: &str) -> std::result::Result<Shape, String> {
    let b = line.as_bytes();
    let mut starts = vec![0];
    let (mut i, mut quote, mut start, mut boundary) = (0, 0, true, true);
    while i < b.len() {
        let ch = b[i];
        if quote != b'\'' && (ch == b'`' || b[i..].starts_with(b"$(") || b[i..].starts_with(b"${") || b[i..].starts_with(b"$'")) {
            break;
        }
        if ch == b'\\' && quote != b'\'' {
            i += 2;
            start = false;
            boundary = false;
            continue;
        }
        if quote != 0 {
            if quote == ch {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if boundary && ch == b'#' {
            i += b[i..].iter().position(|c| *c == b'\n').unwrap_or(b.len() - i);
            continue;
        }
        if b[i..].starts_with(b"<<") {
            break;
        }
        if start {
            let word = line.get(i..).unwrap_or("").split([' ', '\t', '\n', ';', '&', '|']).next();
            match word {
                Some("[[") => return Err("`[[` is a bash keyword not in POSIX sh: write `[ … ]`, or use `bash -c '…'`".into()),
                Some("source") => return Err("`source` is not in POSIX sh: write `. <file>`, or use `bash -c '…'`".into()),
                _ => {}
            }
        }
        match ch {
            b'\''|b'"'=>{quote=ch;start=false;boundary=false;},
            b' '|b'\t'=>boundary=true,
            b';'|b'\n'|b'('=>{start=true;boundary=true; starts.push(i+1);},
            b'|' if b.get(i+1)==Some(&b'&')=>return Err("`|&` is bash's stderr pipe: write `2>&1 |`".into()),
            b'|'|b'&' if b.get(i+1)==Some(&ch)=>{i+=1; start=true;boundary=true;starts.push(i+1);},
            b'|'=>{start=true;boundary=true;starts.push(i+1);},
            b'&' if b[i..].starts_with(b"&>>")=>return Err("`&>>` is bash's append redirection: write `>> file 2>&1`".into()),
            b'&' if b[i..].starts_with(b"&>")=>return Err("`&>` is bash's redirection: write `> file 2>&1`".into()),
            b'&' if i==0 || !matches!(b[i-1],b'<'|b'>')=>return Err("a single `&` outside quotes runs the preceding command in the background: quote the argument if `&` belongs to a URL or string, use `&&` or `;` to run commands in turn, or wait for parallel jobs (`sh -c '… & … & wait'`)".into()),
            _=>{start=false;boundary=false;}
        }
        i += 1;
    }
    Ok(Shape { starts })
}

/// Consume one allowlisted shell word, keeping its end in the original string. In particular,
/// quotes never make an expansion or substitution in double quotes into a literal.
fn word(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut q = '\0';
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match (q, c) {
            (_, '\r' | '\n' | '\0') => return None,
            ('\0', ' ' | '\t') => return Some((out, &s[i..])),
            ('\0', '\'' | '"') => q = c,
            ('\0', c) if c.is_ascii_alphanumeric() || "_-./:@+=,%^".contains(c) => out.push(c),
            ('"', '$') if it.peek().is_some_and(|(_, ch)| *ch == '\'') => out.push(c),
            ('\0', _) | ('"', '$' | '`') => return None,
            ('"', '\\') => match it.next()?.1 {
                c @ ('\\' | '"') => out.push(c),
                '\'' => out.push_str("\\'"),
                _ => return None,
            },
            (q0, c) if c == q0 => q = '\0',
            (_, c) => out.push(c),
        }
    }
    (q == '\0').then_some((out, ""))
}

fn simple(line: &str) -> Option<(String, &str)> {
    let line = line.trim_start_matches([' ', '\t']);
    let (mut name, mut rest) = word(line)?;
    if name == "exec" {
        (name, rest) = word(rest.trim_start_matches([' ', '\t']))?;
        if name == "--" {
            (name, rest) = word(rest.trim_start_matches([' ', '\t']))?;
        }
        if name.starts_with('-') {
            return None;
        }
    }
    if name.is_empty() || assignment(&name).is_some() || builtin(&name) {
        return None;
    }
    let mut tail = rest.trim_start_matches([' ', '\t']);
    while !tail.is_empty() {
        tail = word(tail)?.1.trim_start_matches([' ', '\t']);
    }
    Some((name, rest))
}

fn builtin(name: &str) -> bool {
    const BUILTINS: &[&str] = &[
        "if", "then", "else", "elif", "fi", "for", "do", "done", "while", "until", "case", "esac", "in", "{", "!", "[", "cd", "export", "set", "unset", ".", "eval", "exec", "test", "alias",
        "unalias", "ulimit", "umask", "time", ":", "command", "type", "hash", "read", "wait", "exit", "shift", "trap", "times", "getopts", "readonly", "local", "return", "echo", "pwd", "printf",
        "kill", "true", "false", "break", "continue", "bg", "fg", "jobs",
    ];
    BUILTINS.contains(&name)
}

fn explicit(program: &str) -> bool {
    matches!(Path::new(program).file_name().and_then(|s| s.to_str()), Some("sh" | "bash" | "dash" | "zsh" | "ksh" | "pwsh" | "powershell" | "cmd"))
}

pub(super) fn plan(line: &str, root: &Path, checkout: &Path, env: &Value, shape: Shape) -> Result<Planned> {
    let mut cmd = Command::new("/bin/sh");
    if let Some((program, rest)) = simple(line) {
        let mut how = resolve(&program, root, checkout, env)?.ok_or_else(|| Error::msg(format!("`{program}` is not on PATH: install it or give its executable path")))?;
        if explicit(&program) {
            cmd = Command::new(&how.program);
            let mut tail = rest.trim_start_matches([' ', '\t']);
            while !tail.is_empty() {
                let (arg, r) = word(tail).expect("simple checked every argument");
                cmd.arg(arg);
                tail = r.trim_start_matches([' ', '\t']);
            }
            how.shell = Some(format!("{program} (explicit shell)"));
            return Ok(Planned { cmd, how });
        }
        // Pass the exact path out-of-band. No quoting of a filesystem name and no second PATH search.
        cmd.arg("-c").arg(format!("exec \"$0\"{rest}")).arg(&how.program);
        Ok(Planned { cmd, how })
    } else {
        let mut how = Evidence { shell: Some("POSIX sh (shell syntax or builtin)".into()), ..Default::default() };
        for at in shape.starts {
            let mut tail = &line[at..];
            while let Some((p, rest)) = word(tail.trim_start_matches([' ', '\t'])) {
                if assignment(&p).is_some() {
                    tail = rest;
                    continue;
                }
                if explicit(&p) && at == 0 {
                    return Err(Error::usage(format!("`{p}` has outer shell syntax: single-quote the script so only `{p}` expands it, or put it in a script file")));
                }
                if !p.is_empty()
                    && !builtin(&p)
                    && let Some(hit) = resolve(&p, root, checkout, env)?
                    && hit.shadow.is_some()
                {
                    how.shadow = hit.shadow;
                }
                break;
            }
        }
        cmd.args(["-c", line]);
        Ok(Planned { cmd, how })
    }
}
