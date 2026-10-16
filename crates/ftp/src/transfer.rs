//! Downloading remote files and folders into a local directory.

use crate::{RemoteError, RemoteFs};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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

/// Download one file. The data goes to a temp file next to `local` that
/// replaces it only once the transfer succeeded, so a failed or cancelled
/// download never destroys an existing local copy.
fn download_file(
    fs: &mut dyn RemoteFs,
    remote: &str,
    local: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), RemoteError> {
    if cancel.load(Ordering::Relaxed) { return Err(RemoteError::Cancelled); }
    progress(remote);
    let part = part_sibling(local);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&part)?;
    let result = fs
        .read_file(remote, &mut file)
        .and_then(|_| file.sync_all().map_err(RemoteError::from))
        .and_then(|()| std::fs::rename(&part, local).map_err(RemoteError::from));
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// A hidden, unused path in the same folder as `local`, so the final
/// `rename` stays on one filesystem.
fn part_sibling(local: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    local.with_file_name(format!(".{name}.opusdm-part-{}-{seq}", std::process::id()))
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

/// Upload the local files and folders in `local` into the remote directory
/// `remote_dir`. A folder keeps its tree; symbolic links and other special
/// files are skipped. Returns the remote path of every top-level item.
pub fn upload(
    fs: &mut dyn RemoteFs,
    local: &[PathBuf],
    remote_dir: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Vec<String>, RemoteError> {
    let mut made = Vec::with_capacity(local.len());
    for path in local {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !safe_name(&name) {
            return Err(RemoteError::Failed(format!("{}: not a usable file name", path.display())));
        }
        let target = join_remote(remote_dir, &name);
        upload_item(fs, path, &target, cancel, progress, 0)?;
        made.push(target);
    }
    Ok(made)
}

fn upload_item(
    fs: &mut dyn RemoteFs,
    local: &Path,
    remote: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
    depth: usize,
) -> Result<(), RemoteError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(RemoteError::Cancelled);
    }
    let meta = std::fs::symlink_metadata(local)?;
    if meta.is_dir() {
        if depth > MAX_DEPTH {
            return Err(RemoteError::Failed(format!("{}: folders nested too deep", local.display())));
        }
        fs.mkdir(remote)?;
        let mut children: Vec<_> = std::fs::read_dir(local)?.filter_map(Result::ok).collect();
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            upload_item(fs, &child.path(), &join_remote(remote, &name), cancel, progress, depth + 1)?;
        }
    } else if meta.is_file() {
        progress(remote);
        let mut file = File::open(local)?;
        fs.write_file(remote, &mut file)?;
    }
    Ok(())
}

/// Delete `items` on the server, folders with everything inside. Files go
/// first, so a failure leaves a consistent remainder.
pub fn remove_tree(
    fs: &mut dyn RemoteFs,
    items: &[RemoteItem],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), RemoteError> {
    for item in items {
        remove_item(fs, &item.path, item.is_dir, cancel, progress, 0)?;
    }
    Ok(())
}

fn remove_item(
    fs: &mut dyn RemoteFs,
    path: &str,
    is_dir: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
    depth: usize,
) -> Result<(), RemoteError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(RemoteError::Cancelled);
    }
    if path.trim_end_matches('/').is_empty() {
        return Err(RemoteError::Failed("refusing to delete the server root".to_string()));
    }
    progress(path);
    if !is_dir {
        return fs.remove_file(path);
    }
    if depth > MAX_DEPTH {
        return Err(RemoteError::Failed(format!("{path}: folders nested too deep (a link loop?)")));
    }
    for entry in fs.list(path)? {
        if !safe_name(&entry.name) {
            return Err(RemoteError::Failed(format!("{path}: refusing the unsafe name {:?}", entry.name)));
        }
        // A link is removed as a link, never followed into.
        let child_is_dir = entry.is_dir && !entry.is_symlink;
        remove_item(fs, &join_remote(path, &entry.name), child_is_dir, cancel, progress, depth + 1)?;
    }
    fs.remove_dir(path)
}

/// Look up the server paths in `paths` (absolute, no trailing `/`) and say
/// which are folders. Fails when one does not exist.
pub fn resolve_items(fs: &mut dyn RemoteFs, paths: &[String]) -> Result<Vec<RemoteItem>, RemoteError> {
    let mut listed: Option<(String, Vec<crate::RemoteEntry>)> = None;
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let cut = path.rfind('/').ok_or_else(|| RemoteError::Failed(format!("{path}: not an absolute path")))?;
        let (dir, name) = (if cut == 0 { "/" } else { &path[..cut] }, &path[cut + 1..]);
        if !safe_name(name) {
            return Err(RemoteError::Failed(format!("{path}: not a usable name")));
        }
        if listed.as_ref().is_none_or(|(d, _)| d != dir) {
            listed = Some((dir.to_string(), fs.list(dir)?));
        }
        let entry = listed
            .as_ref()
            .and_then(|(_, entries)| entries.iter().find(|e| e.name == name))
            .ok_or_else(|| RemoteError::Failed(format!("{path}: no such file or folder")))?;
        items.push(RemoteItem { path: path.clone(), name: name.to_string(), is_dir: entry.is_dir });
    }
    Ok(items)
}

/// Which of `names` already exist in the remote directory `dir`.
pub fn existing_remote(fs: &mut dyn RemoteFs, dir: &str, names: &[String]) -> Result<Vec<String>, RemoteError> {
    let present: Vec<String> = fs.list(dir)?.into_iter().map(|e| e.name).collect();
    Ok(names.iter().filter(|n| present.contains(n)).cloned().collect())
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
        fn write_file(&mut self, path: &str, data: &mut dyn std::io::Read) -> Result<u64, RemoteError> {
            let mut buf = Vec::new();
            data.read_to_end(&mut buf)?;
            let n = buf.len() as u64;
            self.files.insert(path.to_string(), buf);
            self.add_to_parent(path, false);
            Ok(n)
        }
        fn mkdir(&mut self, path: &str) -> Result<(), RemoteError> {
            if !self.dirs.contains_key(path) {
                self.dirs.insert(path.to_string(), Vec::new());
                self.add_to_parent(path, true);
            }
            Ok(())
        }
        fn remove_file(&mut self, path: &str) -> Result<(), RemoteError> {
            self.files.remove(path).ok_or_else(|| RemoteError::Failed(format!("no file {path}")))?;
            self.drop_from_parent(path);
            Ok(())
        }
        fn remove_dir(&mut self, path: &str) -> Result<(), RemoteError> {
            if !self.dirs.get(path).is_some_and(|d| d.is_empty()) {
                return Err(RemoteError::Failed(format!("{path}: not empty")));
            }
            self.dirs.remove(path);
            self.drop_from_parent(path);
            Ok(())
        }
        fn close(&mut self) {}
    }

    impl Fake {
        fn split(path: &str) -> (String, String) {
            let cut = path.rfind('/').unwrap_or(0);
            let parent = if cut == 0 { "/".to_string() } else { path[..cut].to_string() };
            (parent, path[cut + 1..].to_string())
        }
        fn add_to_parent(&mut self, path: &str, is_dir: bool) {
            let (parent, name) = Self::split(path);
            let list = self.dirs.entry(parent).or_default();
            list.retain(|e| e.name != name);
            list.push(entry(&name, is_dir));
        }
        fn drop_from_parent(&mut self, path: &str) {
            let (parent, name) = Self::split(path);
            if let Some(list) = self.dirs.get_mut(&parent) {
                list.retain(|e| e.name != name);
            }
        }
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

    /// A failed re-download keeps the local copy that was already there and
    /// leaves no temp file behind.
    #[test]
    fn a_failed_download_keeps_the_existing_local_file() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("nope"), b"KEEP").unwrap();
        let missing = vec![RemoteItem { path: "/nope".into(), name: "nope".into(), is_dir: false }];
        assert!(download(&mut tree(), &missing, td.path(), &AtomicBool::new(false), &mut |_| {}).is_err());
        assert_eq!(std::fs::read(td.path().join("nope")).unwrap(), b"KEEP");
        assert_eq!(std::fs::read_dir(td.path()).unwrap().count(), 1);
    }

    /// A successful download replaces an existing local file and leaves no
    /// temp file behind.
    #[test]
    fn a_download_replaces_the_existing_local_file() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("top.txt"), b"OLD").unwrap();
        download(&mut tree(), &items()[1..], td.path(), &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(std::fs::read(td.path().join("top.txt")).unwrap(), b"T");
        assert_eq!(std::fs::read_dir(td.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancel_stops_the_download() {
        let td = tempfile::tempdir().unwrap();
        let err = download(&mut tree(), &items(), td.path(), &AtomicBool::new(true), &mut |_| {}).unwrap_err();
        assert_eq!(err, RemoteError::Cancelled);
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

    #[test]
    fn upload_creates_the_tree_and_replaces_files() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("d/sub")).unwrap();
        std::fs::write(td.path().join("d/a.txt"), b"AAA").unwrap();
        std::fs::write(td.path().join("d/sub/b.txt"), b"BB").unwrap();
        std::fs::write(td.path().join("top.txt"), b"T").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", td.path().join("d/link")).unwrap();

        let mut fake = Fake::default();
        fake.dirs.insert("/up".into(), Vec::new());
        fake.files.insert("/up/top.txt".into(), b"old".to_vec());
        let made = upload(&mut fake, &[td.path().join("d"), td.path().join("top.txt")], "/up", &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(made, vec!["/up/d".to_string(), "/up/top.txt".to_string()]);
        assert_eq!(fake.files["/up/d/a.txt"], b"AAA");
        assert_eq!(fake.files["/up/d/sub/b.txt"], b"BB");
        assert_eq!(fake.files["/up/top.txt"], b"T");
        assert!(!fake.files.contains_key("/up/d/link"), "links are skipped");
    }

    #[test]
    fn upload_stops_when_cancelled() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a"), b"A").unwrap();
        let mut fake = Fake::default();
        fake.dirs.insert("/up".into(), Vec::new());
        assert!(upload(&mut fake, &[td.path().join("a")], "/up", &AtomicBool::new(true), &mut |_| {}).is_err());
        assert!(fake.files.is_empty());
    }

    #[test]
    fn remove_tree_deletes_files_then_folders() {
        let mut fake = tree();
        remove_tree(&mut fake, &items(), &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(fake.files.is_empty());
        assert!(!fake.dirs.contains_key("/d") && !fake.dirs.contains_key("/d/sub"));
    }

    #[test]
    fn remove_tree_refuses_the_root_and_unsafe_names() {
        let mut fake = tree();
        let root = vec![RemoteItem { path: "/".into(), name: "".into(), is_dir: true }];
        assert!(remove_tree(&mut fake, &root, &AtomicBool::new(false), &mut |_| {}).is_err());
        fake.dirs.insert("/d".into(), vec![entry("..", true)]);
        assert!(remove_tree(&mut fake, &items()[..1], &AtomicBool::new(false), &mut |_| {}).is_err());
    }

    #[test]
    fn resolve_items_finds_kinds_and_rejects_missing_paths() {
        let mut fake = tree();
        let items = resolve_items(&mut fake, &["/d/sub".to_string(), "/d/a.txt".to_string()]).unwrap();
        assert_eq!(items[0], RemoteItem { path: "/d/sub".into(), name: "sub".into(), is_dir: true });
        assert!(!items[1].is_dir);
        assert!(resolve_items(&mut fake, &["/d/missing".to_string()]).is_err());
        assert!(resolve_items(&mut fake, &["/d/..".to_string()]).is_err());
    }

    #[test]
    fn existing_remote_reports_only_names_that_are_there() {
        let mut fake = tree();
        let found = existing_remote(&mut fake, "/d", &["a.txt".to_string(), "zzz".to_string()]).unwrap();
        assert_eq!(found, vec!["a.txt".to_string()]);
    }
}
