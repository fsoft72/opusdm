//! Read, restore and erase what is in the freedesktop.org trash: the home
//! trash and the trash folders at the top of every mounted disk (see
//! [`crate::trash`] for how entries get there).

use crate::delete::remove_recursive;
use crate::trash::home_trash;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Suffix of the files in `info/`.
const INFO_SUFFIX: &str = ".trashinfo";
/// The group header every `.trashinfo` file starts with.
const INFO_GROUP: &str = "[Trash Info]";
/// Cache file some file managers keep next to `files/` and `info/`.
const DIRECTORY_SIZES: &str = "directorysizes";
/// Filesystem types that never hold a trash and are not worth a `stat`.
const PSEUDO_FS: &[&str] = &[
    "proc", "sysfs", "cgroup", "cgroup2", "devpts", "devtmpfs", "mqueue", "debugfs", "tracefs",
    "securityfs", "pstore", "bpf", "configfs", "fusectl", "hugetlbfs", "binfmt_misc", "autofs",
    "efivarfs", "nsfs", "ramfs", "rpc_pipefs",
];

/// One entry of a trash folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashItem {
    /// The trash folder holding it (the one with `files/` and `info/`).
    pub trash: PathBuf,
    /// Its name inside `files/` (also the stem of its `.trashinfo`).
    pub name: String,
    /// Where it was before it was trashed.
    pub original: PathBuf,
    /// When it was trashed, `YYYY-MM-DDThh:mm:ss` as written in the info file.
    pub deleted: String,
    /// Whether it is a folder.
    pub is_dir: bool,
}

impl TrashItem {
    /// The trashed file or folder itself.
    pub fn file(&self) -> PathBuf {
        self.trash.join("files").join(&self.name)
    }

    /// Its `.trashinfo` file.
    pub fn info(&self) -> PathBuf {
        self.trash.join("info").join(format!("{}{INFO_SUFFIX}", self.name))
    }
}

/// Every existing trash folder of this user: the home trash, then the
/// `.Trash/$uid` and `.Trash-$uid` folders at the top of the mounted disks.
pub fn trash_dirs() -> Vec<PathBuf> {
    // SAFETY: getuid() has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut out: Vec<PathBuf> = home_trash().into_iter().filter(|t| t.is_dir()).collect();
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    for top in mount_tops(&mounts) {
        for trash in [top.join(".Trash").join(uid.to_string()), top.join(format!(".Trash-{uid}"))] {
            if trash.is_dir() && !out.contains(&trash) {
                out.push(trash);
            }
        }
    }
    out
}

/// Every item of every trash folder, the most recently deleted first.
pub fn list_all() -> Vec<TrashItem> {
    let mut items: Vec<TrashItem> = trash_dirs().iter().flat_map(|t| list_items(t)).collect();
    items.sort_by(|a, b| b.deleted.cmp(&a.deleted).then_with(|| a.name.cmp(&b.name)));
    items
}

/// The items of the trash folder `trash`. An info file without its trashed
/// entry, or one that cannot be read, is left out.
pub fn list_items(trash: &Path) -> Vec<TrashItem> {
    let Ok(entries) = fs::read_dir(trash.join("info")) else { return Vec::new() };
    let top = trash_top(trash);
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file_name.strip_suffix(INFO_SUFFIX) else { continue };
        let Ok(meta) = fs::symlink_metadata(trash.join("files").join(name)) else { continue };
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        let Some((path, deleted)) = parse_trash_info(&text) else { continue };
        let Some(original) = percent_decode(&path) else { continue };
        items.push(TrashItem {
            trash: trash.to_path_buf(),
            name: name.to_string(),
            original: top.join(original),
            deleted,
            is_dir: meta.is_dir(),
        });
    }
    items
}

/// Put `item` back where it was (its folder is created when missing) and
/// return that path. An existing entry there is never overwritten.
pub fn restore(item: &TrashItem) -> Result<PathBuf, String> {
    let target = &item.original;
    if fs::symlink_metadata(target).is_ok() {
        return Err(format!("{} already exists", target.display()));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::rename(item.file(), target).map_err(|e| format!("cannot restore {}: {e}", target.display()))?;
    remove_info(item).map_err(|e| format!("restored, but its trash info is left: {e}"))?;
    Ok(target.clone())
}

/// Delete `item` for good, with its info file.
pub fn erase(item: &TrashItem) -> Result<(), String> {
    let errors = remove_recursive(&item.file());
    if let Some((path, msg)) = errors.first() {
        return Err(format!("{}: {msg} ({} error(s))", path.display(), errors.len()));
    }
    remove_info(item).map_err(|e| format!("cannot remove its trash info: {e}"))
}

/// Delete everything in the trash folder `trash` (also entries without an
/// info file). Failures are collected.
pub fn empty(trash: &Path) -> Vec<(PathBuf, String)> {
    let mut errors = Vec::new();
    for sub in ["files", "info"] {
        let dir = trash.join(sub);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                errors.push((dir, e.to_string()));
                continue;
            }
        };
        for entry in entries.flatten() {
            errors.extend(remove_recursive(&entry.path()));
        }
    }
    let sizes = trash.join(DIRECTORY_SIZES);
    if let Err(e) = fs::remove_file(&sizes) {
        if e.kind() != io::ErrorKind::NotFound {
            errors.push((sizes, e.to_string()));
        }
    }
    errors
}

/// Remove the info file of `item`; one already gone is not an error.
fn remove_info(item: &TrashItem) -> io::Result<()> {
    match fs::remove_file(item.info()) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The folder a relative `Path` of `trash` starts from: the top of the disk
/// for `$top/.Trash-$uid` and `$top/.Trash/$uid`, else `/` (the home trash
/// stores absolute paths).
fn trash_top(trash: &Path) -> PathBuf {
    let name = trash.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = trash.parent();
    if name.starts_with(".Trash-") {
        return parent.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
    }
    match parent {
        Some(p) if p.file_name() == Some(OsStr::new(".Trash")) => {
            p.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"))
        }
        _ => PathBuf::from("/"),
    }
}

/// The mount points listed in `mounts` (the text of `/proc/self/mounts`),
/// leaving out pseudo filesystems.
fn mount_tops(mounts: &str) -> Vec<PathBuf> {
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (_, dir, fs_type) = (fields.next()?, fields.next()?, fields.next()?);
            (!PSEUDO_FS.contains(&fs_type)).then(|| PathBuf::from(unescape_mount(dir)))
        })
        .collect()
}

/// A mount point with its octal escapes (`\040` for a space) decoded.
fn unescape_mount(field: &str) -> std::ffi::OsString {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes.get(i + 1..i + 4).and_then(|d| std::str::from_utf8(d).ok()).and_then(|d| u8::from_str_radix(d, 8).ok());
        match (bytes[i], octal) {
            (b'\\', Some(b)) => {
                out.push(b);
                i += 4;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    OsStr::from_bytes(&out).to_os_string()
}

/// The still encoded `Path` and the `DeletionDate` of a `.trashinfo` text.
fn parse_trash_info(text: &str) -> Option<(String, String)> {
    let mut in_group = false;
    let (mut path, mut date) = (None, String::new());
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == INFO_GROUP;
            continue;
        }
        if !in_group {
            continue;
        }
        match line.split_once('=') {
            Some(("Path", v)) => path = Some(v.to_string()),
            Some(("DeletionDate", v)) => date = v.to_string(),
            _ => {}
        }
    }
    Some((path?, date))
}

/// `text` with its `%XX` escapes decoded; `None` for a broken escape.
fn percent_decode(text: &str) -> Option<PathBuf> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(OsStr::from_bytes(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trash::trash_one;

    /// A temp folder with `name` written in it and trashed to `<tmp>/Trash`.
    fn trashed(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let trash = td.path().join("Trash");
        let file = td.path().join(name);
        fs::write(&file, name).unwrap();
        let file = fs::canonicalize(&file).unwrap();
        trash_one(&file, Some(&trash)).unwrap();
        (td, trash, file)
    }

    #[test]
    fn a_trashed_file_is_listed_with_its_origin() {
        let (_td, trash, file) = trashed("a b.txt");
        let items = list_items(&trash);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "a b.txt");
        assert_eq!(items[0].original, file);
        assert_eq!(items[0].deleted.len(), 19);
        assert!(!items[0].is_dir);
    }

    #[test]
    fn restore_puts_it_back_and_drops_the_info() {
        let (_td, trash, file) = trashed("note.txt");
        let item = list_items(&trash).remove(0);
        assert_eq!(restore(&item).unwrap(), file);
        assert_eq!(fs::read_to_string(&file).unwrap(), "note.txt");
        assert!(!item.info().exists());
        assert!(list_items(&trash).is_empty());
    }

    #[test]
    fn restore_never_overwrites() {
        let (_td, trash, file) = trashed("note.txt");
        fs::write(&file, "new").unwrap();
        let item = list_items(&trash).remove(0);
        assert!(restore(&item).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        assert!(item.file().exists());
    }

    #[test]
    fn restore_recreates_a_missing_folder() {
        let td = tempfile::tempdir().unwrap();
        let trash = td.path().join("Trash");
        let dir = fs::canonicalize(td.path()).unwrap().join("gone");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("f"), "x").unwrap();
        trash_one(&dir.join("f"), Some(&trash)).unwrap();
        fs::remove_dir(&dir).unwrap();

        restore(&list_items(&trash).remove(0)).unwrap();
        assert!(dir.join("f").exists());
    }

    #[test]
    fn erase_removes_file_and_info() {
        let (_td, trash, _) = trashed("x");
        let item = list_items(&trash).remove(0);
        erase(&item).unwrap();
        assert!(!item.file().exists());
        assert!(!item.info().exists());
    }

    #[test]
    fn empty_removes_everything() {
        let (_td, trash, _) = trashed("x");
        fs::write(trash.join("files/orphan"), "o").unwrap();
        assert!(empty(&trash).is_empty());
        assert_eq!(fs::read_dir(trash.join("files")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(trash.join("info")).unwrap().count(), 0);
    }

    #[test]
    fn info_without_its_file_is_not_listed() {
        let (_td, trash, _) = trashed("x");
        fs::remove_file(trash.join("files/x")).unwrap();
        assert!(list_items(&trash).is_empty());
    }

    #[test]
    fn parse_reads_only_the_trash_group() {
        let text = "[Other]\nPath=/no\n[Trash Info]\nPath=/a%20b\nDeletionDate=2026-10-14T10:00:00\n";
        assert_eq!(parse_trash_info(text), Some(("/a%20b".into(), "2026-10-14T10:00:00".into())));
        assert_eq!(parse_trash_info("[Trash Info]\nDeletionDate=x\n"), None);
    }

    #[test]
    fn percent_decode_undoes_the_encoding() {
        assert_eq!(percent_decode("/a%20b/%C3%A8"), Some(PathBuf::from("/a b/è")));
        assert_eq!(percent_decode("/bad%2"), None);
    }

    #[test]
    fn relative_paths_start_at_the_top_of_the_disk() {
        assert_eq!(trash_top(Path::new("/mnt/usb/.Trash-1000")), PathBuf::from("/mnt/usb"));
        assert_eq!(trash_top(Path::new("/mnt/usb/.Trash/1000")), PathBuf::from("/mnt/usb"));
        assert_eq!(trash_top(Path::new("/home/me/.local/share/Trash")), PathBuf::from("/"));
    }

    #[test]
    fn mounts_skip_pseudo_filesystems_and_decode_spaces() {
        let mounts = "proc /proc proc rw 0 0\n/dev/sdb1 /media/My\\040Disk ext4 rw 0 0\n";
        assert_eq!(mount_tops(mounts), vec![PathBuf::from("/media/My Disk")]);
    }
}
