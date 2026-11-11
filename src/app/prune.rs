//! `strobe prune` and the disk numbers of `strobe status` (SPEC §5.14). Everything strobe keeps falls
//! in one category: caches strobe makes again when it needs them (variant builds, seed copies, GPU
//! shader caches, downloaded symbols, other builds' SDKs and engines, what finished processes left in
//! `run/`), data it cannot make again (recordings, experiments), and what a prune never touches
//! (tracked history, logs, settings, strobe's own executables). A size is what the files take on the
//! disk, each file once however many names it has, never through a link out of strobe's folders.

use super::Output;
use crate::cli::Args;
use crate::host::{self, cache, disk, paths};
use crate::model::{Error, Result};
use crate::store;
use crate::target::isolate;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// strobe makes it again when it needs it: `strobe prune` removes what no live process uses.
    Cache,
    /// Not made again: removed only when named (or `--all`), and only what is older than `--older-than`.
    Data,
    /// Never pruned.
    Kept,
}

impl Class {
    fn name(self) -> &'static str {
        match self {
            Class::Cache => "cache",
            Class::Data => "data",
            Class::Kept => "kept",
        }
    }
}

pub struct Category {
    pub name: &'static str,
    pub class: Class,
    pub what: &'static str,
}

const fn cat(name: &'static str, class: Class, what: &'static str) -> Category {
    Category { name, class, what }
}

pub const CATEGORIES: &[Category] = &[
    cat("builds", Class::Cache, "variant builds: git worktrees, built executables, compile caches"),
    cat("seeds", Class::Cache, "shared copies of large seed files"),
    cat("gpu-cache", Class::Cache, "GPU shader caches of warm launches"),
    cat("symbols", Class::Cache, "downloaded debug symbols"),
    cat("sdk", Class::Cache, "the bench SDK of other strobe builds"),
    cat("engine", Class::Cache, "trace_processor versions this build does not use"),
    cat("run", Class::Cache, "what finished processes left: launch folders, temporary files"),
    cat("recordings", Class::Data, "recordings (rec_…)"),
    cat("experiments", Class::Data, "experiments (exp_…)"),
    cat("history", Class::Kept, "tracked series (strobe track)"),
    cat("logs", Class::Kept, "the daemon's and the helper's logs"),
    cat("other", Class::Kept, "settings, tokens, checks and journals"),
    cat("install", Class::Kept, "strobe's executables and skill, outside the state folder"),
];

fn index(name: &str) -> usize {
    CATEGORIES.iter().position(|c| c.name == name).unwrap_or(CATEGORIES.len() - 2)
}

/// Data is removed when it is older than this, unless `--older-than` says otherwise.
const DATA_AGE: Duration = Duration::from_secs(30 * 86_400);
/// A temporary file without a process to ask about (`run/`) is left this long: a command may still use it.
const TEMP_AGE: Duration = Duration::from_secs(86_400);
/// Caches unused this long go on their own, at most once a day (`prune.unused` in the user's strobe.json).
const AUTO_AGE: Duration = Duration::from_secs(14 * 86_400);

/// Why an entry stays whatever a prune asks: a short reason (entries are counted by it) and the
/// particulars.
#[derive(Clone, Debug)]
struct Why {
    what: String,
    detail: String,
}

fn why(what: impl Into<String>, detail: impl Into<String>) -> Option<Why> {
    Some(Why { what: what.into(), detail: detail.into() })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Removal {
    /// Its paths, then nothing else.
    Plain,
    /// A launch folder of a strobe that is gone: what still runs from it ends first (`isolate::release_gone`).
    Launch,
    /// A recording or an experiment: through `store`, their only writer.
    Store,
}

struct Entry {
    cat: usize,
    name: String,
    /// Removed together; the first is the entry itself.
    paths: Vec<PathBuf>,
    /// The folder the paths lie in: removal refuses anything outside it.
    root: PathBuf,
    /// The entry's own modification time, from its folder's listing.
    seen: Option<SystemTime>,
    how: Used,
    /// When it was last used, read the first time a plan asks.
    used: std::cell::OnceCell<Option<SystemTime>>,
    keep: Option<Why>,
    /// Entries whose stamp a live process locks while it uses them (`host::cache`).
    leases: Vec<PathBuf>,
    /// A git worktree: the repository's git folder, the worktree's record in it, and the worktree's
    /// own folder resolved (all while it is there).
    worktree: Option<(PathBuf, PathBuf, PathBuf)>,
    removal: Removal,
    size: disk::Sized,
    /// Not walked: the time budget ran out before it (`status`), its size unknown.
    unmeasured: bool,
}

/// How an entry's last use is read.
enum Used {
    /// When it or what it holds directly was last written (data: `.used` is touched on each use).
    Write,
    /// Its stamp (`host::cache`), else `Write`.
    Cache,
    /// A worktree: the newest of its stamp, its executable copy's stamp and its build key.
    Build { out: PathBuf, key: PathBuf },
}

impl Entry {
    fn new(cat: &str, name: String, (path, seen): (PathBuf, Option<SystemTime>), root: &Path) -> Entry {
        Entry {
            cat: index(cat),
            name,
            paths: vec![path],
            root: root.to_path_buf(),
            seen,
            how: Used::Write,
            used: std::cell::OnceCell::new(),
            keep: None,
            leases: Vec::new(),
            worktree: None,
            removal: Removal::Plain,
            size: disk::Sized::default(),
            unmeasured: false,
        }
    }

    fn class(&self) -> Class {
        CATEGORIES[self.cat].class
    }

    fn used(&self) -> Option<SystemTime> {
        *self.used.get_or_init(|| {
            let p = &self.paths[0];
            match &self.how {
                Used::Write => last_write(p),
                Used::Cache => cache_used(p),
                Used::Build { out, key } => [cache_used(p), cache::last_used(out), last_write(key)].into_iter().flatten().max(),
            }
        })
    }

    /// Whether its last use is less than `d` ago.
    fn within(&self, d: Duration) -> bool {
        self.used().and_then(|t| t.elapsed().ok()).is_none_or(|a| a < d)
    }
}

/// The newest modification time of `path` and of what it holds directly (a listing, no walk).
fn last_write(path: &Path) -> Option<SystemTime> {
    let own = disk::stat(path).ok()?;
    let inner = if own.kind == disk::Kind::Dir { disk::list(path).unwrap_or_default().into_iter().filter_map(|i| i.modified).max() } else { None };
    own.modified.max(inner)
}

/// When a cache entry was last used: its stamp (`host::cache`), else when it or what it holds
/// directly was last written.
fn cache_used(path: &Path) -> Option<SystemTime> {
    cache::last_used(path).or_else(|| last_write(path))
}

fn ago(t: Option<SystemTime>) -> String {
    match t.and_then(|t| t.elapsed().ok()) {
        None => "just now".into(),
        Some(d) => {
            let s = d.as_secs();
            match s {
                0..120 => format!("{s} s ago"),
                120..7200 => format!("{} min ago", s / 60),
                7200..172_800 => format!("{} h ago", s / 3600),
                _ => format!("{} d ago", s / 86_400),
            }
        }
    }
}

fn days(d: Duration) -> String {
    let s = d.as_secs();
    if s.is_multiple_of(86_400) {
        format!("{}d", s / 86_400)
    } else if s.is_multiple_of(3600) {
        format!("{}h", s / 3600)
    } else if s.is_multiple_of(60) {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// What `dir` holds but stamps, with each one's modification time, from one listing.
fn children(dir: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    listed(dir, |_| true)
}

/// The folders in `dir` (`children`).
fn dirs(dir: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    listed(dir, |i| i.kind == disk::Kind::Dir)
}

fn listed(dir: &Path, keep: impl Fn(&disk::Item) -> bool) -> Vec<(PathBuf, Option<SystemTime>)> {
    let mut v: Vec<(PathBuf, Option<SystemTime>)> =
        disk::list(dir).unwrap_or_default().into_iter().filter(|i| keep(i) && !i.name.to_string_lossy().ends_with(".used")).map(|i| (dir.join(&i.name), i.modified)).collect();
    v.sort();
    v
}

fn same_or_under(p: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| {
        let s = paths::plain(p).replace('/', "\\");
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let (p, d) = (norm(p), norm(dir));
    p == d || p.strip_prefix(&d).is_some_and(|r| r.starts_with('\\') || r.starts_with('/'))
}

/// Whether `pid` is a live strobe (a registration or a file name can name a reused pid).
fn strobe_alive(pid: u32) -> bool {
    pid == std::process::id() || host::exe_of(pid).and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())).is_some_and(|n| n.starts_with("strobe"))
}

/// What runs on this device now that holds strobe's files.
struct Live {
    /// Running benches (`bench::slot::list`).
    benches: Vec<Value>,
    /// A running bench of a strobe build that takes no leases on the builds it uses.
    unleased: Option<String>,
    /// What the launch folders of live strobes borrow (seed copies, GPU cache stores).
    borrowed: Vec<PathBuf>,
    /// The daemon runs another strobe build, which may use another engine version.
    other_daemon: Option<String>,
}

impl Live {
    fn read() -> Live {
        let mut live = Live { benches: Vec::new(), unleased: None, borrowed: Vec::new(), other_daemon: None };
        live.local();
        live.other_daemon =
            super::mcp::running().then(super::mcp::daemon_build).flatten().filter(|b| b.as_str() != super::mcp::build_id()).map(|b| format!("the daemon runs another strobe build ({b})"));
        live
    }

    /// Reads again what this state folder's files say runs (the bench registry and slot, launch
    /// folders): cheap enough before each entry a removal takes.
    fn local(&mut self) {
        let benches = super::bench::slot::list();
        let mut unleased = benches
            .iter()
            .find(|b| b["leases"] != true)
            .map(|b| format!("{} (pid {}) runs on a strobe build that marks no build it uses; prune builds once it ends", b["exp"].as_str().unwrap_or("a bench"), b["pid"]));
        // A strobe from before the bench registry holds the slot by `run/bench.lock` alone.
        if unleased.is_none()
            && let Some(pid) = std::fs::read_to_string(paths::run().join("bench.lock")).ok().and_then(|t| t.trim().parse::<u32>().ok())
            && strobe_alive(pid)
            && pid != std::process::id()
            && !benches.iter().any(|b| b["pid"].as_u64() == Some(pid as u64) && b["leases"] == true)
        {
            unleased = Some(format!("an older strobe (pid {pid}) holds the bench slot (run/bench.lock) and marks no build it uses; prune builds once it ends"));
        }
        self.borrowed = isolate::launch_folders().into_iter().filter(|d| isolate::folder_gone(d) == Some(false)).flat_map(|d| isolate::borrowed(&d)).collect();
        self.benches = benches;
        self.unleased = unleased;
    }

    fn running(&self, exp: &str) -> Option<&Value> {
        self.benches.iter().find(|b| b["exp"].as_str().is_some_and(|e| e == exp || e.strip_prefix(exp).is_some_and(|r| r.starts_with("_s"))))
    }
}

/// Whether experiment `exp` can go on (`bench --resume`): it saved its state and wrote no result.
fn resumable(state: &Path, exp: &str) -> bool {
    let dir = state.join("experiments").join(exp);
    dir.join("state.json").is_file() && !dir.join("result.json").exists()
}

/// The experiments that can resume, with their saved state's text (what a `run/cmd-<hash>` folder is
/// named in).
fn resumable_states(state: &Path) -> Vec<(String, String)> {
    dirs(&state.join("experiments"))
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| !p.join("result.json").exists())
        .filter_map(|p| Some((name_of(&p), std::fs::read_to_string(p.join("state.json")).ok()?)))
        .collect()
}

/// Every entry a prune could remove, with why it stays when it must.
fn entries(state: &Path, live: &Live) -> Vec<Entry> {
    let mut out = Vec::new();
    builds(state, live, &mut out);
    for c in children(&state.join("seeds")) {
        let mut e = Entry::new("seeds", format!("seeds/{}", name_of(&c.0)), c, state);
        e.how = Used::Cache;
        // Leased by a launch that links it and by its experiment until it ends (`host::cache::keep`).
        e.leases = vec![e.paths[0].clone()];
        e.keep = if cache::in_use(&e.paths[0]) {
            why("in use by a running strobe", "")
        } else {
            live.borrowed.iter().find(|b| same_or_under(b, &e.paths[0])).and_then(|_| why("linked into a running launch's home", ""))
        };
        out.push(e);
    }
    for c in children(&state.join("gpu-cache")) {
        let mut e = Entry::new("gpu-cache", format!("gpu-cache/{}", name_of(&c.0)), c, state);
        // Its stamp: a launch takes it and gives it back without writing the store's own folder.
        e.how = Used::Cache;
        e.leases = vec![e.paths[0].clone()];
        // An experiment's store (`<exp>-<role>`) is its warm caches between blocks, and for `--resume`.
        let exp = e.name.trim_start_matches("gpu-cache/").rsplit_once('-').map(|(x, _)| x.to_string()).filter(|x| x != "rec");
        e.keep = if cache::in_use(&e.paths[0]) {
            why("in use by a running strobe", "")
        } else if let Some(b) = exp.as_deref().and_then(|x| live.running(x)) {
            why("its experiment runs", format!("{} (pid {})", b["exp"].as_str().unwrap_or(""), b["pid"]))
        } else if exp.as_deref().is_some_and(|x| resumable(state, x)) {
            why("its experiment can resume", format!("strobe bench --resume {}", exp.as_deref().unwrap_or("")))
        } else {
            live.borrowed.iter().find(|b| same_or_under(b, &e.paths[0])).and_then(|_| why("lent to a running launch", ""))
        };
        out.push(e);
    }
    for (group, _) in children(&state.join("symbols")) {
        for c in children(&group) {
            let mut e = Entry::new("symbols", format!("symbols/{}/{}", name_of(&group), name_of(&c.0)), c, state);
            e.how = Used::Cache;
            out.push(e);
        }
    }
    let build = host::build::build_id();
    for c in children(&state.join("sdk")) {
        let mut e = Entry::new("sdk", format!("sdk/{}", name_of(&c.0)), c, state);
        if name_of(&e.paths[0]) == build {
            e.keep = why("this build's", "");
        } else if e.within(TEMP_AGE) {
            // Builds before leases mark only `.used` when a bench starts.
            e.keep = why("used within a day: a bench of that build may run", ago(e.used()));
        }
        out.push(e);
    }
    for c in children(&state.join("engine")) {
        let mut e = Entry::new("engine", format!("engine/{}", name_of(&c.0)), c, state);
        e.keep = if name_of(&e.paths[0]) == crate::engine::VERSION {
            why("this build's", "")
        } else if let Some(who) = live.unleased.as_ref().or(live.other_daemon.as_ref()) {
            // An older strobe runs its own engine version.
            why("an older strobe runs", who.clone())
        } else {
            None
        };
        out.push(e);
    }
    run_folder(state, &mut out);
    for (dir, cat, prefix) in [
        (state.join("builds"), "builds", "builds/"),
        (state.join("builds").join("out"), "builds", "builds/out/"),
        (state.join("builds").join("cache"), "builds", "builds/cache/"),
        (state.join("seeds"), "seeds", "seeds/"),
        (state.join("gpu-cache"), "gpu-cache", "gpu-cache/"),
        (state.join("run"), "run", "run/"),
    ] {
        orphan_stamps(&dir, cat, prefix, state, &mut out);
    }
    for (group, _) in children(&state.join("symbols")) {
        let prefix = format!("symbols/{}/", name_of(&group));
        orphan_stamps(&group, "symbols", &prefix, state, &mut out);
    }
    for c in children(&state.join("recordings")) {
        let mut e = Entry::new("recordings", name_of(&c.0), c, state);
        e.removal = Removal::Store;
        // A folder made in the last day may still be filling (its own time moves as files land in it).
        let recent = e.seen.and_then(|t| t.elapsed().ok()).is_none_or(|a| a < TEMP_AGE);
        if recent && !e.paths[0].join("meta.json").is_file() {
            e.keep = why("being written (no meta.json yet)", "");
        } else if recent && !live.benches.is_empty() {
            let exp = store::read_json(&e.paths[0].join("meta.json")).ok().and_then(|m| m["experiment"].as_str().map(str::to_string));
            if let Some(b) = exp.as_deref().and_then(|x| live.running(x)) {
                e.keep = why("made by a bench that runs", format!("{} (pid {})", b["exp"].as_str().unwrap_or(""), b["pid"]));
            }
        }
        out.push(e);
    }
    for c in dirs(&state.join("experiments")) {
        let mut e = Entry::new("experiments", name_of(&c.0), c, state);
        e.removal = Removal::Store;
        if let Some(b) = live.running(&e.name) {
            e.keep = why("running", format!("pid {}", b["pid"]));
        }
        out.push(e);
    }
    out
}

/// Stamps (`<entry>.used`, `host::cache`) in `dir` whose entry is gone: an entry each (an
/// experiment's GPU cache store an older strobe removed without its stamp), removed under its own
/// hold, so one a process leases before it makes the entry (a seed copy) stays.
fn orphan_stamps(dir: &Path, cat: &str, prefix: &str, state: &Path, out: &mut Vec<Entry>) {
    for i in disk::list(dir).unwrap_or_default() {
        let n = i.name.to_string_lossy().to_string();
        let Some(entry) = n.strip_suffix(".used").filter(|e| !e.is_empty()).map(|e| dir.join(e)) else { continue };
        if i.kind != disk::Kind::File || !std::fs::symlink_metadata(&entry).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
            continue;
        }
        let mut e = Entry::new(cat, format!("{prefix}{n}"), (entry.clone(), i.modified), state);
        e.how = Used::Cache;
        e.leases = vec![entry.clone()];
        e.keep = if cache::in_use(&entry) { why("in use by a running strobe", "") } else { None };
        out.push(e);
    }
}

/// `builds/`: a git ref's worktree (`<sha12>`) with its executable's copy, build key and log; a folder
/// variant's executable copy (`out/<hash>`); a repository's compile cache (`cache/<hash>`); build logs.
fn builds(state: &Path, live: &Live, out: &mut Vec<Entry>) {
    let dir = state.join("builds");
    let hash = |p: &Path| super::bench::hash_str(&p.to_string_lossy());
    let mut claimed: HashSet<String> = HashSet::new();
    let in_use = |leases: &[PathBuf]| -> Option<Why> {
        if leases.iter().any(|l| cache::in_use(l)) {
            return why("in use by a running strobe", "");
        }
        live.unleased.as_ref().and_then(|u| why("a bench of an older strobe runs", u.clone()))
    };
    for c in dirs(&dir).into_iter().filter(|c| name_of(&c.0).len() == 12 && name_of(&c.0).bytes().all(|b| b.is_ascii_hexdigit())) {
        let p = c.0.clone();
        let h = hash(&p);
        claimed.insert(h.clone());
        let mut e = Entry::new("builds", format!("builds/{}", name_of(&p)), c, state);
        let (key, outdir, log) = (dir.join(format!("{h}.key")), dir.join("out").join(&h), dir.join("logs").join(format!("{h}.log")));
        e.paths.extend([outdir.clone(), key.clone(), log].into_iter().filter(|q| q.exists()));
        e.how = Used::Build { out: outdir.clone(), key };
        e.leases = vec![p.clone(), outdir];
        e.worktree = worktree_of(&p).zip(std::fs::canonicalize(&p).ok()).map(|((common, admin), real)| (common, admin, real));
        e.keep = in_use(&e.leases);
        out.push(e);
    }
    let outs: Vec<(PathBuf, Option<SystemTime>)> = children(&dir.join("out")).into_iter().filter(|c| !claimed.contains(&name_of(&c.0))).collect();
    for c in outs {
        let (p, h) = (c.0.clone(), name_of(&c.0));
        let mut e = Entry::new("builds", format!("builds/out/{h}"), c, state);
        e.how = Used::Cache;
        e.paths.extend([dir.join(format!("{h}.key")), dir.join("logs").join(format!("{h}.log"))].into_iter().filter(|q| q.exists()));
        claimed.insert(h);
        e.leases = vec![p];
        e.keep = in_use(&e.leases);
        out.push(e);
    }
    for c in children(&dir.join("cache")) {
        let p = c.0.clone();
        let mut e = Entry::new("builds", format!("builds/cache/{}", name_of(&p)), c, state);
        e.how = Used::Cache;
        e.leases = vec![p];
        e.keep = in_use(&e.leases);
        out.push(e);
    }
    for c in children(&dir.join("logs")).into_iter().filter(|c| !claimed.contains(name_of(&c.0).trim_end_matches(".log"))) {
        let mut e = Entry::new("builds", format!("builds/logs/{}", name_of(&c.0)), c, state);
        if e.within(TEMP_AGE) {
            e.keep = why("a failed build's log from the last day", ago(e.used()));
        }
        out.push(e);
    }
}

/// The repository's git folder and the worktree's own folder in it, from the worktree's `.git` file
/// (`gitdir: <repo>/.git/worktrees/<id>`) and that folder's `commondir`.
fn worktree_of(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let text = std::fs::read_to_string(dir.join(".git")).ok()?;
    // Resolved now, while the worktree is there (a relative `gitdir:`, git's worktree.useRelativePaths,
    // is from the worktree's folder).
    let admin = std::fs::canonicalize(dir.join(text.trim().strip_prefix("gitdir:")?.trim())).ok()?;
    let common = std::fs::read_to_string(admin.join("commondir")).ok().map(|c| admin.join(c.trim())).unwrap_or_else(|| admin.join("..").join(".."));
    let common = std::fs::canonicalize(&common).ok()?;
    // Only a repository's own record of a worktree: `<repo git folder>/worktrees/<id>`.
    (common.join("HEAD").is_file() && admin.parent() == Some(common.join("worktrees").as_path())).then_some((common, admin))
}

/// `p` with `.` and `..` folded away, by its text alone (for a folder already gone).
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Names in `run/` that are the slot, the bench registry, phone journals and open trace sessions:
/// never a prune's.
const RUN_FIXED: &[&str] = &["benches", "bench.slot", "bench.lock", "phones", "perfetto", "prune.lock"];
/// Temporary files named after the strobe process that made them.
const RUN_PID: &[&str] = &["q-", "helper-install-", "plan-", "sample-", "update-", "track-"];

fn run_folder(state: &Path, out: &mut Vec<Entry>) {
    let dir = state.join("run");
    let mut items: Vec<((PathBuf, Option<SystemTime>), PathBuf)> = Vec::new();
    for c in children(&dir) {
        let n = name_of(&c.0);
        if RUN_FIXED.contains(&n.as_str()) {
            continue;
        }
        if n == "reports" {
            items.extend(children(&c.0).into_iter().map(|r| (r, dir.clone())));
        } else {
            items.push((c, dir.clone()));
        }
    }
    // Snap Firefox's profiles, outside the state folder where its sandbox reads them.
    let snap = crate::target::firefox::snap_profiles();
    if snap != dir {
        items.extend(children(&snap).into_iter().filter(|c| isolate::folder_gone(&c.0).is_some()).map(|c| (c, snap.clone())));
    }
    let resumable = std::cell::OnceCell::new();
    for (c, root) in items {
        let p = c.0.clone();
        let n = name_of(&p);
        let shown = if root == dir { paths::plain(p.strip_prefix(state).unwrap_or(&p)) } else { paths::plain(&p) };
        let mut e = Entry::new("run", shown.replace('\\', "/"), c, &root);
        // `strobe run --base/--head`'s bench file: leased while a run uses it, and needed by an
        // experiment of it that can still resume.
        if n.starts_with("cmd-") && root == dir {
            e.how = Used::Cache;
            e.leases = vec![p.clone()];
            e.keep = if cache::in_use(&p) {
                why("in use by a running strobe", "")
            } else {
                let saved = resumable.get_or_init(|| resumable_states(state));
                saved.iter().find(|(_, text)| text.contains(n.as_str())).map(|(x, _)| Why { what: "its experiment can resume".into(), detail: format!("strobe bench --resume {x}") })
            };
            out.push(e);
            continue;
        }
        match isolate::folder_gone(&p) {
            Some(true) => e.removal = Removal::Launch,
            Some(false) => e.keep = why("its strobe runs", ""),
            None => {
                let pid = RUN_PID.iter().find_map(|pre| n.strip_prefix(pre)).and_then(|r| r.split(['-', '.']).next()).and_then(|d| d.parse::<u32>().ok());
                match pid {
                    Some(pid) if strobe_alive(pid) => e.keep = why("its strobe runs", format!("pid {pid}")),
                    Some(_) => {}
                    None if e.within(TEMP_AGE) => e.keep = why("a temporary file from the last day", ago(e.used())),
                    None => {}
                }
            }
        }
        out.push(e);
    }
}

/// The experiments each recording is named in (`result.json`, `state.json`), and the experiments
/// tracked history names (`history/`): what a removal must not break.
fn references(entries: &[Entry], state: &Path) -> Refs {
    let names = |c: &str| entries.iter().filter(|e| CATEGORIES[e.cat].name == c).map(|e| e.name.as_str()).collect::<HashSet<&str>>();
    let (recordings, experiments) = (names("recordings"), names("experiments"));
    let mut refs = Refs::default();
    let add = |to: &mut HashMap<String, Vec<String>>, what: &str, by: &str| {
        let v = to.entry(what.to_string()).or_default();
        if !v.iter().any(|x| x == by) {
            v.push(by.to_string());
        }
    };
    for e in entries.iter().filter(|e| CATEGORIES[e.cat].name == "experiments") {
        for f in ["result.json", "state.json"] {
            let Ok(text) = std::fs::read_to_string(e.paths[0].join(f)) else { continue };
            // Every handle it holds, whatever its prefix (`bench --id` names an experiment freely; a
            // bisect's steps are `<exp>_s<k>`).
            for t in handles(&text) {
                if recordings.contains(t) {
                    add(&mut refs.recordings, t, &e.name);
                } else if t != e.name && experiments.contains(t) {
                    add(&mut refs.experiments, t, &e.name);
                }
            }
        }
    }
    let mut stack = vec![state.join("history")];
    while let Some(d) = stack.pop() {
        for (p, _) in children(&d) {
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(text) = std::fs::read_to_string(&p) {
                refs.tracked.extend(handles(&text).filter(|t| experiments.contains(t)).map(str::to_string));
            }
        }
    }
    refs
}

/// Every run of letters, digits, `_` and `-` in `text`: the handles it may hold.
fn handles(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).filter(|t| t.len() > 3)
}

/// Everything strobe keeps, measured.
pub struct Disk {
    state: PathBuf,
    entries: Vec<Entry>,
    /// Per category: what no entry holds (kept); `cut` when the deadline came first.
    loose: Vec<disk::Sized>,
    /// Bytes of Snap Firefox's profiles no entry holds (outside the state folder, in `run`'s loose).
    snap: u64,
    tally: disk::Tally,
    free: Option<u64>,
    took: Duration,
    /// Of `took`, listing the entries and why each stays.
    listed: Duration,
    refs: Option<Refs>,
}

/// What a removal must not break: the experiments naming each recording, and each experiment (a
/// bisect's steps), and the experiments tracked history names.
#[derive(Default)]
struct Refs {
    recordings: HashMap<String, Vec<String>>,
    experiments: HashMap<String, Vec<String>>,
    tracked: HashSet<String>,
}

fn add(to: &mut disk::Sized, s: disk::Sized) {
    to.bytes += s.bytes;
    to.files += s.files;
    to.newest = to.newest.max(s.newest);
    to.cut |= s.cut;
}

/// A build entry of at least this many files keeps its walk in `disk-cache.json`, as every recording
/// and experiment does: a walk of every file of a 116 GB state folder (2.1 M files, nearly all in
/// build worktrees) took 14 to 25 s.
const KEEP_FILES: u64 = 1000;
/// A kept walk is used for at most this long, and only while its fingerprint holds.
const KEEP_FOR: Duration = Duration::from_secs(86_400);
/// How `disk-cache.json` counts: a walk kept by another way of counting (an earlier build's) is not used.
const CACHE_FORM: u64 = 1;

fn cache_file(state: &Path) -> PathBuf {
    state.join("disk-cache.json")
}

/// What could change an entry, seen without walking it: its paths and what each holds directly
/// (names, kinds, sizes, times) and their stamps (`host::cache`), which every use by strobe touches,
/// where a build changes files deep inside.
fn fingerprint(paths: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut item = |i: &disk::Item| {
        i.name.hash(&mut h);
        (i.kind as u8, i.len, i.bytes, i.modified).hash(&mut h);
    };
    for p in paths {
        if let Ok(s) = disk::stat(p) {
            item(&s);
            if s.kind == disk::Kind::Dir {
                disk::list(p).unwrap_or_default().iter().for_each(&mut item);
            }
        }
        if let Ok(s) = disk::stat(&cache::stamp(p)) {
            item(&s);
        }
    }
    h.finish()
}

/// Walks the state folder and strobe's files outside it: each entry, then what no entry holds. An
/// entry whose fingerprint is what it was at its last walk, less than a day ago, counts that walk's
/// files (`disk-cache.json`). With a budget, the entries not reached before it runs out stay
/// unmeasured: the total is then a floor. `fresh` walks every entry again (`status --disk`).
pub fn measure(budget: Option<Duration>, fresh: bool) -> Disk {
    let started = std::time::Instant::now();
    let state = paths::state_path();
    let live = Live::read();
    let mut entries = entries(&state, &live);
    let listed = started.elapsed();
    let walking = std::time::Instant::now();
    // The deadline holds inside each walk too: one 300 k-file worktree cannot blow the time status has.
    let mut tally = disk::Tally::default();
    tally.deadline = budget.map(|b| walking + b);
    // A kept walk is used only from this way of counting (`CACHE_FORM`).
    let kept = store::read_json(&cache_file(&state)).ok().filter(|k| k["form"] == CACHE_FORM).map(|k| k["entries"].clone()).unwrap_or(Value::Null);
    let mut keep = serde_json::Map::new();
    let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Builds last: a budget that runs out leaves the costly ones, not every small entry after them.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|i| CATEGORIES[entries[*i].cat].name == "builds");
    for i in order {
        let e = &mut entries[i];
        let k = &kept[&e.name];
        // Taken before a walk, so a change during it shows next time. A recording or an experiment gets
        // each file whole or not at all (a rename into its folder), which moves the folder's own time,
        // read from its parent's listing at no cost; a build's worktree changes deep inside, seen by
        // its stamp, which every use touches. The few small others are walked each time.
        let fp = match e.class() {
            Class::Data => Some(format!("{:?}", e.seen.map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)))),
            _ if CATEGORIES[e.cat].name == "builds" => Some(fingerprint(&e.paths).to_string()),
            _ => None,
        };
        if !fresh && fp.is_some() && k["fp"].as_str() == fp.as_deref() && k["at"].as_u64().is_some_and(|at| now.saturating_sub(at) < KEEP_FOR.as_secs()) {
            let shared: Vec<[u64; 5]> = serde_json::from_value(k["shared"].clone()).unwrap_or_default();
            e.size.bytes = tally.restore(i, k["alone"].as_u64().unwrap_or(0), &shared);
            e.size.files = k["files"].as_u64().unwrap_or(0);
            keep.insert(e.name.clone(), k.clone());
            continue;
        }
        if budget.is_some_and(|b| walking.elapsed() >= b) {
            e.unmeasured = true;
            continue;
        }
        for p in e.paths.clone() {
            add(&mut e.size, tally.walk(&p, i));
        }
        // Cut at the deadline: what it found is a floor, and never kept as its walk.
        if e.size.cut {
            e.unmeasured = true;
            continue;
        }
        // Not one still being written (a running experiment appends to its logs, which moves no folder time).
        let settled = !(e.class() == Class::Data && e.keep.is_some());
        if let Some(fp) = fp.filter(|_| settled && (e.class() == Class::Data || e.size.files >= KEEP_FILES)) {
            let (alone, shared) = tally.kept_of(i);
            keep.insert(e.name.clone(), json!({ "fp": fp, "at": now, "alone": alone, "files": e.size.files, "shared": shared }));
        }
    }
    let _ = paths::write_atomic(&cache_file(&state), json!({ "form": CACHE_FORM, "entries": keep }).to_string().as_bytes());
    let n = entries.len();
    let claimed: HashSet<PathBuf> = entries.iter().flat_map(|e| e.paths.iter().cloned()).collect();
    let mut loose = vec![disk::Sized::default(); CATEGORIES.len()];
    // The state folder's own listing through what it ends on: a state folder that is a junction.
    let real = std::fs::canonicalize(&state).unwrap_or_else(|_| state.clone());
    for p in disk::list(&real).unwrap_or_default().into_iter().map(|i| state.join(i.name)) {
        let c = match name_of(&p).as_str() {
            "log" => index("logs"),
            name if CATEGORIES.iter().any(|c| c.name == name && c.class != Class::Kept) || name == "history" => index(name),
            _ => index("other"),
        };
        add(&mut loose[c], tally.walk_except(&p, n + c, &claimed, 3));
    }
    // Snap Firefox's profiles: outside the state folder, counted with `run` but not as in it.
    let snap_dir = crate::target::firefox::snap_profiles();
    let mut snap = disk::Sized::default();
    if !snap_dir.starts_with(&state) {
        snap = tally.walk_except(&snap_dir, n + index("run"), &claimed, 1);
        add(&mut loose[index("run")], snap);
    }
    let c = index("install");
    for p in install_files() {
        add(&mut loose[c], tally.walk(&p, n + c));
    }
    Disk { free: disk::free(&state), state, entries, loose, snap: snap.bytes, tally, took: started.elapsed(), listed, refs: None }
}

/// strobe's own files outside the state folder: its executables (and old copies an update set
/// aside), the deep-mode helper, the skill.
fn install_files() -> Vec<PathBuf> {
    let dir = super::up::install_dir();
    let mut out: Vec<PathBuf> = if cfg!(windows) {
        vec![dir]
    } else {
        children(&dir).into_iter().map(|c| c.0).filter(|p| ["strobe", "strobew"].iter().any(|n| name_of(p) == *n || name_of(p).starts_with(&format!("{n}.")))).collect()
    };
    #[cfg(windows)]
    {
        let h = crate::probe::helper::helper_exe();
        out.extend(h.parent().map(children).unwrap_or_default().into_iter().map(|c| c.0));
    }
    if let Some(skill) = super::up::skill_path().parent() {
        out.push(skill.to_path_buf());
    }
    out
}

/// What a prune asks for.
pub struct Ask {
    cats: Vec<usize>,
    older: Option<Duration>,
}

impl Ask {
    /// `strobe prune` with no category: every cache, unused now.
    fn caches() -> Ask {
        Ask { cats: (0..CATEGORIES.len()).filter(|i| CATEGORIES[*i].class == Class::Cache).collect(), older: None }
    }

    fn from(args: &Args) -> Result<Ask> {
        let older = args.get("older-than").map(crate::cli::duration_ms).transpose()?.map(Duration::from_millis);
        let mut cats = Vec::new();
        for p in &args.pos {
            let Some(i) = CATEGORIES.iter().position(|c| c.name == p.as_str() && c.class != Class::Kept) else {
                let names: Vec<&str> = CATEGORIES.iter().filter(|c| c.class != Class::Kept).map(|c| c.name).collect();
                return Err(
                    Error::usage(format!("prune has no category {p}: {}", names.join(", "))).hint("strobe prune with none removes every cache no process uses; --all adds recordings and experiments")
                );
            };
            if !cats.contains(&i) {
                cats.push(i);
            }
        }
        if args.has("all") {
            cats = (0..CATEGORIES.len()).filter(|i| CATEGORIES[*i].class != Class::Kept).collect();
        } else if cats.is_empty() {
            cats = Ask::caches().cats;
        }
        Ok(Ask { cats, older })
    }

    fn data_age(&self) -> Duration {
        self.older.unwrap_or(DATA_AGE)
    }
}

/// Which entries a prune removes, and why each other one stays.
struct Plan {
    remove: Vec<usize>,
    kept: Vec<(usize, Why)>,
}

impl Disk {
    fn refs(&mut self) {
        if self.refs.is_none() {
            self.refs = Some(references(&self.entries, &self.state));
        }
    }

    fn plan(&mut self, ask: &Ask) -> Plan {
        if ask.cats.iter().any(|c| CATEGORIES[*c].class == Class::Data) {
            self.refs();
        }
        let mut remove = Vec::new();
        let mut kept = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !ask.cats.contains(&e.cat) {
                continue;
            }
            let decided = if let Some(w) = &e.keep {
                Some(w.clone())
            } else if e.class() == Class::Data && e.within(ask.data_age()) {
                why(format!("used within {}", days(ask.data_age())), ago(e.used()))
            } else if let Some(o) = ask.older.filter(|o| e.within(*o)) {
                why(format!("used within {}", days(o)), ago(e.used()))
            } else if CATEGORIES[e.cat].name == "experiments" && self.refs.as_ref().is_some_and(|r| r.tracked.contains(&e.name)) {
                why("named by tracked history", "strobe history")
            } else {
                None
            };
            match decided {
                Some(w) => kept.push((i, w)),
                None => remove.push(i),
            }
        }
        // What an experiment that stays names stays with it, its handles would break: an experiment
        // (a bisect's steps), again for what each one kept names, then the recordings any of them name.
        if let Some(refs) = &self.refs {
            loop {
                let gone: HashSet<&str> = remove.iter().map(|i| self.entries[*i].name.as_str()).collect();
                let mut keep_too = Vec::new();
                for &i in &remove {
                    let e = &self.entries[i];
                    let (by, what) = match CATEGORIES[e.cat].name {
                        "experiments" => (&refs.experiments, "named by an experiment that stays"),
                        "recordings" => (&refs.recordings, "named by an experiment that stays"),
                        _ => continue,
                    };
                    if let Some(exp) = by.get(&e.name).and_then(|v| v.iter().find(|x| !gone.contains(x.as_str()))) {
                        keep_too.push((i, Why { what: what.into(), detail: exp.clone() }));
                    }
                }
                if keep_too.is_empty() {
                    break;
                }
                remove.retain(|i| !keep_too.iter().any(|k| k.0 == *i));
                kept.extend(keep_too);
            }
        }
        Plan { remove, kept }
    }

    /// Bytes each entry of the plan frees: files whose every name lies in the removed entries.
    fn frees(&self, plan: &Plan) -> HashMap<usize, u64> {
        let set: HashSet<usize> = plan.remove.iter().copied().collect();
        self.tally.freed(|o| set.contains(&o))
    }

    fn cat_bytes(&self, c: usize) -> (u64, u64, usize) {
        let mine: Vec<&Entry> = self.entries.iter().filter(|e| e.cat == c).collect();
        let bytes = self.loose[c].bytes + mine.iter().map(|e| e.size.bytes).sum::<u64>();
        let files = self.loose[c].files + mine.iter().map(|e| e.size.files).sum::<u64>();
        (bytes, files, mine.len())
    }

    pub fn total(&self) -> u64 {
        (0..CATEGORIES.len()).map(|c| self.cat_bytes(c).0).sum()
    }

    /// `strobe status`'s disk lines and JSON: the total, what a plain `strobe prune` frees, and with
    /// `detail` each category with what `strobe prune <category>` frees.
    pub fn status(&mut self, detail: bool) -> (String, Value) {
        let plan = self.plan(&Ask::caches());
        let freed = self.frees(&plan);
        let reclaim: u64 = freed.values().sum();
        let total = self.total();
        // Outside the state folder: strobe's own files, and Snap Firefox's profiles (entries and the rest).
        let outside = self.cat_bytes(index("install")).0 + self.snap + self.entries.iter().filter(|e| !e.root.starts_with(&self.state)).map(|e| e.size.bytes).sum::<u64>();
        // What the deadline left out, per category: entries not walked or cut, and a cut walk of the rest.
        let missing: Vec<usize> = (0..CATEGORIES.len()).map(|c| self.entries.iter().filter(|e| e.cat == c && e.unmeasured).count() + usize::from(self.loose.get(c).is_some_and(|l| l.cut))).collect();
        let missing = |c: usize| missing[c];
        let unmeasured: usize = (0..CATEGORIES.len()).map(missing).sum();
        // A floor is never given as an exact number: `bytes` is null and `at_least_bytes` says it.
        let exact = |n: u64, whole: bool| if whole { json!(n) } else { Value::Null };
        let mut cats = Vec::new();
        let mut rows = vec![vec!["".to_string(), "size".into(), "files".into(), "entries".into(), "reclaimable".into(), "".into()]];
        for (c, k) in CATEGORIES.iter().enumerate() {
            let (bytes, files, n) = self.cat_bytes(c);
            let r = match k.class {
                Class::Cache => Some(plan.remove.iter().filter(|i| self.entries[**i].cat == c).map(|i| freed.get(i).copied().unwrap_or(0)).sum::<u64>()),
                Class::Data if detail => {
                    let p = self.plan(&Ask { cats: vec![c], older: None });
                    Some(self.frees(&p).values().sum())
                }
                _ => None,
            };
            let whole = missing(c) == 0;
            cats.push(json!({
                "name": k.name, "class": k.class.name(), "bytes": exact(bytes, whole), "at_least_bytes": bytes, "files": files, "entries": n,
                "reclaimable_bytes": r.and_then(|r| whole.then_some(r)), "reclaimable_at_least_bytes": r, "unmeasured": missing(c), "complete": whole, "what": k.what,
            }));
            let bytes = bytes as f64;
            let floor = if whole { "" } else { "≥ " };
            let note = match k.class {
                Class::Data if detail => {
                    format!("strobe prune {}: older than {}, unless {} names it", k.name, days(DATA_AGE), if k.name == "recordings" { "an experiment that stays" } else { "tracked history" })
                }
                Class::Kept => "never pruned".into(),
                _ => String::new(),
            };
            rows.push(vec![
                k.name.to_string(),
                format!("{floor}{}", super::fmt::bytes(bytes)),
                files.to_string(),
                n.to_string(),
                r.map(|r| format!("{floor}{}", super::fmt::bytes(r as f64))).unwrap_or_else(|| "-".into()),
                note,
            ]);
        }
        let hint = reclaim >= 5 << 30 || self.free.is_some_and(|f| reclaim * 10 >= f && reclaim > 0);
        let floor = if unmeasured > 0 { "at least " } else { "" };
        let mut t = format!(
            "  {} disk       {floor}{} ({floor}{} in {}{}) · {floor}{} reclaimable{} · {} free on its disk\n",
            if hint { "!" } else { "✓" },
            super::fmt::bytes(total as f64),
            super::fmt::bytes((total - outside) as f64),
            paths::plain(&self.state),
            if outside > 0 { format!(", {floor}{} outside it", super::fmt::bytes(outside as f64)) } else { String::new() },
            super::fmt::bytes(reclaim as f64),
            if hint { ": strobe prune frees it" } else { "" },
            self.free.map(|f| super::fmt::bytes(f as f64)).unwrap_or_else(|| "?".into()),
        );
        if unmeasured > 0 {
            t += &format!("               {unmeasured} entries not measured within the time status gives the walk: strobe status --disk measures all\n");
        }
        let refused = auto_age().err();
        if let Some(why) = &refused {
            t += &format!("               caches are not pruned on their own: {why}\n");
        }
        if detail {
            t += &super::fmt::table(&rows, &[false, true, true, true, true, false], "               ", 2);
            t += "               sizes on the disk, each file once, every entry walked now\n";
        }
        let whole = unmeasured == 0;
        let json = json!({
            "state": paths::plain(&self.state), "total_bytes": exact(total, whole), "at_least_bytes": total, "outside_bytes": exact(outside, whole),
            "reclaimable_bytes": exact(reclaim, whole), "reclaimable_at_least_bytes": reclaim, "free_bytes": self.free,
            "complete": whole, "unmeasured": unmeasured, "auto_refused": refused,
            "seconds": (self.took.as_secs_f64() * 100.0).round() / 100.0, "list_seconds": (self.listed.as_secs_f64() * 100.0).round() / 100.0, "categories": cats, "auto": last_auto(),
        });
        (t, json)
    }
}

/// `strobe prune [<category>…] [--all] [--older-than <dur>] [--dry-run] [-f]`.
pub fn run(args: &Args) -> Result<Output> {
    let ask = Ask::from(args)?;
    // A plan alone removes nothing: it takes no lock and makes no lock file. Another prune running is no
    // usage mistake (exit 1).
    let lock = if args.has("dry-run") { None } else { Some(PruneLock::take().ok_or_else(|| Error::msg("another strobe prune runs on this state folder").hint("wait for it to end"))?) };
    let mut d = measure(None, false);
    let plan = d.plan(&ask);
    let freed = d.frees(&plan);
    let (mut text, mut json) = plan_text(&d, &ask, &plan, &freed);
    let total: u64 = freed.values().sum();
    use std::io::IsTerminal;
    let tty = std::io::stdin().is_terminal() && !args.json();
    let go = if plan.remove.is_empty() || args.has("dry-run") {
        false
    } else if args.has("force") {
        true
    } else if tty {
        let n = plan.remove.len();
        eprint!("{text}Remove {n} {}, {}? [y/N] ", if n == 1 { "entry" } else { "entries" }, super::fmt::bytes(total as f64));
        text.clear();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim(), "y" | "Y" | "yes")
    } else {
        text += "nothing removed: no terminal to confirm on; strobe prune --force removes these\n";
        json["removed"] = json!([]);
        json["confirmed"] = json!(false);
        let mut out = Output::new(text, json);
        out.code = 3;
        return Ok(out);
    };
    if !go {
        if !plan.remove.is_empty() {
            text += if args.has("dry-run") { "dry run: nothing removed\n" } else { "nothing removed\n" };
        }
        json["removed"] = json!([]);
        return Ok(Output::new(text, json));
    }
    let done = apply(&d, &plan.remove, super::progress, || None);
    drop(lock);
    text += &done.text();
    json["removed"] = json!(done.removed);
    json["kept_now"] = json!(done.kept);
    json["failed"] = json!(done.failed);
    json["reclaimed_bytes"] = json!(done.freed);
    json["free_after_bytes"] = json!(disk::free(&d.state));
    json["ended"] = json!(done.ended);
    d.entries.clear();
    let mut out = Output::new(text, json);
    if !done.failed.is_empty() {
        out.code = 1;
    }
    Ok(out)
}

fn plan_text(d: &Disk, ask: &Ask, plan: &Plan, freed: &HashMap<usize, u64>) -> (String, Value) {
    let mut t = format!("strobe prune · {}\n", paths::plain(&d.state));
    let mut rows = vec![vec!["remove".to_string(), "entries".into(), "frees".into(), "".into()]];
    let mut cats = Vec::new();
    for (c, k) in CATEGORIES.iter().enumerate() {
        let gone: Vec<usize> = plan.remove.iter().copied().filter(|i| d.entries[*i].cat == c).collect();
        let kept: Vec<&(usize, Why)> = plan.kept.iter().filter(|(i, _)| d.entries[*i].cat == c).collect();
        let named = ask.cats.contains(&c);
        let not_named = d.entries.iter().filter(|e| e.cat == c).count();
        if !named && (k.class == Class::Kept || not_named == 0) {
            continue;
        }
        let bytes: u64 = gone.iter().map(|i| freed.get(i).copied().unwrap_or(0)).sum();
        let mut reasons: Vec<(String, usize, u64)> = Vec::new();
        for (i, w) in &kept {
            match reasons.iter_mut().find(|r| r.0 == w.what) {
                Some(r) => {
                    r.1 += 1;
                    r.2 += d.entries[*i].size.bytes;
                }
                None => reasons.push((w.what.clone(), 1, d.entries[*i].size.bytes)),
            }
        }
        let keep = if named {
            reasons.iter().map(|(w, n, b)| format!("{n} {w} ({})", super::fmt::bytes(*b as f64))).collect::<Vec<_>>().join(" · ")
        } else {
            let b: u64 = d.entries.iter().filter(|e| e.cat == c).map(|e| e.size.bytes).sum();
            format!(
                "{not_named} kept ({}): not named; strobe prune {} removes those older than {}{}",
                super::fmt::bytes(b as f64),
                k.name,
                days(ask.older.unwrap_or(DATA_AGE)),
                if k.name == "recordings" { " that no experiment that stays names" } else { "" }
            )
        };
        rows.push(vec![k.name.to_string(), gone.len().to_string(), super::fmt::bytes(bytes as f64), if keep.is_empty() { String::new() } else { format!("kept: {keep}") }]);
        cats.push(json!({
            "name": k.name, "class": k.class.name(), "named": named, "remove": gone.len(), "frees_bytes": bytes,
            "kept": reasons.iter().map(|(w, n, b)| json!({ "why": w, "entries": n, "bytes": b })).collect::<Vec<_>>(),
        }));
    }
    let total: u64 = freed.values().sum();
    rows.push(vec!["total".into(), plan.remove.len().to_string(), super::fmt::bytes(total as f64), "".into()]);
    t += &super::fmt::table(&rows, &[false, true, true, false], "  ", 2);
    // What a live process uses is named one by one: it is what a person may want to stop first.
    for (i, w) in plan.kept.iter().filter(|(_, w)| w.what.contains("running") || w.what.contains("runs") || w.what.contains("in use")) {
        t += &format!("  in use     {} ({}){}\n", d.entries[*i].name, w.what, if w.detail.is_empty() { String::new() } else { format!(": {}", w.detail) });
    }
    if ask.cats.iter().any(|c| CATEGORIES[*c].class == Class::Data) {
        t += &format!("  data older than {} goes (last use: its newest file); --older-than sets the age\n", days(ask.data_age()));
    }
    let entry = |i: usize| {
        let e = &d.entries[i];
        json!({ "name": e.name, "category": CATEGORIES[e.cat].name, "bytes": e.size.bytes, "frees_bytes": freed.get(&i), "files": e.size.files, "used": e.used().map(iso), "paths": e.paths.iter().map(|p| paths::plain(p)).collect::<Vec<_>>() })
    };
    let json = json!({
        "state": paths::plain(&d.state), "older_than_s": ask.older.map(|o| o.as_secs()), "data_age_s": ask.data_age().as_secs(),
        "categories": cats, "frees_bytes": total, "free_bytes": d.free, "seconds": (d.took.as_secs_f64() * 100.0).round() / 100.0,
        "remove": plan.remove.iter().map(|i| entry(*i)).collect::<Vec<_>>(),
        "kept": plan.kept.iter().map(|(i, w)| { let mut v = entry(*i); v["why"] = json!(w.what); v["detail"] = json!(w.detail); v }).collect::<Vec<_>>(),
    });
    (t, json)
}

fn iso(t: SystemTime) -> String {
    let s = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (s / 86_400, s % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// What a removal did.
#[derive(Default)]
struct Done {
    removed: Vec<Value>,
    kept: Vec<Value>,
    failed: Vec<Value>,
    ended: Vec<String>,
    freed: u64,
    per_cat: HashMap<usize, (usize, u64)>,
    /// Why the removal ended before the last entry (a bench started).
    stopped: Option<String>,
}

impl Done {
    fn text(&self) -> String {
        let mut t = String::new();
        if let Some(why) = &self.stopped {
            t += &format!("  stopped    {why}\n");
        }
        for l in &self.ended {
            t += &format!("  {l}\n");
        }
        for (c, k) in CATEGORIES.iter().enumerate() {
            if let Some((n, b)) = self.per_cat.get(&c) {
                t += &format!("  removed    {n} {} ({})\n", k.name, super::fmt::bytes(*b as f64));
            }
        }
        for k in &self.kept {
            t += &format!("  kept       {}: {}\n", k["name"].as_str().unwrap_or(""), k["why"].as_str().unwrap_or(""));
        }
        for f in &self.failed {
            t += &format!("  failed     {}: {}\n", f["name"].as_str().unwrap_or(""), f["errors"].as_array().and_then(|a| a.first()).and_then(Value::as_str).unwrap_or(""));
        }
        t += &format!("Total reclaimed space: {}\n", super::fmt::bytes(self.freed as f64));
        t
    }
}

/// Removes the entries, each under its stamps' exclusive holds (an entry a process started to use
/// meanwhile stays), launch folders after what runs from them ended, worktrees out of their
/// repository's list too. What is freed is counted file by file as it goes.
/// `stop` is asked before each entry: what it names (a bench that started) ends the removal there.
fn apply(d: &Disk, chosen: &[usize], say: impl Fn(String), stop: impl Fn() -> Option<String>) -> Done {
    let mut done = Done::default();
    // Launch folders first: they hold links to seed copies, which then free their bytes.
    let mut order: Vec<usize> = chosen.to_vec();
    order.sort_by_key(|i| (d.entries[*i].removal != Removal::Launch, d.entries[*i].cat));
    // What started since the plan (a person may have taken minutes to answer): a launch that borrows a
    // seed copy or a GPU cache store, a bench of a build that takes no leases or of the experiment a
    // store, a recording or an experiment belongs to.
    // Read again before each entry: the registry, the slot and the launch folders (the daemon's build
    // once).
    let mut live = Live::read();
    for (n, i) in order.into_iter().enumerate() {
        if let Some(why) = stop() {
            done.stopped = Some(why);
            break;
        }
        if n > 0 {
            live.local();
        }
        let e = &d.entries[i];
        let cat = CATEGORIES[e.cat].name;
        let started = match cat {
            "seeds" => live.borrowed.iter().any(|b| same_or_under(b, &e.paths[0])),
            "gpu-cache" => {
                let exp = e.name.trim_start_matches("gpu-cache/").rsplit_once('-').map(|(x, _)| x).filter(|x| *x != "rec");
                live.borrowed.iter().any(|b| same_or_under(b, &e.paths[0])) || exp.is_some_and(|x| live.running(x).is_some() || resumable(&d.state, x))
            }
            "builds" => live.unleased.is_some(),
            "engine" => live.unleased.is_some() || live.other_daemon.is_some(),
            "experiments" => live.running(&e.name).is_some(),
            "recordings" => {
                let meta = e.paths[0].join("meta.json");
                !meta.is_file() || (!live.benches.is_empty() && store::read_json(&meta).ok().and_then(|m| m["experiment"].as_str().map(str::to_string)).is_some_and(|x| live.running(&x).is_some()))
            }
            // A `strobe run` bench file an experiment that started (and stopped) meanwhile names.
            "run" if e.name.starts_with("run/cmd-") => {
                let n = name_of(&e.paths[0]);
                resumable_states(&d.state).iter().any(|(_, text)| text.contains(n.as_str()))
            }
            _ => false,
        };
        if started {
            done.kept.push(json!({ "name": e.name, "why": "in use by a strobe that started meanwhile" }));
            continue;
        }
        let mut holds = Vec::new();
        let mut busy = false;
        for l in &e.leases {
            match cache::hold(l) {
                Some(h) => holds.push(h),
                None => {
                    busy = true;
                    break;
                }
            }
        }
        if busy {
            holds.into_iter().for_each(cache::Hold::kept);
            done.kept.push(json!({ "name": e.name, "why": "in use by a strobe that started meanwhile" }));
            continue;
        }
        if e.removal == Removal::Launch {
            done.ended.extend(isolate::release_gone(&e.paths[0]));
        }
        let mut r = disk::Removed::default();
        for p in &e.paths {
            let one = match e.removal {
                Removal::Store => store::remove(p),
                _ => disk::remove(p, &e.root),
            };
            r.freed += one.freed;
            r.files += one.files;
            r.failed.extend(one.failed);
        }
        if r.failed.is_empty() && e.leases.is_empty() {
            let _ = std::fs::remove_file(cache::stamp(&e.paths[0]));
        }
        if let Some((common, admin, real)) = &e.worktree
            && r.failed.is_empty()
        {
            unregister(common, admin, &e.paths[0], real, &mut r);
        }
        // A symbols group left empty goes too.
        if CATEGORIES[e.cat].name == "symbols"
            && let Some(parent) = e.paths[0].parent()
        {
            let _ = std::fs::remove_dir(parent);
        }
        let ok = r.failed.is_empty();
        for h in holds {
            if ok { h.removed() } else { h.kept() }
        }
        done.freed += r.freed;
        let c = done.per_cat.entry(e.cat).or_default();
        c.1 += r.freed;
        if ok {
            c.0 += 1;
            done.removed.push(json!({ "name": e.name, "category": CATEGORIES[e.cat].name, "freed_bytes": r.freed, "files": r.files }));
            say(format!("  removed {} ({})", e.name, super::fmt::bytes(r.freed as f64)));
        } else {
            done.failed
                .push(json!({ "name": e.name, "category": CATEGORIES[e.cat].name, "freed_bytes": r.freed, "errors": r.failed.iter().take(5).collect::<Vec<_>>(), "error_count": r.failed.len() }));
        }
    }
    done
}

/// Takes a removed worktree out of its repository's list (`git worktree remove`, which accepts a
/// folder already gone), so the next `git worktree add` of that path does not fail on a stale entry.
/// When git cannot (no git, the repository moved), its own record of the worktree goes by hand, only
/// when it names this folder.
fn unregister(common: &Path, admin: &Path, dir: &Path, real: &Path, r: &mut disk::Removed) {
    // As a person types the paths: git takes no `\\?\` prefix.
    let (git_dir, folder) = (paths::plain(common), paths::plain(dir));
    let _ = host::git::out(Path::new(&git_dir), &["--git-dir", &git_dir, "worktree", "remove", "--force", "--force", &folder]);
    if !admin.exists() {
        return;
    }
    // The record's `gitdir` names the worktree's `.git`, relative to the record itself with git's
    // worktree.useRelativePaths: resolved against the record, never the folder strobe runs in.
    let names = std::fs::read_to_string(admin.join("gitdir")).ok().is_some_and(|g| {
        let at = lexical(&admin.join(g.trim()));
        same_or_under(&at, &real.join(".git")) || same_or_under(&at, &dir.join(".git"))
    });
    if names {
        let one = disk::remove(admin, &common.join("worktrees"));
        r.failed.extend(one.failed);
    }
}

/// One prune at a time per state folder (`run/prune.lock`).
struct PruneLock(std::fs::File);

impl PruneLock {
    fn take() -> Option<PruneLock> {
        let f = std::fs::File::options().create(true).truncate(false).write(true).open(paths::run().join("prune.lock")).ok()?;
        f.try_lock().ok()?;
        Some(PruneLock(f))
    }
}

impl Drop for PruneLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// `prune.unused` in the user's own strobe.json: how long a cache may go unused before it goes on its
/// own (`"14d"`, the default), or `"off"`.
/// A user strobe.json that does not parse refuses it: the 14-day default must never come back behind
/// an `"off"` the file meant to say.
fn auto_age() -> std::result::Result<Option<Duration>, String> {
    let prune = crate::config::user_strict("prune")?;
    match &prune["unused"] {
        Value::Null => Ok(Some(AUTO_AGE)),
        Value::String(s) if s == "off" => Ok(None),
        Value::String(s) => match crate::cli::duration_ms(s) {
            // An age of nothing would remove every cache the moment nothing uses it.
            Ok(0) => Err(format!("prune.unused in your strobe.json is {s:?}: write \"off\" to turn automatic pruning off, or an age such as \"14d\"")),
            Ok(ms) => Ok(Some(Duration::from_millis(ms))),
            Err(e) => Err(format!("prune.unused in your strobe.json: {}", e.message)),
        },
        other => Err(format!("prune.unused in your strobe.json must be \"off\" or an age such as \"14d\", not {other}")),
    }
}

fn auto_file() -> PathBuf {
    paths::state_path().join("prune.json")
}

fn last_auto() -> Value {
    store::read_json(&auto_file()).unwrap_or(Value::Null)
}

/// Caches unused for `prune.unused` (14 days) go, at most once a day, when no bench runs or waits:
/// after a bench, track or run from the command line, and in the daemon. Returns the line to say
/// when it removed anything.
pub fn auto() -> Option<String> {
    let age = match auto_age() {
        Ok(Some(a)) => a,
        Ok(None) => return None,
        Err(e) => return Some(format!("strobe: {e}; caches are not pruned on their own until it reads")),
    };
    let day = Duration::from_secs(86_400);
    let at = last_auto()["at"].as_u64().map(|s| std::time::UNIX_EPOCH + Duration::from_secs(s));
    // Never beside a bench or a timed recording anywhere on the machine (rule 2): its blocks would
    // share the disk and CPUs with tens of GB of deletion.
    if at.and_then(|t| t.elapsed().ok()).is_some_and(|e| e < day) || super::bench::slot::busy().is_some() {
        return None;
    }
    let lock = PruneLock::take()?;
    let live = Live::read();
    let entries = entries(&paths::state_path(), &live);
    let now = SystemTime::now();
    let chosen: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.class() == Class::Cache && e.keep.is_none() && e.used().and_then(|t| now.duration_since(t).ok()).is_some_and(|a| a >= age))
        .map(|(i, _)| i)
        .collect();
    let d = Disk { state: paths::state_path(), entries, loose: Vec::new(), snap: 0, tally: disk::Tally::default(), free: None, took: Duration::ZERO, listed: Duration::ZERO, refs: None };
    // Asked again before each entry: a bench that starts meanwhile ends it there.
    let done = apply(&d, &chosen, |_| {}, super::bench::slot::busy);
    drop(lock);
    let secs = now.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // A removal a bench stopped goes on another time (the day starts again once it is done).
    if done.stopped.is_none() {
        let _ = paths::write_atomic(
            &auto_file(),
            json!({ "at": secs, "unused_s": age.as_secs(), "removed": done.removed.len(), "freed_bytes": done.freed, "failed": done.failed.len() }).to_string().as_bytes(),
        );
    }
    if done.removed.is_empty() && done.failed.is_empty() {
        return None;
    }
    let parts: Vec<String> = CATEGORIES.iter().enumerate().filter_map(|(c, k)| done.per_cat.get(&c).filter(|p| p.0 > 0).map(|p| format!("{} {}", p.0, k.name))).collect();
    Some(format!(
        "strobe: pruned {} unused for {}: {}{} (prune.unused in your strobe.json changes the age or turns it off)",
        parts.join(", "),
        days(age),
        super::fmt::bytes(done.freed as f64),
        if done.failed.is_empty() { String::new() } else { format!("; {} could not be removed (strobe prune --dry-run lists them)", done.failed.len()) }
    ))
}
