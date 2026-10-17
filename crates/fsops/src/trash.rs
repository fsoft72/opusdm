//! Move files to the freedesktop.org trash (Trash specification 1.0): the
//! home trash (`$XDG_DATA_HOME/Trash`) when the file lives on the same
//! device, else the trash at the top of the file's mount (`$top/.Trash/$uid`
//! when `$top/.Trash` is a sticky, non-symlink folder, else `$top/.Trash-$uid`).
//! Every trashed entry gets `info/<name>.trashinfo` with its original path
//! and the deletion date, so file managers can restore it.

use crate::cancel::CancelToken;
use crate::rename::rename_noreplace;
use crate::types::Progress;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// Mode of the trash folders the spec asks to create: owner only.
const TRASH_DIR_MODE: u32 = 0o700;
/// The sticky bit a shared `$top/.Trash` must carry to be trusted.
const STICKY_BIT: u32 = 0o1000;
/// Highest suffix tried when looking for a free name inside a trash.
const MAX_NAME_TRIES: u32 = 10_000;

/// Move every path in `srcs` to the trash. Failures (for example a device
/// without a usable trash) are collected, not fatal.
pub fn run_trash(srcs: &[PathBuf], tx: &Sender<Progress>, cancel: &CancelToken) -> Progress {
    let home = home_trash();
    let mut trashed = 0u64;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();

    for src in srcs {
        if cancel.is_cancelled() {
            return Progress::Done { copied: trashed, skipped: 0, cancelled: true, errors };
        }
        let _ = tx.send(Progress::File {
            path: src.clone(),
            done_bytes: 0,
            total_bytes: 0,
            files_done: trashed,
            files_total: srcs.len() as u64,
            file_bytes_done: 0,
            file_bytes_total: 0,
        });
        match trash_one(src, home.as_deref()) {
            Ok(_) => trashed += 1,
            Err(e) => errors.push((src.clone(), e)),
        }
    }

    Progress::Done { copied: trashed, skipped: 0, cancelled: false, errors }
}

/// The home trash folder: `$XDG_DATA_HOME/Trash`, or `~/.local/share/Trash`
/// when the variable is unset or empty. `None` without a home folder.
pub fn home_trash() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("Trash"))
}

/// Move `path` to the trash and return where it landed. `home_trash` is the
/// home trash folder (see [`home_trash`]); it is used when it sits on the
/// device of `path`, else the trash at the top of the mount is tried.
pub fn trash_one(path: &Path, home_trash: Option<&Path>) -> Result<PathBuf, String> {
    let path = real_path(path)?;
    let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    let dev = meta.dev();

    if let Some(home) = home_trash {
        if device_of(home) == Some(dev) {
            return move_into(&path, home, &path);
        }
    }

    let top = mount_top(&path, dev)?;
    let relative = path.strip_prefix(&top).map_err(|e| e.to_string())?.to_path_buf();
    // SAFETY: getuid() has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut last_error = String::from("no trash on this device");
    for trash in top_trashes(&top, uid) {
        if device_of(&trash) != Some(dev) {
            continue;
        }
        match move_into(&path, &trash, &relative) {
            Ok(dest) => return Ok(dest),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

/// `path` made absolute with its parent folder resolved (the entry itself is
/// kept as is, so a symlink is trashed, not its target).
fn real_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| format!("cannot resolve the path: {e}"))?;
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        return Err("cannot trash the root folder".into());
    };
    let parent = fs::canonicalize(parent).map_err(|e| e.to_string())?;
    Ok(parent.join(name))
}

/// The device of `path`, or of its nearest existing ancestor (the home trash
/// may not exist yet).
fn device_of(path: &Path) -> Option<u64> {
    path.ancestors().find_map(|p| fs::metadata(p).ok()).map(|m| m.dev())
}

/// The top folder of the mount holding `path` (a resolved path on device
/// `dev`): the highest ancestor still on that device.
fn mount_top(path: &Path, dev: u64) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("cannot trash the root folder")?;
    let mut top = parent.to_path_buf();
    for dir in parent.ancestors() {
        match fs::metadata(dir) {
            Ok(m) if m.dev() == dev => top = dir.to_path_buf(),
            _ => break,
        }
    }
    Ok(top)
}

/// The trash folders the spec allows at the top of a mount, in order:
/// `$top/.Trash/$uid` (only when `.Trash` is a real sticky folder), then
/// `$top/.Trash-$uid`.
fn top_trashes(top: &Path, uid: u32) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let shared = top.join(".Trash");
    if let Ok(m) = fs::symlink_metadata(&shared) {
        if m.is_dir() && m.mode() & STICKY_BIT != 0 {
            out.push(shared.join(uid.to_string()));
        }
    }
    out.push(top.join(format!(".Trash-{uid}")));
    out
}

/// Move `path` into `trash/files` under a free name and write its
/// `trash/info/<name>.trashinfo` recording `info_path` (absolute for the
/// home trash, relative to the mount top otherwise).
fn move_into(path: &Path, trash: &Path, info_path: &Path) -> Result<PathBuf, String> {
    let files = trash.join("files");
    let info = trash.join("info");
    for dir in [&files, &info] {
        DirBuilder::new()
            .recursive(true)
            .mode(TRASH_DIR_MODE)
            .create(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let name = path.file_name().ok_or("the path has no file name")?.to_string_lossy().into_owned();
    let content = trash_info(info_path, &deletion_date());

    for i in 1..=MAX_NAME_TRIES {
        let candidate = numbered_name(&name, i);
        let info_file = info.join(format!("{candidate}.trashinfo"));
        let dest = files.join(&candidate);
        // The info file is created first and exclusively: it reserves the name.
        let mut f = match OpenOptions::new().write(true).create_new(true).open(&info_file) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot write {}: {e}", info_file.display())),
        };
        // The rename itself refuses an entry already in `files/` (one left
        // without its info, or one that appeared just now): try the next name.
        let moved = f.write_all(content.as_bytes()).and_then(|_| f.sync_all()).and_then(|_| rename_noreplace(path, &dest));
        match moved {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&info_file);
            }
            Err(e) => {
                let _ = fs::remove_file(&info_file);
                return Err(format!("cannot move to the trash: {e}"));
            }
        }
    }
    Err(format!("no free name for {name:?} in the trash"))
}

/// `name` for the first try, then `stem.N.ext` (or `name.N` without an
/// extension) from the second one.
fn numbered_name(name: &str, i: u32) -> String {
    if i == 1 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}.{i}.{ext}"),
        _ => format!("{name}.{i}"),
    }
}

/// The `.trashinfo` text for `path` deleted at `date`.
fn trash_info(path: &Path, date: &str) -> String {
    format!("[Trash Info]\nPath={}\nDeletionDate={date}\n", percent_encode(path))
}

/// `path` with every byte other than unreserved URI characters and `/`
/// written as `%XX`, as the spec asks for the `Path` key.
fn percent_encode(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The current local time as `YYYY-MM-DDThh:mm:ss`.
fn deletion_date() -> String {
    // SAFETY: time() with a null pointer only returns the time; localtime_r
    // writes into the zeroed `tm` we own and returns null on failure.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return String::from("1970-01-01T00:00:00");
        }
        tm
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_goes_to_files_with_its_info() {
        let td = tempfile::tempdir().unwrap();
        let trash = td.path().join("Trash");
        let file = td.path().join("my file.txt");
        fs::write(&file, b"x").unwrap();
        let original = real_path(&file).unwrap();

        let dest = trash_one(&file, Some(&trash)).unwrap();

        assert!(!file.exists());
        assert_eq!(dest, trash.join("files/my file.txt"));
        assert_eq!(fs::read(&dest).unwrap(), b"x");
        let info = fs::read_to_string(trash.join("info/my file.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\n"), "{info}");
        assert!(info.contains(&format!("Path={}\n", percent_encode(&original))), "{info}");
        assert!(info.contains("DeletionDate="), "{info}");
    }

    #[test]
    fn a_second_file_with_the_same_name_gets_a_new_name() {
        let td = tempfile::tempdir().unwrap();
        let trash = td.path().join("Trash");
        for sub in ["a", "b"] {
            fs::create_dir(td.path().join(sub)).unwrap();
            fs::write(td.path().join(sub).join("note.txt"), sub).unwrap();
        }

        trash_one(&td.path().join("a/note.txt"), Some(&trash)).unwrap();
        let second = trash_one(&td.path().join("b/note.txt"), Some(&trash)).unwrap();

        assert_eq!(second, trash.join("files/note.2.txt"));
        assert_eq!(fs::read_to_string(&second).unwrap(), "b");
        assert!(trash.join("info/note.2.txt.trashinfo").exists());
    }

    /// An entry already in `files/` without its info file is never replaced:
    /// the trashed file takes the next free name.
    #[test]
    fn an_orphan_in_the_trash_is_never_replaced() {
        let td = tempfile::tempdir().unwrap();
        let trash = td.path().join("Trash");
        fs::create_dir_all(trash.join("files")).unwrap();
        fs::write(trash.join("files/note.txt"), b"orphan").unwrap();
        let file = td.path().join("note.txt");
        fs::write(&file, b"mine").unwrap();

        let dest = trash_one(&file, Some(&trash)).unwrap();

        assert_eq!(dest, trash.join("files/note.2.txt"));
        assert_eq!(fs::read(trash.join("files/note.txt")).unwrap(), b"orphan");
        assert!(!trash.join("info/note.txt.trashinfo").exists(), "the reserved info is dropped");
        assert!(trash.join("info/note.2.txt.trashinfo").exists());
    }

    #[test]
    fn a_folder_is_trashed_whole() {
        let td = tempfile::tempdir().unwrap();
        let trash = td.path().join("Trash");
        let dir = td.path().join("dir");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/f"), b"x").unwrap();

        trash_one(&dir, Some(&trash)).unwrap();

        assert!(!dir.exists());
        assert!(trash.join("files/dir/sub/f").exists());
    }

    #[test]
    fn a_missing_file_is_an_error_and_leaves_no_info() {
        let td = tempfile::tempdir().unwrap();
        let trash = td.path().join("Trash");
        assert!(trash_one(&td.path().join("nope"), Some(&trash)).is_err());
        assert!(!trash.join("info/nope.trashinfo").exists());
    }

    #[test]
    fn the_path_is_percent_encoded() {
        assert_eq!(percent_encode(Path::new("/home/me/a b%c/è.txt")), "/home/me/a%20b%25c/%C3%A8.txt");
    }

    #[test]
    fn numbered_names_keep_the_extension() {
        assert_eq!(numbered_name("a.txt", 1), "a.txt");
        assert_eq!(numbered_name("a.txt", 3), "a.3.txt");
        assert_eq!(numbered_name("Makefile", 2), "Makefile.2");
        assert_eq!(numbered_name(".bashrc", 2), ".bashrc.2");
    }

    #[test]
    fn the_deletion_date_has_the_spec_shape() {
        let d = deletion_date();
        assert_eq!(d.len(), 19, "{d}");
        assert_eq!(&d[10..11], "T");
    }
}
