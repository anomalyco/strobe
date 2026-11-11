//! Disk use of strobe's own folders (SPEC §5.4, §5.14): what each file takes on the disk (its
//! allocation, as the filesystem reports it), each file once however many hard links it has, never
//! through a link out of a folder; and removal that never follows one and counts what it freed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
    /// A symbolic link, a junction or a mount point: counted as nothing, never followed.
    Link,
}

/// One entry of a folder, from its listing.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: std::ffi::OsString,
    pub kind: Kind,
    /// Bytes the filesystem allocated for it.
    pub bytes: u64,
    /// Its length.
    pub len: u64,
    /// The file's identity on its volume (an inode, an NTFS file id): one per file, whatever its names.
    pub id: u64,
    /// Its volume (Unix: the device): a walk never leaves the volume it started on.
    pub dev: u64,
    /// How many names the file has (hard links); 0 until known (Windows asks only when it matters).
    pub links: u32,
    pub modified: Option<SystemTime>,
}

/// The entries of `dir`, `.` and `..` left out.
pub fn list(dir: &Path) -> std::io::Result<Vec<Item>> {
    os::list(dir).map(|(_, items)| items)
}

/// `list`, with what the folder itself takes (its index): NTFS keeps a folder's allocation out of its
/// parent's listing (0 there), which left out 3% of a build worktree's 3 GB, its node_modules folders.
fn list_dir(dir: &Path) -> std::io::Result<(u64, Vec<Item>)> {
    os::list(dir)
}

/// `path` itself, a link not followed.
pub fn stat(path: &Path) -> std::io::Result<Item> {
    os::stat(path)
}

/// Windows lists no file's names: a walk asks each file of at least this size (a query by name, about
/// 40 µs each: asking all 2.1 M files of a 116 GB state folder made its walk 104 s instead of 14 s)
/// and counts a smaller one as having one name. Measured there: the walk's total moved by 0.02% and
/// its reclaimable bytes by 0.03% against asking every file (SPEC §5.14).
const ASKED: u64 = 1 << 20;

/// How many names `item` (in `dir`) has: Unix's listing says; Windows asks the file.
fn links(dir: &Path, item: &mut Item) -> u32 {
    if item.links == 0 {
        item.links = os::links(&dir.join(&item.name)).unwrap_or(1);
    }
    item.links
}

/// Bytes free on the volume `path` is on, for this user.
pub fn free(path: &Path) -> Option<u64> {
    os::free(path)
}

/// The identity of an open file (`Item::id`).
pub fn file_id(f: &std::fs::File) -> Option<u64> {
    os::file_id(f)
}

/// What one walk found under a path.
#[derive(Clone, Copy, Default, Debug, serde::Serialize)]
pub struct Sized {
    /// Allocated bytes of the files counted here: each file once, where a walk met it first.
    pub bytes: u64,
    pub files: u64,
    /// The newest modification time of the path and anything in it.
    #[serde(skip)]
    pub newest: Option<SystemTime>,
}

/// A file with more than one name, met by a walk: its size, how many names it has, and the owner
/// (an index the caller picks) of each name the walk met.
struct Shared {
    bytes: u64,
    links: u32,
    owners: Vec<usize>,
}

/// Paths a walk leaves out, down to `depth` levels below where it started.
struct Skip<'a> {
    except: &'a std::collections::HashSet<PathBuf>,
    depth: usize,
}

/// Sizes of several paths, each file once: a hard link met again is not counted again, and a file
/// with names the walk never met (outside every walked path) is not freed by removing these.
#[derive(Default)]
pub struct Tally {
    shared: HashMap<(u64, u64), Shared>,
    /// Allocated bytes of the files with one name, per owner: removing that owner frees them.
    alone: HashMap<usize, u64>,
}

impl Tally {
    /// Walks `path` for `owner`, on the volume `path` is on (a filesystem mounted inside is left out).
    pub fn walk(&mut self, path: &Path, owner: usize) -> Sized {
        self.walk_except(path, owner, &std::collections::HashSet::new(), 0)
    }

    /// `walk`, leaving out the paths in `except` (walked for their own owners) down to `depth` levels
    /// below `path`.
    pub fn walk_except(&mut self, path: &Path, owner: usize, except: &std::collections::HashSet<PathBuf>, depth: usize) -> Sized {
        let mut out = Sized::default();
        if except.contains(path) {
            return out;
        }
        let Ok(mut top) = stat(path) else { return out };
        let parent = path.parent().unwrap_or(Path::new(""));
        top.name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        let dev = top.dev;
        let skip = Skip { except, depth };
        self.item(parent, top, owner, dev, &skip, &mut out);
        out
    }

    fn item(&mut self, dir: &Path, mut it: Item, owner: usize, dev: u64, skip: &Skip, out: &mut Sized) {
        if it.dev != dev {
            return;
        }
        if it.modified > out.newest {
            out.newest = it.modified;
        }
        match it.kind {
            Kind::Link => {}
            Kind::File => {
                out.files += 1;
                // A file with nothing allocated (empty, or kept inside the volume's own records) frees
                // nothing: asking how many names it has would only cost.
                if it.bytes == 0 {
                    return;
                }
                if cfg!(windows) && it.bytes < ASKED {
                    it.links = 1;
                }
                if links(dir, &mut it) <= 1 {
                    out.bytes += it.bytes;
                    *self.alone.entry(owner).or_default() += it.bytes;
                    return;
                }
                let s = self.shared.entry((it.dev, it.id)).or_insert_with(|| Shared { bytes: it.bytes, links: it.links, owners: Vec::new() });
                if s.owners.is_empty() {
                    out.bytes += it.bytes;
                }
                s.owners.push(owner);
            }
            Kind::Dir => {
                let path = dir.join(&it.name);
                let (own, children) = list_dir(&path).unwrap_or_default();
                // A folder's own blocks: its lstat on Unix, its index on NTFS.
                let own = own.max(it.bytes);
                out.bytes += own;
                *self.alone.entry(owner).or_default() += own;
                let inner = Skip { except: skip.except, depth: skip.depth.saturating_sub(1) };
                for child in children {
                    if skip.depth > 0 && skip.except.contains(&path.join(&child.name)) {
                        continue;
                    }
                    self.item(&path, child, owner, dev, &inner, out);
                }
            }
        }
    }

    /// What a walk found for `owner`, to keep and `restore` later: bytes of its files with one name,
    /// and each file with several it met (volume, id, bytes, names, how many of them it met).
    pub fn kept_of(&self, owner: usize) -> (u64, Vec<[u64; 5]>) {
        let shared = self
            .shared
            .iter()
            .filter_map(|((dev, id), s)| {
                let n = s.owners.iter().filter(|o| **o == owner).count() as u64;
                (n > 0).then_some([*dev, *id, s.bytes, s.links as u64, n])
            })
            .collect();
        (self.alone.get(&owner).copied().unwrap_or(0), shared)
    }

    /// Counts for `owner` what an earlier walk found (`kept_of`) without walking again: its bytes are
    /// those files no owner before it holds.
    pub fn restore(&mut self, owner: usize, alone: u64, shared: &[[u64; 5]]) -> u64 {
        let mut bytes = alone;
        *self.alone.entry(owner).or_default() += alone;
        for [dev, id, b, links, n] in shared.iter().copied() {
            let s = self.shared.entry((dev, id)).or_insert_with(|| Shared { bytes: b, links: links as u32, owners: Vec::new() });
            if s.owners.is_empty() {
                bytes += b;
            }
            s.owners.extend(std::iter::repeat_n(owner, n as usize));
        }
        bytes
    }

    /// Bytes freed by removing every owner `gone` says: their files with one name, and each file with
    /// several whose every name lies in them, put on the first of them that met it.
    pub fn freed(&self, gone: impl Fn(usize) -> bool) -> HashMap<usize, u64> {
        let mut out: HashMap<usize, u64> = self.alone.iter().filter(|(o, _)| gone(**o)).map(|(o, b)| (*o, *b)).collect();
        for s in self.shared.values() {
            if s.owners.len() as u32 >= s.links && s.owners.iter().all(|o| gone(*o)) {
                *out.entry(s.owners[0]).or_default() += s.bytes;
            }
        }
        out
    }
}

/// What a removal did: bytes freed (files whose last name it removed), files removed, and what it
/// could not remove.
#[derive(Default, Debug)]
pub struct Removed {
    pub freed: u64,
    pub files: u64,
    pub failed: Vec<String>,
}

/// Whether `path` lies inside `root`, both made absolute and plain (`..` resolved, the folder above
/// `path` canonicalized; `path` itself is not followed).
pub fn inside(path: &Path, root: &Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return false };
    if name == ".." || name == "." {
        return false;
    }
    let (Ok(parent), Ok(root)) = (std::fs::canonicalize(parent), std::fs::canonicalize(root)) else { return false };
    parent.starts_with(&root)
}

/// Removes `path`, which must lie inside `root`: a link (symbolic link, junction) is removed itself,
/// never what it points at; a folder is emptied file by file, so one file that cannot go (held open,
/// denied) is named and the rest still goes. Read-only files and folders are made writable first.
pub fn remove(path: &Path, root: &Path) -> Removed {
    let mut out = Removed::default();
    if !inside(path, root) {
        out.failed.push(format!("{}: not inside {}", path.display(), root.display()));
        return out;
    }
    match stat(path) {
        Ok(mut it) => {
            let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
            it.name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
            remove_item(&parent, it, &mut out);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => out.failed.push(format!("{}: {e}", path.display())),
    }
    out
}

fn remove_item(dir: &Path, mut it: Item, out: &mut Removed) {
    let path = dir.join(&it.name);
    match it.kind {
        Kind::File => {
            // The last name of a file frees its bytes; another name keeps them (a link elsewhere, or one
            // this removal reaches later, which then is the last).
            let last = it.bytes == 0 || links(dir, &mut it) <= 1;
            match retry(|| std::fs::remove_file(&path), &path) {
                Ok(()) => {
                    out.files += 1;
                    if last {
                        out.freed += it.bytes;
                    }
                }
                Err(e) => out.failed.push(format!("{}: {e}", path.display())),
            }
        }
        Kind::Link => {
            // A junction or a link to a folder is removed as a folder on Windows; neither follows it.
            let r = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path));
            if let Err(e) = r {
                out.failed.push(format!("{}: {e}", path.display()));
            }
        }
        Kind::Dir => {
            writable(&path);
            let before = out.failed.len();
            let mut own = it.bytes;
            match list_dir(&path) {
                Ok((bytes, children)) => {
                    own = own.max(bytes);
                    for c in children {
                        remove_item(&path, c, out);
                    }
                }
                Err(e) => out.failed.push(format!("{}: {e}", path.display())),
            }
            // A folder that kept something it holds stays; what it holds was named.
            if before == out.failed.len() {
                match retry(|| std::fs::remove_dir(&path), &path) {
                    Ok(()) => out.freed += own,
                    Err(e) => out.failed.push(format!("{}: {e}", path.display())),
                }
            }
        }
    }
}

/// Runs a removal, again after making `path` writable (a read-only file or folder), and a few more
/// times over half a second while Windows still holds a name another process just closed (a scanner).
fn retry(mut f: impl FnMut() -> std::io::Result<()>, path: &Path) -> std::io::Result<()> {
    let mut last = None;
    for i in 0..10 {
        match f() {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                if i == 0 && e.kind() == std::io::ErrorKind::PermissionDenied {
                    writable(path);
                } else if !cfg!(windows) {
                    return Err(e);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("could not remove")))
}

/// Clears a read-only mark (Windows) or gives the owner write permission (Unix, a folder strobe made
/// read-only); never through a link.
fn writable(path: &Path) {
    let Ok(m) = std::fs::symlink_metadata(path) else { return };
    if m.file_type().is_symlink() {
        return;
    }
    let mut p = m.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if p.mode() & 0o200 == 0 {
            p.set_mode(p.mode() | 0o700);
            let _ = std::fs::set_permissions(path, p);
        }
    }
    #[cfg(not(unix))]
    if p.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = std::fs::set_permissions(path, p);
    }
}

#[cfg(unix)]
mod os {
    use super::{Item, Kind};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn item(name: std::ffi::OsString, m: &std::fs::Metadata) -> Item {
        let t = m.file_type();
        let kind = if t.is_symlink() {
            Kind::Link
        } else if t.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        Item { name, kind, bytes: m.blocks() * 512, len: m.len(), id: m.ino(), dev: m.dev(), links: m.nlink() as u32, modified: m.modified().ok() }
    }

    /// A folder's own blocks are its lstat's, in its parent's listing: 0 here.
    pub fn list(dir: &Path) -> std::io::Result<(u64, Vec<Item>)> {
        Ok((0, std::fs::read_dir(dir)?.flatten().filter_map(|e| e.metadata().ok().map(|m| item(e.file_name(), &m))).collect()))
    }

    pub fn stat(path: &Path) -> std::io::Result<Item> {
        Ok(item(path.file_name().map(|n| n.to_os_string()).unwrap_or_default(), &std::fs::symlink_metadata(path)?))
    }

    pub fn links(path: &Path) -> Option<u32> {
        std::fs::symlink_metadata(path).ok().map(|m| m.nlink() as u32)
    }

    pub fn file_id(f: &std::fs::File) -> Option<u64> {
        f.metadata().ok().map(|m| m.ino())
    }

    pub fn free(path: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and a statvfs the call fills.
        (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
    }
}

#[cfg(windows)]
mod os {
    use super::{Item, Kind};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileIdBothDirectoryInfo, FileStandardInfo, GetDiskFreeSpaceExW, GetFileInformationByHandleEx,
        OPEN_EXISTING,
    };

    /// The two reparse tags that name another place: a symbolic link and a junction (mount point).
    /// Other reparse points (deduplicated or cloud files) are the files themselves.
    fn surrogate(tag: u32) -> bool {
        tag & 0x2000_0000 != 0
    }

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// A path Windows takes whatever its length: `\\?\` before an absolute path that has no prefix.
    fn long(p: &Path) -> std::path::PathBuf {
        let s = p.as_os_str().to_string_lossy();
        if s.starts_with(r"\\?\") || !p.is_absolute() {
            return p.to_path_buf();
        }
        let s = s.replace('/', r"\");
        if let Some(unc) = s.strip_prefix(r"\\") { format!(r"\\?\UNC\{unc}").into() } else { format!(r"\\?\{s}").into() }
    }

    fn time(t: i64) -> Option<std::time::SystemTime> {
        // 100 ns ticks since 1601-01-01.
        let unix = t.checked_sub(116_444_736_000_000_000)?;
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(u64::try_from(unix).ok()? * 100))
    }

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle this module opened.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn open(p: &Path, access: u32) -> std::io::Result<Handle> {
        let w = wide(&long(p));
        // SAFETY: a NUL-terminated path; the handle is closed by `Handle`.
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE { Err(std::io::Error::last_os_error()) } else { Ok(Handle(h)) }
    }

    /// The volume a path is on: its drive or share, from the path itself.
    fn volume(p: &Path) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(std::path::Component::Prefix(x)) = p.components().next() {
            x.as_os_str().to_string_lossy().to_lowercase().trim_start_matches(r"\\?\").hash(&mut h);
        }
        h.finish()
    }

    fn kind(attributes: u32, tag: u32) -> Kind {
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && surrogate(tag) {
            Kind::Link
        } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            Kind::Dir
        } else {
            Kind::File
        }
    }

    /// One read of the folder's listing per 64 KB, each entry with its size, allocation, attributes,
    /// reparse tag and file id: no file is opened. And the folder's own allocation (its index), which
    /// its parent's listing gives as 0.
    pub fn list(dir: &Path) -> std::io::Result<(u64, Vec<Item>)> {
        let h = open(dir, FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)?;
        let dev = volume(&long(dir));
        let mut standard: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: an open handle and a structure the call fills.
        let own = if unsafe { GetFileInformationByHandleEx(h.0, FileStandardInfo, (&mut standard as *mut FILE_STANDARD_INFO).cast(), std::mem::size_of::<FILE_STANDARD_INFO>() as u32) } != 0 {
            standard.AllocationSize.max(0) as u64
        } else {
            0
        };
        let mut buf = vec![0u64; 8192];
        let mut out = Vec::new();
        loop {
            // SAFETY: the buffer is 64 KB, 8-byte aligned, as the call requires.
            let ok = unsafe { GetFileInformationByHandleEx(h.0, FileIdBothDirectoryInfo, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32) };
            if ok == 0 {
                let e = std::io::Error::last_os_error();
                // ERROR_NO_MORE_FILES.
                if e.raw_os_error() == Some(18) {
                    return Ok((own, out));
                }
                return Err(e);
            }
            let base = buf.as_ptr().cast::<u8>();
            let mut at = 0usize;
            loop {
                // SAFETY: entries lie inside the buffer the call filled, each at the offset the one before names.
                let e: FILE_ID_BOTH_DIR_INFO = unsafe { std::ptr::read_unaligned(base.add(at).cast()) };
                let name_at = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                let n = e.FileNameLength as usize / 2;
                // SAFETY: the name follows the fixed part, FileNameLength bytes long.
                let name: Vec<u16> = (0..n).map(|i| unsafe { std::ptr::read_unaligned(base.add(at + name_at + i * 2).cast::<u16>()) }).collect();
                let name = std::ffi::OsString::from_wide(&name);
                if name != "." && name != ".." {
                    // A reparse point's tag is where the extended-attribute size would be.
                    let tag = if e.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { e.EaSize } else { 0 };
                    out.push(Item {
                        name,
                        kind: kind(e.FileAttributes, tag),
                        bytes: e.AllocationSize.max(0) as u64,
                        len: e.EndOfFile.max(0) as u64,
                        id: e.FileId as u64,
                        dev,
                        links: 0,
                        modified: time(e.LastWriteTime),
                    });
                }
                if e.NextEntryOffset == 0 {
                    break;
                }
                at += e.NextEntryOffset as usize;
            }
        }
    }

    /// The file's stat information by its name, without opening it (Windows 10 1709 and later).
    fn by_name(p: &Path) -> Option<windows_sys::Wdk::Storage::FileSystem::FILE_STAT_INFORMATION> {
        use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
        use windows_sys::Wdk::Storage::FileSystem::{FILE_STAT_INFORMATION, FileStatInformation, NtQueryInformationByName};
        let s = long(p).as_os_str().to_string_lossy().to_string();
        // `\\?\C:\x` → `\??\C:\x`, the object manager's name for it.
        let nt: Vec<u16> = format!(r"\??\{}", s.strip_prefix(r"\\?\")?).encode_utf16().collect();
        let name = UNICODE_STRING { Length: (nt.len() * 2) as u16, MaximumLength: (nt.len() * 2) as u16, Buffer: nt.as_ptr() as *mut u16 };
        // OBJ_CASE_INSENSITIVE.
        let attrs = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: std::ptr::null_mut(),
            ObjectName: &name,
            Attributes: 0x40,
            SecurityDescriptor: std::ptr::null(),
            SecurityQualityOfService: std::ptr::null(),
        };
        let mut io: windows_sys::Win32::System::IO::IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        let mut info: FILE_STAT_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: attributes and buffers that outlive the call; the size is the structure's.
        let status = unsafe { NtQueryInformationByName(&attrs, &mut io, (&mut info as *mut FILE_STAT_INFORMATION).cast(), std::mem::size_of::<FILE_STAT_INFORMATION>() as u32, FileStatInformation) };
        (status >= 0).then_some(info)
    }

    pub fn stat(path: &Path) -> std::io::Result<Item> {
        let name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        if let Some(s) = by_name(path) {
            return Ok(Item {
                name,
                kind: kind(s.FileAttributes, s.ReparseTag),
                bytes: s.AllocationSize.max(0) as u64,
                len: s.EndOfFile.max(0) as u64,
                id: s.FileId as u64,
                dev: volume(&long(path)),
                links: s.NumberOfLinks,
                modified: time(s.LastWriteTime),
            });
        }
        // No query by name (an older Windows, another filesystem): the handle's own information.
        let h = open(path, 0)?;
        let info = info(h.0).ok_or_else(std::io::Error::last_os_error)?;
        let len = (info.nFileSizeHigh as u64) << 32 | info.nFileSizeLow as u64;
        let m = std::fs::symlink_metadata(path)?;
        let kind = if m.file_type().is_symlink() {
            Kind::Link
        } else if m.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        Ok(Item {
            name,
            kind,
            bytes: len.div_ceil(4096) * 4096,
            len,
            id: (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64,
            dev: volume(&long(path)),
            links: info.nNumberOfLinks,
            modified: m.modified().ok(),
        })
    }

    pub fn links(path: &Path) -> Option<u32> {
        if let Some(s) = by_name(path) {
            return Some(s.NumberOfLinks);
        }
        let h = open(path, 0).ok()?;
        info(h.0).map(|i| i.nNumberOfLinks)
    }

    fn info(h: HANDLE) -> Option<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
        let mut info: windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: an open handle and a structure the call fills.
        (unsafe { windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle(h, &mut info) } != 0).then_some(info)
    }

    pub fn file_id(f: &std::fs::File) -> Option<u64> {
        use std::os::windows::io::AsRawHandle;
        info(f.as_raw_handle()).map(|i| (i.nFileIndexHigh as u64) << 32 | i.nFileIndexLow as u64)
    }

    pub fn free(path: &Path) -> Option<u64> {
        let w = wide(&long(path));
        let mut free = 0u64;
        // SAFETY: a NUL-terminated path and an output the call fills.
        (unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) } != 0).then_some(free)
    }
}
