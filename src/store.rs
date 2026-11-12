//! Recordings and experiments on disk: the only writer (SPEC §14).

use crate::host::{self, paths};
use crate::model::{Error, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

pub mod actions;
pub mod ledger;

/// A short random id, `<prefix>_` + 6 hex from the OS's random source, that names no recording or
/// experiment yet and that this process never handed out before (two ids in one clock tick were
/// once the same, and the second recording wrote into the first's folder).
pub fn new_id(prefix: &str) -> String {
    static ISSUED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut issued = ISSUED.lock().unwrap_or_else(|e| e.into_inner());
    let issued = issued.get_or_insert_with(HashSet::new);
    let roots = [paths::recordings(), paths::experiments()];
    loop {
        // A counter over the clock stands in should the OS's random source ever fail: each try draws
        // another id, even within one clock tick.
        static FALLBACK: AtomicU64 = AtomicU64::new(0);
        let hex = paths::csprng_hex(3).unwrap_or_else(|_| format!("{:06x}", (host::now_ns() ^ FALLBACK.fetch_add(0x9E37_79B9, Ordering::Relaxed)) & 0xFF_FFFF));
        let id = format!("{prefix}_{hex}");
        if !roots.iter().any(|r| r.join(&id).exists()) && issued.insert(id.clone()) {
            return id;
        }
    }
}

/// A new recording's id and its folder, made by this call alone (a folder that already exists:
/// another process took the id, draws another), held as a `Draft` until its meta.json is written.
pub fn new_recording() -> Result<(String, Draft)> {
    let (id, dir) = claim("rec", &paths::recordings())?;
    Ok((id, Draft(Some(dir))))
}

/// Whether `id` may name a new recording: `rec_` and letters, digits, `_` or `-`, and no folder yet
/// (checked again when it is claimed). A command checks it before it launches anything.
pub fn free_recording_id(id: &str) -> Result<()> {
    let ok = id.strip_prefix("rec_").is_some_and(|r| !r.is_empty() && r.len() <= 64 && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    if !ok {
        return Err(Error::usage(format!("--id {id}: a recording id is rec_ and letters, digits, _ or -")));
    }
    if paths::recordings().join(id).exists() {
        return Err(Error::usage(format!("--id {id}: that recording exists already; pick another id")));
    }
    Ok(())
}

/// The folder of a recording whose id the caller chose (`--id`, so an MCP call that outlives its
/// timeout can be followed), made by this call alone: an id that is not `rec_` and letters, digits,
/// `_` or `-`, or that names a folder already, is a usage error, so no draft ever deletes another
/// recording.
pub fn claim_recording(id: &str) -> Result<Draft> {
    free_recording_id(id)?;
    let dir = paths::recordings().join(id);
    std::fs::create_dir_all(paths::recordings())?;
    match std::fs::create_dir(&dir) {
        Ok(()) => Ok(Draft(Some(dir))),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(Error::usage(format!("--id {id}: that recording exists already; pick another id"))),
        Err(e) => Err(Error::msg(format!("create {}: {e}", dir.display()))),
    }
}

/// `new_recording` for an experiment.
pub fn new_experiment() -> Result<(String, PathBuf)> {
    claim("exp", &paths::experiments())
}

fn claim(prefix: &str, root: &Path) -> Result<(String, PathBuf)> {
    for _ in 0..100 {
        let id = new_id(prefix);
        let dir = root.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::msg(format!("create {}: {e}", dir.display()))),
        }
    }
    Err(Error::msg(format!("no free {prefix}_ id in {}", root.display())))
}

/// Removes a recording's or an experiment's folder for `strobe prune` (SPEC §5.14): only a folder
/// directly in `recordings/` or `experiments/`, never through a link (`host::disk::remove`).
pub fn remove(dir: &Path) -> host::disk::Removed {
    match [paths::recordings(), paths::experiments()].into_iter().find(|r| dir.parent() == Some(r.as_path())) {
        Some(root) => host::disk::remove(dir, &root),
        None => host::disk::Removed { failed: vec![format!("{}: not a recording or an experiment", dir.display())], ..Default::default() },
    }
}

/// Marks a recording or experiment used now (`<dir>/.used`): `strobe prune` ages data by its last use.
/// Not while a folder a person named is being read (`Named`): an id found in such a file (a foreign
/// result's recordings, a meta.json's experiment) marks nothing of the store.
fn touch(dir: &Path) {
    if reading_named() {
        return;
    }
    if let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(".used")) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Marks a folder of the store used now (`touch`), as a view of it by its exact id does (the daemon's
/// `/report/`, `/video/`, `/perfetto/open/`: a report looked at in the browser every day is not pruned
/// as unused). Only a folder directly in `recordings/` or `experiments/`.
pub fn used(dir: &Path) {
    if kept(dir) {
        touch(dir);
    }
}

/// How many folders a person named are being read now, on any thread (`Named`).
static NAMED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// While held, a folder or file a person named on the command line (not one strobe keeps) is being
/// read: what such a file says must not make strobe look at anything outside it. Readers that would
/// (a `package.json` up from a frame's file, a git checkout's root, whether a path in a result exists,
/// a `.used` stamp) do not, on every thread, while any is held: a daemon reading a stored recording at
/// the same moment only names fewer packages.
pub struct Named(());

impl Named {
    pub fn hold() -> Named {
        NAMED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Named(())
    }
}

impl Drop for Named {
    fn drop(&mut self) {
        NAMED.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Whether a named folder or file is being read (`Named`).
pub fn reading_named() -> bool {
    NAMED.load(std::sync::atomic::Ordering::SeqCst) > 0
}

/// Whether `dir` is a folder strobe keeps: one directly in `recordings/` or `experiments/`.
pub fn kept(dir: &Path) -> bool {
    dir.parent().is_some_and(|p| p == paths::recordings() || p == paths::experiments())
}

/// The rule for a file inside a folder a person named on the command line: it is read only if it is a
/// regular file there: not a symbolic link, a junction or any other reparse point (Windows), not a file
/// with other hard links (Unix: another name of a file elsewhere), not a device or pipe.
pub fn regular(path: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(path) else { return false };
    if !m.file_type().is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT
        if m.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    crate::host::disk::names(path).is_some_and(|n| n <= 1)
}

/// A name a folder's own files give for another of its files (a map index's `maps/0.map.gz`, a
/// video.json's `video.mp4`, a check's actions file): only plain steps (letters, digits, `_`, `-`, `.`;
/// no `.` or `..` step, no separator but `/`, no drive or stream syntax). `regular` then decides whether it
/// is read.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c)))
}

/// A file of `dir` by a name one of its files gives: inside it (`plain_name`), and, in a folder a
/// person named, a regular file (`regular`). In a folder strobe keeps, as it is.
pub fn held_file(dir: &Path, name: &str) -> Option<PathBuf> {
    if !plain_name(name) {
        return None;
    }
    let p = dir.join(name);
    let ok = if kept(dir) { p.is_file() } else { folders_plain(dir, name) && regular(&p) };
    ok.then_some(p)
}

/// Every folder on the way to `name` inside `dir` is a real folder (no link, junction or reparse point).
fn folders_plain(dir: &Path, name: &str) -> bool {
    let mut at = dir.to_path_buf();
    let steps: Vec<&str> = name.split('/').collect();
    for s in &steps[..steps.len().saturating_sub(1)] {
        at = at.join(s);
        if !real_folder(&at) {
            return false;
        }
    }
    true
}

/// A folder that is a folder there, not a link, junction or other reparse point.
fn real_folder(p: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(p) else { return false };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    m.file_type().is_dir()
}

/// The most entries, and the deepest folders, a named folder may hold (a recording holds a few dozen
/// files, its `maps/` and `import/` one level down).
const NAMED_ENTRIES: usize = 20_000;
const NAMED_DEPTH: usize = 4;

/// A folder a person named (a recording's or an experiment's, copied or from another machine), checked
/// once before anything in it is read: every entry, to `NAMED_DEPTH` folders down, is a regular file
/// (`regular`) or a real folder, with a plain name (no `:` stream syntax). A folder that holds a link
/// out of it (a symbolic link, a junction, a hard link) is refused with the entry's name, so a file the
/// recording's readers open by its fixed name (`frames.json`, `cpu-0.cpuprofile`, …) is always the
/// folder's own. strobe's own folders are not checked.
pub fn named_folder(dir: &Path) -> Result<()> {
    if kept(dir) {
        return Ok(());
    }
    // The folder itself may be a link the person typed: what is inside it is checked.
    if !dir.is_dir() {
        return Err(Error::usage(format!("{}: not a folder", dir.display())));
    }
    let mut todo = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((at, depth)) = todo.pop() {
        for e in std::fs::read_dir(&at).map_err(|e| Error::usage(format!("{}: {e}", at.display())))?.flatten() {
            seen += 1;
            if seen > NAMED_ENTRIES {
                return Err(Error::usage(format!("{}: more than {NAMED_ENTRIES} entries, more than a recording or an experiment holds: not read", dir.display())));
            }
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let refuse = |what: &str| Err(Error::usage(format!("{}: {what}, so nothing in this folder is read (a named folder's files must be its own)", p.display())));
            if name.contains(':') {
                return refuse("a name with a `:` (a stream or a drive)");
            }
            if real_folder(&p) {
                if depth + 1 < NAMED_DEPTH {
                    todo.push((p, depth + 1));
                }
            } else if !regular(&p) {
                return refuse("a link, junction, hard-linked or special file");
            }
        }
    }
    Ok(())
}

/// Whether `path` lies in strobe's own store (`recordings/`, `experiments/`): what strobe wrote there.
/// Anything else (a folder named by its path) is read with the bounds below.
pub fn in_store(path: &Path) -> bool {
    path.starts_with(paths::recordings()) || path.starts_with(paths::experiments())
}

/// A path that names another machine or a device rather than this disk: `//host/…`, `\\host\…`,
/// `\\?\UNC\…`, `\\.\pipe\…`, also as a `file:` URL's path. Opening, or only asking whether one
/// exists, connects to that host (SMB, and on Windows sends the user's credentials): strobe never asks
/// of a path it read from a file.
pub fn unc(path: &str) -> bool {
    let p = path.replace('\\', "/");
    let p = match p.strip_prefix("file:") {
        // `file:///x` is this disk's `/x`; `file:////host/x` and `file://host/x` name a host.
        Some(rest) => match rest.strip_prefix("//") {
            Some(r) if r.starts_with('/') => r.to_string(),
            Some(r) => return !(r.starts_with("localhost/") || r == "localhost"),
            None => return false,
        },
        None => p,
    };
    match p.strip_prefix("//") {
        // `\\?\C:\x` is this disk's, as a verbatim path; `\\?\UNC\host\x` another machine's.
        Some(rest) if rest.starts_with("?/") => rest[2..].to_ascii_uppercase().starts_with("UNC/"),
        Some(_) => true,
        None => false,
    }
}

/// Whether strobe may look on the disk for something a frame or a result names (a `package.json` up
/// from a script's file, a git checkout's root, whether a bench file is here): never for another
/// machine's path (`unc`), and never while a folder or file a person named is read (`Named`).
pub fn may_look(path: &str) -> bool {
    !unc(path) && !reading_named()
}

/// The most a JSON file outside the store is read whole (`read_json`; the largest one strobe writes is
/// well under 100 MB).
pub const MAX_OUTSIDE_JSON: u64 = 1 << 30;

/// How much a gzip file outside the store may expand to: 64 times its size (the most any file strobe
/// writes expands is 19 times, a thread timeline; a heap snapshot 6), at least 64 MiB, at most 4 GiB.
/// A file in the store is read whole, as strobe wrote it.
pub fn gz_limit(path: &Path) -> Option<u64> {
    if in_store(path) {
        return None;
    }
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Some(len.saturating_mul(64).clamp(64 << 20, 4 << 30))
}

/// A gzip file's contents as they are read (`MultiGzDecoder`), outside the store bounded by `gz_limit`:
/// past it the read fails with why.
pub fn gunzip(path: &Path) -> std::io::Result<Box<dyn std::io::Read>> {
    let f = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
    let gz = flate2::read::MultiGzDecoder::new(f);
    Ok(match gz_limit(path) {
        None => Box::new(gz),
        Some(limit) => Box::new(Capped { inner: gz, left: limit, limit, path: path.to_path_buf() }),
    })
}

/// A gzip file's text (`gunzip`).
pub fn gunzip_string(path: &Path) -> std::io::Result<String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut gunzip(path)?, &mut text)?;
    Ok(text)
}

/// A reader that fails once more than `limit` bytes came out of it.
struct Capped<R> {
    inner: R,
    left: u64,
    limit: u64,
    path: PathBuf,
}

impl<R: std::io::Read> std::io::Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n as u64 > self.left {
            return Err(std::io::Error::other(format!("{}: expands to more than {} MB, 64 times its size, which no file strobe writes does: not read", self.path.display(), self.limit >> 20)));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// Removes an unfinished recording (`Draft`): none of it is kept.
fn discard(dir: &Path) {
    if dir.parent().is_some_and(|p| p == paths::recordings() || p == paths::experiments()) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub fn experiment_dir(id: &str) -> Result<PathBuf> {
    // A person's `--id` (bench, bisect) names a folder of the store: only a plain id, as handles are
    // (`resolve` finds no other), short enough for a bisect's steps (`<id>_s<k>`).
    if !plain_id(id) || id.len() > 96 {
        return Err(Error::usage(format!("--id {id}: an experiment's id is letters, digits, _ and - (at most 96), as every handle is: then strobe query {id} finds it")));
    }
    let dir = paths::experiments().join(id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Writes a file whole or not at all (`host::paths::write_atomic`): a reader never sees half a
/// result.json, and a crash keeps the last complete one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    paths::write_atomic(path, bytes).map_err(|e| Error::msg(format!("write {}: {e}", path.display())))
}

/// A file of a recording written in pieces (a trace or a heap snapshot streamed from the target),
/// whole or not at all: it appears at `path` only on `commit` (`host::paths::Atomic`). What it holds
/// is the target's data verbatim (SPEC rule 9), so only its owner may read it, from its creation.
pub fn create(path: &Path) -> Result<paths::Atomic> {
    paths::Atomic::create(path, 0o600).map_err(|e| Error::msg(format!("create {}: {e}", path.display())))
}

/// Moves a finished file another program wrote (an encoder's video, in the run folder) into a
/// recording's folder as `name`, owner-only like everything of the target's (rule 9). Same volume
/// (the state folder): a rename, so it appears whole.
pub fn adopt(dir: &Path, name: &str, from: &Path) -> Result<PathBuf> {
    let to = dir.join(name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(from, std::fs::Permissions::from_mode(0o600));
    }
    if std::fs::rename(from, &to).is_err() {
        std::fs::copy(from, &to).map_err(|e| Error::msg(format!("keep {}: {e}", to.display())))?;
    }
    Ok(to)
}

/// A recording's folder until it is complete: dropped before `keep` (an error, a refusal), the folder
/// and all it holds go, so no recording is left without its `meta.json`. Only `new_recording` and
/// `claim_recording` make one, over a folder they created, so a draft never deletes a folder it did
/// not make.
pub struct Draft(Option<PathBuf>);

impl Draft {
    pub fn dir(&self) -> &Path {
        self.0.as_deref().unwrap_or(Path::new(""))
    }

    pub fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for Draft {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            discard(&dir);
        }
    }
}

pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(value)?)
}

pub fn append_index(line: &Value) -> Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(paths::experiments().join("index.jsonl"))?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Whether `s` can name a recording or experiment folder: letters, digits, `_` and `-`, at most 128.
/// Nothing else (a path, `.`, `..`, a drive) ever reaches a folder of the store: an id read from a file
/// (a result's recordings, a meta.json's own) is checked with this before it is joined to anything.
pub fn plain_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Resolves a handle or a unique prefix (`rec_3f`, `exp_8c`) to its directory. `bench --id` names an
/// experiment freely, so a handle without either prefix may be a recording or an experiment. Only a
/// plain id (`plain_id`): a path never resolves here (`query` and `report` read paths themselves), so
/// nothing outside the store is ever marked used.
pub fn resolve(handle: &str) -> Result<PathBuf> {
    if !plain_id(handle) {
        return Err(Error::usage(format!("no recording or experiment {handle}: a handle is an id (rec_…, exp_…) or its first letters")));
    }
    let roots = if handle.starts_with("exp_") {
        vec![paths::experiments()]
    } else if handle.starts_with("rec_") {
        vec![paths::recordings()]
    } else {
        vec![paths::recordings(), paths::experiments()]
    };
    if let Some(exact) = roots.iter().map(|root| root.join(handle)).find(|p| p.is_dir()) {
        touch(&exact);
        return Ok(exact);
    }
    let matches: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| std::fs::read_dir(root).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|e| e.file_name().to_string_lossy().starts_with(handle))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    match matches.len() {
        1 => {
            touch(&matches[0]);
            Ok(matches[0].clone())
        }
        0 => Err(Error::usage(format!("no recording or experiment {handle}"))),
        _ => Err(Error::usage(format!("{handle} is ambiguous ({} matches)", matches.len()))),
    }
}

/// Whether a resolved handle is an experiment (bench or bisect) rather than a recording.
pub fn is_experiment(dir: &std::path::Path) -> bool {
    dir.parent().is_some_and(|parent| parent == paths::experiments())
}

/// Every line of `experiments/index.jsonl`, oldest first; lines that do not parse are skipped.
pub fn index() -> Vec<Value> {
    std::fs::read_to_string(paths::experiments().join("index.jsonl")).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

pub fn read_json(path: &std::path::Path) -> Result<Value> {
    use serde::Deserialize;
    if !in_store(path)
        && let Ok(m) = std::fs::metadata(path)
        && m.len() > MAX_OUTSIDE_JSON
    {
        return Err(Error::usage(format!("{}: {} MB, more than any file strobe writes ({} MB at most): not read", path.display(), m.len() >> 20, MAX_OUTSIDE_JSON >> 20)));
    }
    let bytes = std::fs::read(path)?;
    deep_enough(&bytes, path)?;
    // Profiles nest one level per stack frame; serde_json stops at 128 by default.
    let mut de = serde_json::Deserializer::from_slice(&bytes);
    de.disable_recursion_limit();
    Ok(Value::deserialize(&mut de)?)
}

/// The deepest JSON strobe reads: an allocation profile nests two levels per stack frame (V8 keeps 128
/// frames of a sampled allocation; the deepest file kept here nests 83), and reading or dropping a value
/// recurses once per level, so a file nested far deeper (a crafted one) would overflow the thread's
/// stack: refused before it is parsed. Measured on Windows release builds (serde_json 1.0.154, this
/// profile): a 1 MiB main thread (MSVC, the ARM64 release) overflows past 3,018 levels of
/// `serde_json::Value` and 2,324 of a typed profile node, a 2 MiB one (the x64 release's GNU main
/// thread, every spawned thread) past 6,142 and 4,764: at least 4.5 times this.
pub const MAX_JSON_DEPTH: usize = 512;

/// Refuses JSON nested past `MAX_JSON_DEPTH` (strings skipped, one pass over the bytes).
pub fn deep_enough(bytes: &[u8], path: &Path) -> Result<()> {
    let (mut depth, mut max, mut string, mut escape) = (0usize, 0usize, false, false);
    for &c in bytes {
        if string {
            match c {
                _ if escape => escape = false,
                b'\\' => escape = true,
                b'"' => string = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => string = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
                if max > MAX_JSON_DEPTH {
                    return Err(Error::usage(format!("{}: JSON nested more than {MAX_JSON_DEPTH} levels deep, which no file strobe writes is: not read", path.display())));
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

/// The most of a result.json or meta.json strobe reads when a person names it by its path (the largest
/// result kept here is under 20 MB).
pub const MAX_NAMED_JSON: u64 = 256 << 20;

/// A result.json or meta.json named by its path, read as it is and checked first: an object (its first
/// byte past blanks is `{`), at most `MAX_NAMED_JSON`, nested at most serde_json's default 128 levels
/// (the deepest result kept here nests 11). A file that is none of these is refused with why, never read
/// whole or parsed past its stack.
pub fn read_named_json(path: &Path) -> Result<Value> {
    use std::io::Read;
    let shown = path.display();
    let len = std::fs::metadata(path).map_err(|e| Error::usage(format!("{shown}: {e}")))?.len();
    if len > MAX_NAMED_JSON {
        return Err(Error::usage(format!("{shown}: {} MB, larger than any result.json or meta.json ({} MB at most): not read", len >> 20, MAX_NAMED_JSON >> 20)));
    }
    let mut f = std::fs::File::open(path).map_err(|e| Error::usage(format!("{shown}: {e}")))?;
    let mut head = [0u8; 512];
    let n = f.read(&mut head)?;
    if head[..n].iter().find(|c| !c.is_ascii_whitespace()) != Some(&b'{') {
        return Err(Error::usage(format!("{shown}: not a result.json or meta.json (no JSON object)")));
    }
    // The rest from the same handle: one open, one read, one parse.
    let mut bytes = head[..n].to_vec();
    f.take(MAX_NAMED_JSON).read_to_end(&mut bytes)?;
    // serde_json's own limit (128 levels) refuses a deeper file with an error, not a stack overflow.
    serde_json::from_slice(&bytes).map_err(|e| Error::usage(format!("{shown}: not a result.json or meta.json strobe can read: {e}")))
}

/// The newest recordings or experiments, newest first.
pub fn list(prefix: &str, limit: usize) -> Vec<(String, std::time::SystemTime)> {
    let root = if prefix == "exp" { paths::experiments() } else { paths::recordings() };
    let mut v: Vec<(String, std::time::SystemTime)> = std::fs::read_dir(root)
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| Some((e.file_name().to_string_lossy().to_string(), e.metadata().ok()?.modified().ok()?))).collect())
        .unwrap_or_default();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v.truncate(limit);
    v
}
