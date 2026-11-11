//! Uses of strobe's caches (SPEC §5.14). An entry (a build's worktree, a built executable's copy, a
//! compile cache, a seed copy) has a stamp beside it, `<entry>.used`: its modification time is when
//! strobe last used the entry, and a process using it holds a shared lock on it for as long as it
//! does. `strobe prune` removes an entry only while it holds the stamp's exclusive lock, so it never
//! takes what a live process uses, and a process that starts to use one waits for a removal under way.
//! The OS drops a dead process's lock.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The stamp of a cache entry: `<entry>.used` beside it.
pub fn stamp(entry: &Path) -> PathBuf {
    let name = entry.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    entry.with_file_name(format!("{name}.used"))
}

/// Marks the entry used now.
pub fn touch(entry: &Path) {
    if let Ok(f) = File::options().create(true).truncate(false).write(true).open(stamp(entry)) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// When strobe last used the entry, from its stamp.
pub fn last_used(entry: &Path) -> Option<SystemTime> {
    std::fs::metadata(stamp(entry)).and_then(|m| m.modified()).ok()
}

/// An entry in use by this process: dropping it marks the entry used then and lets it go.
pub struct Lease {
    file: File,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lease")
    }
}

/// The leases one scope keeps, one per entry.
type Kept = Vec<(PathBuf, std::sync::Arc<Lease>)>;

thread_local! {
    /// Leases kept until the innermost `Scope` on this thread ends.
    static SCOPES: std::cell::RefCell<Vec<Kept>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// While alive, `keep` holds leases on this thread until it drops: an experiment's seed copies and GPU
/// cache stores stay its own between blocks, when no launch holds them.
pub struct Scope(());

pub fn scope() -> Scope {
    SCOPES.with(|s| s.borrow_mut().push(Vec::new()));
    Scope(())
}

impl Drop for Scope {
    fn drop(&mut self) {
        let frame = SCOPES.with(|s| s.borrow_mut().pop());
        drop(frame);
    }
}

/// Keeps a lease on `entry` until the innermost scope on this thread ends (one per entry); without a
/// scope the caller's own lease is all there is.
pub fn keep(entry: &Path, lease: &std::sync::Arc<Lease>) {
    SCOPES.with(|s| {
        if let Some(frame) = s.borrow_mut().last_mut()
            && !frame.iter().any(|(p, _)| p == entry)
        {
            frame.push((entry.to_path_buf(), lease.clone()));
        }
    });
}

/// Lets go of the lease the scopes on this thread keep on `entry` (before the entry is removed).
pub fn release(entry: &Path) {
    let gone: Vec<_> = SCOPES.with(|s| s.borrow_mut().iter_mut().flat_map(|frame| frame.extract_if(.., |(p, _)| p == entry).collect::<Vec<_>>()).collect());
    drop(gone);
}

/// Removes an entry this process owns (an experiment's GPU cache store at its end) with its stamp,
/// under the stamp's exclusive hold; left alone while another process uses it.
pub fn remove_own(entry: &Path) {
    release(entry);
    if let Some(h) = hold(entry) {
        let _ = std::fs::remove_dir_all(entry);
        if entry.exists() { h.kept() } else { h.removed() }
    }
}

/// Uses the entry until the lease drops; waits while a prune removes it (the caller makes it again).
/// An entry it cannot lease (no stamp to lock, a filesystem that cannot tell its files apart) is an
/// error, never a lease that holds nothing.
pub fn take(entry: &Path) -> std::io::Result<Lease> {
    let path = stamp(entry);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    for _ in 0..20 {
        let f = File::options().create(true).truncate(false).write(true).open(&path)?;
        f.lock_shared().map_err(|e| std::io::Error::new(e.kind(), format!("lock {}: {e}", path.display())))?;
        // A prune that held the lock removed the stamp with the entry: this lock is on a file no
        // longer at the path, which the next prune would not see. Take the one there now.
        match super::disk::same_file(&f, &path) {
            Some(true) => {
                let _ = f.set_modified(SystemTime::now());
                return Ok(Lease { file: f });
            }
            Some(false) => continue,
            None if path.exists() => return Err(std::io::Error::other(format!("{}: cannot tell whether the locked stamp is the one there", path.display()))),
            None => continue,
        }
    }
    Err(std::io::Error::other(format!("{}: removed again and again while it was being leased", path.display())))
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.file.set_modified(SystemTime::now());
        let _ = self.file.unlock();
    }
}

/// Whether a live process uses the entry now (holds its stamp's lock). Read-only: a missing stamp
/// is not made (no process uses an entry without one).
pub fn in_use(entry: &Path) -> bool {
    let Ok(file) = File::options().write(true).open(stamp(entry)) else { return false };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(_) => false,
    }
}

/// The exclusive hold of an entry `strobe prune` removes under.
pub struct Hold {
    /// None where the entry's folder is gone already: nothing can use it.
    file: Option<File>,
    path: PathBuf,
    /// The stamp's time before the hold made or opened it: put back when the entry stays.
    was: Option<SystemTime>,
}

/// The entry's exclusive hold, or None while a live process uses it.
pub fn hold(entry: &Path) -> Option<Hold> {
    let path = stamp(entry);
    let was = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if !path.parent().is_some_and(Path::is_dir) {
        return Some(Hold { file: None, path, was });
    }
    let file = File::options().create(true).truncate(false).write(true).open(&path).ok()?;
    match file.try_lock() {
        Ok(()) => Some(Hold { file: Some(file), path, was }),
        Err(_) => None,
    }
}

impl Hold {
    /// After the entry is gone: its stamp goes too, still held, so no use takes the old stamp's lock
    /// for a new entry (`take` checks the file it locked is the one at the path).
    pub fn removed(self) {
        if self.file.is_some() {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// The entry stays (part of it could not be removed): its stamp keeps the time it had, or goes
    /// when the hold made it.
    pub fn kept(self) {
        match (&self.file, self.was) {
            (Some(f), Some(t)) => {
                let _ = f.set_modified(t);
            }
            (Some(_), None) => {
                let _ = std::fs::remove_file(&self.path);
            }
            _ => {}
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(f) = &self.file {
            let _ = f.unlock();
        }
    }
}
