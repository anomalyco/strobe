//! Decide what to start before opening pipes or reading clocks. Shell programs own their syntax;
//! the planner only needs proof for the narrower case where no shell interpretation is needed.
use super::*;
use std::ffi::OsString;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
use windows as os;

pub(super) struct Planned {
    pub cmd: Command,
    pub how: Evidence,
}

#[derive(Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Evidence {
    pub program: PathBuf,
    pub shell: Option<String>,
    pub literal: Option<String>,
    pub shadow: Option<String>,
}

fn syntax(line: &str) -> Result<os::Shape> {
    if line.trim().is_empty() || line.contains('\0') {
        return Err(Error::usage("give a nonempty command line without a NUL character"));
    }
    os::syntax(line).map_err(|why| Error::usage(format!("`{line}`: {why}")))
}

pub(in super::super) fn early(line: &str) -> Result<()> {
    syntax(line).map(|_| ())
}

pub(super) fn exec(line: &str, root: &Path, env: &Value) -> Result<Planned> {
    os::plan(line, root, env, syntax(line)?).map_err(|e| Error { message: format!("`{line}`: {}", e.message), ..e })
}

fn env_var(env: &Value, name: &str) -> Option<OsString> {
    env.as_object()
        .into_iter()
        .flatten()
        .find(|(key, _)| if cfg!(windows) { key.eq_ignore_ascii_case(name) } else { key.as_str() == name })
        .map(|(_, v)| OsString::from(v.as_str().unwrap_or("")))
        .or_else(|| std::env::var_os(name))
}

fn assignment(word: &str) -> Option<&str> {
    let (name, _) = word.split_once('=')?;
    (name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')).then_some(name)
}

fn resolve(program: &str, root: &Path, env: &Value) -> Result<Option<Evidence>> {
    if program.contains('/') || cfg!(windows) && program.contains(['\\', ':']) {
        return os::find(&root.join(program), env)
            .map(|program| Some(Evidence { program, ..Default::default() }))
            .ok_or_else(|| Error::usage(format!("`{program}` not found or not executable in {}; check the path and build the program first", root.display())));
    }
    let path = env_var(env, "PATH").unwrap_or_default();
    let hit = std::env::split_paths(&path).filter(|p| p.is_absolute()).find_map(|p| os::find(&p.join(program), env));
    let local = os::find(&root.join(program), env);
    let rel = |p: &Path| format!(".{}{}", std::path::MAIN_SEPARATOR, p.file_name().unwrap_or_default().to_string_lossy());
    match (hit, local) {
        (Some(hit), local) => {
            let shadow = local
                .filter(|p| {
                    if p == &hit {
                        return false;
                    }
                    let same = std::fs::canonicalize(p).ok().and_then(|local| std::fs::File::open(&hit).ok().and_then(|f| host::disk::same_file(&f, &local)));
                    same != Some(true)
                })
                .map(|p| format!("bare `{program}` ran `{}` from PATH, not `{}` in the variant's folder: write `{}` to measure the variant's build", hit.display(), rel(&p), rel(&p)));
            Ok(Some(Evidence { program: hit, shadow, ..Default::default() }))
        }
        (None, Some(p)) => Err(Error::usage(format!("`{program}` is not on PATH; bare names look on PATH only: write `{}` to run the variant's build", rel(&p)))),
        (None, None) => Ok(None),
    }
}
