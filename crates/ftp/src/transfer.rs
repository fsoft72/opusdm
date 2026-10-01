//! Downloading remote files and folders into a local directory.

use crate::{RemoteError, RemoteFs};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// How deep a download follows directories (guards against link loops).
const MAX_DEPTH: usize = 64;

/// One thing to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteItem {
    /// Absolute path on the server.
    pub path: String,
    /// Name to give it locally.
    pub name: String,
    /// A directory: its whole tree is downloaded.
    pub is_dir: bool,
}

/// Whether `name` is safe to create locally: a server must not be able to
/// write outside the target through `..`, a `/` or a NUL.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// Join a server directory and a name.
fn join_remote(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// Download `items` into `dest_dir`. Returns the local paths of the items
/// (one per item, in order). `progress` gets each file's remote path before
/// it starts; `cancel` is checked between files.
pub fn download(
    fs: &mut dyn RemoteFs,
    items: &[RemoteItem],
    dest_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Vec<PathBuf>, RemoteError> {
    let mut made = Vec::with_capacity(items.len());
    for item in items {
        if !safe_name(&item.name) {
            return Err(RemoteError::Failed(format!("refusing the unsafe name {:?}", item.name)));
        }
        let local = dest_dir.join(&item.name);
        if item.is_dir {
            download_dir(fs, &item.path, &local, cancel, progress, 0)?;
        } else {
            download_file(fs, &item.path, &local, cancel, progress)?;
        }
        made.push(local);
    }
    Ok(made)
}

fn download_file(
    fs: &mut dyn RemoteFs,
    remote: &str,
    local: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), RemoteError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(RemoteError::Failed("cancelled".to_string()));
    }
    progress(remote);
    let mut file = File::create(local)?;
    let result = fs.read_file(remote, &mut file);
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(local);
    }
    result.map(|_| ())
}

fn download_dir(
    fs: &mut dyn RemoteFs,
    remote: &str,
    local: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
    depth: usize,
) -> Result<(), RemoteError> {
    if depth > MAX_DEPTH {
        return Err(RemoteError::Failed(format!("{remote}: folders nested too deep (a link loop?)")));
    }
    std::fs::create_dir_all(local)?;
    for entry in fs.list(remote)? {
        if !safe_name(&entry.name) {
            return Err(RemoteError::Failed(format!("{remote}: refusing the unsafe name {:?}", entry.name)));
        }
        let child_remote = join_remote(remote, &entry.name);
        let child_local = local.join(&entry.name);
        if entry.is_dir {
            download_dir(fs, &child_remote, &child_local, cancel, progress, depth + 1)?;
        } else {
            download_file(fs, &child_remote, &child_local, cancel, progress)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RemoteEntry;
    use std::collections::HashMap;
    use std::io::Write;

    /// An in-memory server: directories map to entries, files to content.
    #[derive(Default)]
    struct Fake {
        dirs: HashMap<String, Vec<RemoteEntry>>,
        files: HashMap<String, Vec<u8>>,
    }

    fn entry(name: &str, is_dir: bool) -> RemoteEntry {
        RemoteEntry { name: name.into(), is_dir, is_symlink: false, size: 0, modified: 0, mode: 0 }
    }

    impl RemoteFs for Fake {
        fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>, RemoteError> {
            self.dirs.get(path).cloned().ok_or_else(|| RemoteError::Failed(format!("no dir {path}")))
        }
        fn read_file(&mut self, path: &str, out: &mut dyn Write) -> Result<u64, RemoteError> {
            let data = self.files.get(path).ok_or_else(|| RemoteError::Failed(format!("no file {path}")))?;
            out.write_all(data)?;
            Ok(data.len() as u64)
        }
        fn close(&mut self) {}
    }

    fn tree() -> Fake {
        let mut f = Fake::default();
        f.dirs.insert("/d".into(), vec![entry("a.txt", false), entry("sub", true)]);
        f.dirs.insert("/d/sub".into(), vec![entry("b.txt", false)]);
        f.files.insert("/d/a.txt".into(), b"AAA".to_vec());
        f.files.insert("/d/sub/b.txt".into(), b"BB".to_vec());
        f.files.insert("/top.txt".into(), b"T".to_vec());
        f
    }

    fn items() -> Vec<RemoteItem> {
        vec![
            RemoteItem { path: "/d".into(), name: "d".into(), is_dir: true },
            RemoteItem { path: "/top.txt".into(), name: "top.txt".into(), is_dir: false },
        ]
    }

    #[test]
    fn downloads_files_and_whole_trees() {
        let td = tempfile::tempdir().unwrap();
        let mut seen = Vec::new();
        let made = download(&mut tree(), &items(), td.path(), &AtomicBool::new(false), &mut |p| seen.push(p.to_string())).unwrap();
        assert_eq!(made, vec![td.path().join("d"), td.path().join("top.txt")]);
        assert_eq!(std::fs::read(td.path().join("d/a.txt")).unwrap(), b"AAA");
        assert_eq!(std::fs::read(td.path().join("d/sub/b.txt")).unwrap(), b"BB");
        assert_eq!(std::fs::read(td.path().join("top.txt")).unwrap(), b"T");
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn unsafe_names_are_refused_before_anything_is_written() {
        for bad in ["..", "a/b", "", ".", "x\0y"] {
            assert!(!safe_name(bad), "{bad:?}");
        }
        let td = tempfile::tempdir().unwrap();
        let evil = vec![RemoteItem { path: "/top.txt".into(), name: "../evil".into(), is_dir: false }];
        assert!(download(&mut tree(), &evil, td.path(), &AtomicBool::new(false), &mut |_| {}).is_err());
        assert!(!td.path().parent().unwrap().join("evil").exists());

        let mut hostile = tree();
        hostile.dirs.insert("/d".into(), vec![entry("..", false)]);
        assert!(download(&mut hostile, &items()[..1], td.path(), &AtomicBool::new(false), &mut |_| {}).is_err());
    }

    #[test]
    fn a_failed_file_leaves_no_partial_copy() {
        let td = tempfile::tempdir().unwrap();
        let missing = vec![RemoteItem { path: "/nope".into(), name: "nope".into(), is_dir: false }];
        assert!(download(&mut tree(), &missing, td.path(), &AtomicBool::new(false), &mut |_| {}).is_err());
        assert!(!td.path().join("nope").exists());
    }

    #[test]
    fn cancel_stops_the_download() {
        let td = tempfile::tempdir().unwrap();
        let err = download(&mut tree(), &items(), td.path(), &AtomicBool::new(true), &mut |_| {}).unwrap_err();
        assert_eq!(err, RemoteError::Failed("cancelled".to_string()));
    }

    #[test]
    fn a_link_loop_is_cut_off() {
        let mut f = Fake::default();
        f.dirs.insert("/l".into(), vec![entry("l", true)]);
        // Every nested path lists the same single child, forever.
        let mut path = "/l".to_string();
        for _ in 0..=MAX_DEPTH + 1 {
            path.push_str("/l");
            f.dirs.insert(path.clone(), vec![entry("l", true)]);
        }
        let td = tempfile::tempdir().unwrap();
        let item = vec![RemoteItem { path: "/l".into(), name: "l".into(), is_dir: true }];
        assert!(download(&mut f, &item, td.path(), &AtomicBool::new(false), &mut |_| {}).is_err());
    }
}
