//! Where strobe keeps its state (SPEC §4.2), and how it writes files there: whole or not at all.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The state folder, created user-only (`0700` on Linux and macOS; `%LOCALAPPDATA%` is the user's
/// own on Windows). Traces and heap snapshots in it can hold what the target showed (SPEC rule 9).
pub fn state_dir() -> PathBuf {
    let dir = state_path();
    owner_only_dir(&dir);
    dir
}

/// Where the state folder is, without making it or changing its mode (a read-only command's).
pub fn state_path() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(super::home).join("strobe")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| super::home().join(".local/share")).join("strobe")
    }
}

pub fn sub(name: &str) -> PathBuf {
    let dir = state_dir().join(name);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn recordings() -> PathBuf {
    sub("recordings")
}

pub fn experiments() -> PathBuf {
    sub("experiments")
}

pub fn engine() -> PathBuf {
    sub("engine")
}

/// Temporary files: query scripts, run folders, tokens. User-only like the state folder.
pub fn run() -> PathBuf {
    let dir = state_dir().join("run");
    owner_only_dir(&dir);
    dir
}

pub fn sdk() -> PathBuf {
    sub("sdk")
}

/// A path as a person types it: on Windows without the verbatim prefix a canonical path carries
/// (`\\?\C:\x` → `C:\x`, `\\?\UNC\host\share` → `\\host\share`), which takes no `/` and no `..`.
pub fn plain(p: &Path) -> String {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(r) if r.get(..4).is_some_and(|u| u.eq_ignore_ascii_case(r"UNC\")) => format!(r"\\{}", &r[4..]),
        Some(r) if r.as_bytes().get(1) == Some(&b':') => r.to_string(),
        _ => s,
    }
}

/// What lies around a base's and a head's folders that differs for a program started in each before its
/// own code runs (`VariantPlacement`, a warning): another kind of filesystem; a `package.json`,
/// `node_modules`, `tsconfig.json` or `bunfig.toml` in a folder above one of them alone (a runtime looks
/// up for them); one under the temporary folder and the other not, or a folder above one alone with 500
/// entries or more where those above the other hold under 100 each (a runtime that lists the folders it
/// starts in reads them). None when nothing does.
pub fn placement_diff(base: &Path, head: &Path) -> Option<String> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (base, head) = (canon(base), canon(head));
    let mut found: Vec<String> = Vec::new();
    #[cfg(windows)]
    let volume = |p: &Path| super::win::volume(p);
    #[cfg(not(windows))]
    let volume = |p: &Path| super::os::volume(p);
    // Another kind of filesystem (NTFS and ReFS, ext4 and tmpfs or 9p): another volume of the same kind
    // costs nothing measurable (adais: `bun -e 0` from C: and D:, both NTFS, the same within 1 ms).
    if let (Some((va, fa)), Some((vb, fb))) = (volume(&base), volume(&head))
        && fa != fb
    {
        found.push(format!("they are on different filesystems ({fa} on {va}, {fb} on {vb})"));
    }
    // The folders above each that are not above the other: what one side meets and the other does not.
    let above = |p: &Path, other: &Path| -> Vec<PathBuf> { p.ancestors().skip(1).filter(|a| !other.starts_with(a)).map(Path::to_path_buf).collect() };
    let sides = [("base", above(&base, &head)), ("head", above(&head, &base))];
    for (role, dirs) in &sides {
        let marks: Vec<String> =
            dirs.iter().flat_map(|d| ["package.json", "node_modules", "tsconfig.json", "bunfig.toml"].into_iter().map(move |m| d.join(m))).filter(|f| f.exists()).map(|f| plain(&f)).take(3).collect();
        if !marks.is_empty() {
            found.push(format!("{} lies above {role}'s folder alone", marks.join(", ")));
        }
    }
    let tmp = canon(&std::env::temp_dir());
    match (base.starts_with(&tmp), head.starts_with(&tmp)) {
        (true, false) => found.push(format!("base's folder is under the temporary folder {}", plain(&tmp))),
        (false, true) => found.push(format!("head's folder is under the temporary folder {}", plain(&tmp))),
        _ => {
            // Entries counted up to 500: the largest folder above each side alone.
            let widest = |dirs: &[PathBuf]| dirs.iter().map(|d| (std::fs::read_dir(d).map(|r| r.take(500).count()).unwrap_or(0), d.clone())).max_by_key(|e| e.0);
            let (a, b) = (widest(&sides[0].1), widest(&sides[1].1));
            let (na, nb) = (a.as_ref().map_or(0, |e| e.0), b.as_ref().map_or(0, |e| e.0));
            if let Some((role, (_, d))) = match (na >= 500 && nb < 100, nb >= 500 && na < 100) {
                (true, _) => a.map(|e| ("base", e)),
                (_, true) => b.map(|e| ("head", e)),
                _ => None,
            } {
                found.push(format!("{} above {role}'s folder alone holds 500 entries or more, where those above the other's hold under 100", plain(&d)));
            }
        }
    }
    (!found.is_empty()).then(|| found.join("; "))
}

/// Creates `dir` (its parents as the umask says) and makes it the user's alone on Linux and macOS:
/// created `0700`, and an existing one (an older strobe made it with the umask's mode) that others
/// may read is set to `0700` when it is seen.
pub fn owner_only_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::DirBuilder::new().mode(0o700).create(dir);
        if std::fs::metadata(dir).is_ok_and(|m| m.permissions().mode() & 0o077 != 0) {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    let _ = std::fs::create_dir_all(dir);
}

/// Writes `bytes` to `path` whole or not at all: into a temporary file beside it, flushed to disk,
/// then renamed over it. A crash or a full disk leaves the old file (or none), never half a file.
/// On Windows a scanner that holds the old file open fails the rename for a moment: it is tried
/// again for up to about 2.5 s.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    replace(path, bytes, 0o644)
}

/// `write_atomic` for a secret (a token): the file is `0600` from its creation on Linux and macOS.
/// On Windows, files in the state folder inherit its user-only ACL.
pub fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    replace(path, bytes, 0o600)
}

/// `write_atomic` for a program strobe runs (a downloaded engine): `0755` from its creation, so
/// no moment exists where the file is there but cannot run.
pub fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    replace(path, bytes, 0o755)
}

/// `mode` is the Unix permission bits of the new file; Windows has none to set.
fn replace(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let mut f = Atomic::create(path, mode)?;
    f.write_all(bytes)?;
    f.commit()
}

/// A file written in pieces (a trace streamed from the target, a heap snapshot), whole or not at
/// all, as `write_atomic` writes one at once: into a temporary file beside `path` until `commit`
/// flushes it to disk and renames it over `path`. Dropped without `commit` (an error midway), the
/// temporary file is removed and `path` stays as it was.
pub struct Atomic {
    file: Option<std::fs::File>,
    tmp: PathBuf,
    path: PathBuf,
}

impl Atomic {
    /// `mode` is the Unix permission bits of the new file (`0o644`, or `0o600` for what only its
    /// owner may read); Windows has none to set.
    pub fn create(path: &Path, mode: u32) -> std::io::Result<Atomic> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let tmp = dir.join(format!(".{name}.{}-{}.tmp", std::process::id(), csprng_hex(4)?));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let file = o.open(&tmp)?;
        Ok(Atomic { file: Some(file), tmp, path: path.to_path_buf() })
    }

    pub fn commit(mut self) -> std::io::Result<()> {
        if let Some(f) = self.file.take() {
            f.sync_all()?;
        }
        let mut wait = std::time::Duration::from_millis(10);
        let mut tries = 0;
        loop {
            match std::fs::rename(&self.tmp, &self.path) {
                Ok(()) => {
                    self.tmp = PathBuf::new();
                    return Ok(());
                }
                // Sharing violations and access denied: another process holds the file (an antivirus).
                Err(e) if cfg!(windows) && tries < 8 && matches!(e.raw_os_error(), Some(5 | 32 | 33)) => {
                    tries += 1;
                    std::thread::sleep(wait);
                    wait *= 2;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn file(&mut self) -> std::io::Result<&mut std::fs::File> {
        self.file.as_mut().ok_or_else(|| std::io::Error::other("the file was committed"))
    }
}

impl Write for Atomic {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file()?.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file()?.flush()
    }
}

impl Drop for Atomic {
    fn drop(&mut self) {
        self.file.take();
        if !self.tmp.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// `n` bytes from the OS's random source (`BCryptGenRandom` on Windows, `getrandom` on Linux,
/// `getentropy` on macOS), as hex: ids, temporary names, tokens. It has no weaker fallback.
pub fn csprng_hex(n: usize) -> std::io::Result<String> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).map_err(|e| std::io::Error::other(format!("the OS random source failed: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}
