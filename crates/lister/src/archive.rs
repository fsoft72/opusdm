//! Virtual archive filesystem support for navigating and extracting archives
//! (every format libarchive reads, see `opusdm-archive`).

use crate::model::{Row, RowAttrs};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ArchiveItem {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mode: u32,
    /// Modification time as Unix seconds, 0 when the archive stores none.
    pub mtime: i64,
}

/// Single-extension markers recognized as archive-like content, for icon and
/// file classification (`file_kind::kind_of`). A narrower subset -
/// what this app can actually extract/browse - is checked by
/// [`is_archive_file`], which also recognizes `.tar.<ext>` compound suffixes.
pub const ARCHIVE_EXTS: &[&str] =
    &["zip", "tar", "tgz", "7z", "gz", "bz2", "tbz2", "xz", "txz", "zst", "rar"];

/// Check if a path points to a supported archive file, by its name.
pub fn is_archive_file(path: &Path) -> bool {
    path.file_name().is_some_and(|n| opusdm_archive::is_archive_name(&n.to_string_lossy()))
}

/// Parse a virtual archive path formatted like `/path/to/archive.zip#sub/dir`.
/// Returns `(archive_path, inner_path)`.
///
/// The archive ends at the first `#` whose prefix names an archive, so a `#`
/// in a parent folder name (`/music/C#/a.zip#sub`) is skipped and a `#` in the
/// inner path is kept. The archive path keeps its raw (maybe non-UTF-8) bytes.
pub fn parse_archive_path(path: &Path) -> Option<(PathBuf, String)> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    bytes.iter().enumerate().filter(|&(_, &b)| b == b'#').find_map(|(i, _)| {
        let arch_path = Path::new(OsStr::from_bytes(&bytes[..i]));
        if !is_archive_file(arch_path) {
            return None;
        }
        let inner = String::from_utf8_lossy(&bytes[i + 1..]);
        Some((arch_path.to_path_buf(), inner.trim_matches('/').to_string()))
    })
}

/// Construct a virtual archive path from an archive path and inner path.
pub fn make_archive_path(arch_path: &Path, inner: &str) -> PathBuf {
    let clean = inner.trim_matches('/');
    if clean.is_empty() {
        PathBuf::from(format!("{}#", arch_path.display()))
    } else {
        PathBuf::from(format!("{}#{}", arch_path.display(), clean))
    }
}

/// Read all items contained in an archive.
pub fn read_archive_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    _check_supported(arch_path)?;
    let entries = opusdm_archive::list(arch_path)?;
    Ok(entries.into_iter().map(ArchiveItem::from).collect())
}

impl From<opusdm_archive::Entry> for ArchiveItem {
    /// The listing item of an archive entry.
    fn from(entry: opusdm_archive::Entry) -> Self {
        let is_dir = entry.kind == opusdm_archive::EntryKind::Dir;
        ArchiveItem {
            is_dir,
            path: entry.path,
            size: entry.size,
            mode: if entry.mode != 0 { entry.mode } else if is_dir { 0o755 } else { 0o644 },
            mtime: entry.mtime.unwrap_or(0),
        }
    }
}

/// Read rows for the given inner prefix within an archive.
pub fn read_archive_rows(arch_path: &Path, inner_prefix: &str) -> io::Result<Vec<Row>> {
    let items = LISTINGS.lock().unwrap_or_else(|e| e.into_inner()).get(arch_path, read_archive_entries)?;
    Ok(_rows_in(&items, inner_prefix))
}

/// The rows of the folder `inner_prefix` of an archive holding `items`: one row
/// per direct child. A folder shows once however it appears (its own entry,
/// the parent of other entries, or both) and takes its date from its own
/// entry, else from its newest content.
fn _rows_in(items: &[ArchiveItem], inner_prefix: &str) -> Vec<Row> {
    let prefix = if inner_prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", inner_prefix.trim_matches('/'))
    };

    let mut slots: Vec<(String, RowAttrs)> = Vec::new();
    let mut dir_slot: HashMap<String, usize> = HashMap::new();
    let mut own_entry: HashSet<String> = HashSet::new();

    for item in items {
        let Some(rel) = item.path.strip_prefix(&prefix) else { continue };
        if rel.is_empty() {
            continue;
        }
        let (name, nested) = match rel.split_once('/') {
            Some((dir_name, _)) => (dir_name, true),
            None => (rel, false),
        };
        if !nested && !item.is_dir {
            slots.push((
                name.to_string(),
                RowAttrs { size: item.size, modified_unix: item.mtime, uid: 1000, gid: 1000, mode: item.mode, ..Default::default() },
            ));
            continue;
        }
        let slot = *dir_slot.entry(name.to_string()).or_insert_with(|| {
            slots.push((name.to_string(), RowAttrs { is_dir: true, uid: 1000, gid: 1000, mode: 0o755, ..Default::default() }));
            slots.len() - 1
        });
        let attrs = &mut slots[slot].1;
        if !nested {
            attrs.modified_unix = item.mtime;
            attrs.mode = item.mode;
            own_entry.insert(name.to_string());
        } else if !own_entry.contains(name) {
            attrs.modified_unix = attrs.modified_unix.max(item.mtime);
        }
    }

    slots.into_iter().map(|(name, attrs)| Row::new(name, attrs)).collect()
}

/// How many archive listings [`LISTINGS`] keeps.
const LISTING_CACHE_SIZE: usize = 4;

/// Parsed listings of the archives browsed last, so moving between the
/// folders of one archive does not run the external tool again.
static LISTINGS: Mutex<ListingCache> = Mutex::new(ListingCache::new(LISTING_CACHE_SIZE));

/// What identifies one version of an archive file: its size and mtime.
type Stamp = (u64, Option<std::time::SystemTime>);

/// The stamp of `path` now.
fn _stamp(path: &Path) -> io::Result<Stamp> {
    let m = std::fs::metadata(path)?;
    Ok((m.len(), m.modified().ok()))
}

/// The parsed listings of the last `capacity` archives, most recent last.
/// An entry is valid while the archive's size and mtime are unchanged.
pub struct ListingCache {
    capacity: usize,
    entries: Vec<(PathBuf, Stamp, std::sync::Arc<Vec<ArchiveItem>>)>,
}

impl ListingCache {
    /// An empty cache holding at most `capacity` listings.
    pub const fn new(capacity: usize) -> Self {
        Self { capacity, entries: Vec::new() }
    }

    /// The listing of `arch`: the cached one when the file did not change,
    /// otherwise a fresh one read with `load` and remembered.
    pub fn get(&mut self, arch: &Path, load: impl FnOnce(&Path) -> io::Result<Vec<ArchiveItem>>) -> io::Result<std::sync::Arc<Vec<ArchiveItem>>> {
        let stamp = _stamp(arch)?;
        if let Some(i) = self.entries.iter().position(|(p, _, _)| p == arch) {
            let entry = self.entries.remove(i);
            if entry.1 == stamp {
                let items = entry.2.clone();
                self.entries.push(entry);
                return Ok(items);
            }
        }
        let items = std::sync::Arc::new(load(arch)?);
        if self.entries.len() >= self.capacity {
            self.entries.remove(0);
        }
        self.entries.push((arch.to_path_buf(), stamp, items.clone()));
        Ok(items)
    }
}

/// Fail with `Unsupported` unless `arch` has the name of a supported archive.
fn _check_supported(arch: &Path) -> io::Result<()> {
    if is_archive_file(arch) {
        return Ok(());
    }
    Err(io::Error::new(io::ErrorKind::Unsupported, "Unsupported archive format"))
}

/// Fail unless `path` exists.
fn _check_extracted(path: &Path) -> io::Result<()> {
    if path.symlink_metadata().is_ok() {
        return Ok(());
    }
    Err(io::Error::new(io::ErrorKind::NotFound, format!("not found in the archive: {}", path.display())))
}

/// Reject entry names that would climb out of the extraction folder.
fn _check_inner_path(inner_file: &str) -> io::Result<()> {
    let climbs = Path::new(inner_file).components().any(|c| matches!(c, std::path::Component::ParentDir));
    if climbs || inner_file.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid archive entry: {inner_file:?}")));
    }
    Ok(())
}

/// Extract a single file from the archive into a private temporary folder for
/// viewing, and return the path of the extracted file.
///
/// The folder is unique per call (mode 0700, unpredictable name) and is kept
/// so an external viewer can open the file; [`cleanup_temp_dirs`] removes every
/// folder created by this process.
pub fn extract_single_file_temp(arch: &Path, inner_file: &str) -> io::Result<PathBuf> {
    _check_supported(arch)?;
    _check_inner_path(inner_file)?;
    let temp_dir = tempfile::Builder::new().prefix("opusdm-preview-").tempdir()?.keep();
    PREVIEW_DIRS.lock().unwrap_or_else(|e| e.into_inner()).push(temp_dir.clone());
    let extracted = _extract_one(arch, inner_file, &temp_dir);
    if extracted.is_err() {
        if let Err(e) = std::fs::remove_dir_all(&temp_dir) {
            opusdm_ipc::err_log!("opusdm-lister", "cannot remove preview folder {}: {e}", temp_dir.display());
        }
    }
    extracted
}

/// [`extract_single_file_temp`] on a worker thread, so reading the archive never
/// blocks the GTK main loop. The returned channel yields the one result.
pub fn extract_single_file_in_background(arch: PathBuf, inner_file: String) -> async_channel::Receiver<io::Result<PathBuf>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(extract_single_file_temp(&arch, &inner_file));
    });
    rx
}

/// Extract `inner_file` of `arch` as a file named after it under `temp_dir`
/// and return its path.
fn _extract_one(arch: &Path, inner_file: &str, temp_dir: &Path) -> io::Result<PathBuf> {
    let name = Path::new(inner_file).file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid archive entry: {inner_file:?}")))?;
    let dest_path = temp_dir.join(name);
    let mut out = std::fs::File::create(&dest_path)?;
    opusdm_archive::extract_entry(arch, inner_file, &mut out)?;
    Ok(dest_path)
}

/// Preview folders created by [`extract_single_file_temp`] in this process.
static PREVIEW_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Extraction folders still needed by jobs in flight, each with the number of
/// `JobDone` replies it waits for (one per DEST Lister the job fans out to).
static SCRATCH: Mutex<VecDeque<(tempfile::TempDir, usize)>> = Mutex::new(VecDeque::new());

/// Keep `dir` alive until `jobs` jobs have finished (see [`scratch_job_done`]).
pub fn keep_scratch(dir: tempfile::TempDir, jobs: usize) {
    SCRATCH.lock().unwrap_or_else(|e| e.into_inner()).push_back((dir, jobs.max(1)));
}

/// A job finished: the oldest extraction folder has one reply fewer to wait
/// for, and is deleted when none is left. Jobs that were cancelled before they
/// started leave their folder until [`cleanup_temp_dirs`] runs at exit.
pub fn scratch_job_done() {
    let mut scratch = SCRATCH.lock().unwrap_or_else(|e| e.into_inner());
    let Some((_, waiting)) = scratch.front_mut() else { return };
    *waiting -= 1;
    if *waiting == 0 {
        scratch.pop_front();
    }
}

/// Remove every preview and extraction folder this process created (call when
/// the app quits).
pub fn cleanup_temp_dirs() {
    for dir in PREVIEW_DIRS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        let _ = std::fs::remove_dir_all(dir);
    }
    SCRATCH.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Extract items from archive into `dest_dir`, created when missing. Entry names are matched
/// literally, so they can never be read as options or wildcards. A folder
/// brings everything below it.
pub fn extract_items_to_dir(
    arch: &Path,
    items: &[String],
    dest_dir: &Path,
) -> io::Result<Vec<PathBuf>> {
    _check_supported(arch)?;
    items.iter().try_for_each(|i| _check_inner_path(i))?;
    std::fs::create_dir_all(dest_dir)?;
    opusdm_archive::extract(arch, dest_dir, Some(items), &|| false)?;
    let extracted: Vec<PathBuf> = items.iter().map(|item| dest_dir.join(item)).collect();
    extracted.iter().try_for_each(|p| _check_extracted(p))?;
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_archive_file() {
        assert!(is_archive_file(Path::new("archive.zip")));
        assert!(is_archive_file(Path::new("bundle.tar.gz")));
        assert!(is_archive_file(Path::new("archive.tar.bz2")));
        assert!(is_archive_file(Path::new("data.tar.xz")));
        assert!(is_archive_file(Path::new("project.tgz")));
        assert!(is_archive_file(Path::new("archive.7z")));
        assert!(!is_archive_file(Path::new("code.rs")));
        assert!(!is_archive_file(Path::new("data.json")));
    }

    #[test]
    fn test_archive_path_parsing() {
        let p = Path::new("/home/user/test.zip#sub/dir");
        let (arch, inner) = parse_archive_path(p).unwrap();
        assert_eq!(arch, PathBuf::from("/home/user/test.zip"));
        assert_eq!(inner, "sub/dir");

        let p_root = Path::new("/home/user/test.zip#");
        let (arch, inner) = parse_archive_path(p_root).unwrap();
        assert_eq!(arch, PathBuf::from("/home/user/test.zip"));
        assert_eq!(inner, "");
    }

    #[test]
    fn archive_path_parsing_skips_a_hash_in_a_folder_name() {
        let (arch, inner) = parse_archive_path(Path::new("/music/C#/take#2.zip#sub")).unwrap();
        assert_eq!(arch, PathBuf::from("/music/C#/take#2.zip"));
        assert_eq!(inner, "sub");
        // A `#` after the archive belongs to the inner path.
        let (arch, inner) = parse_archive_path(Path::new("/a.zip#notes#1/f.txt")).unwrap();
        assert_eq!(arch, PathBuf::from("/a.zip"));
        assert_eq!(inner, "notes#1/f.txt");
        assert!(parse_archive_path(Path::new("/music/C#/plain")).is_none());
    }

    #[test]
    fn archive_path_parsing_keeps_non_utf8_archive_names() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let p = Path::new(OsStr::from_bytes(b"/tmp/caf\xe9.zip#sub"));
        let (arch, inner) = parse_archive_path(p).unwrap();
        assert_eq!(arch.as_os_str().as_bytes(), b"/tmp/caf\xe9.zip");
        assert_eq!(inner, "sub");
    }

    /// Delete the preview folder holding `file` (tests run in parallel, so they
    /// must not call the global cleanup).
    fn remove_preview_of(file: &Path) {
        let dir = file.ancestors().find(|a| a.file_name().is_some_and(|n| n.to_string_lossy().starts_with("opusdm-preview-")));
        let _ = std::fs::remove_dir_all(dir.expect("a preview folder"));
    }

    /// Build `a.zip` and `a.tar` holding a nested file plus names that look
    /// like options and wildcards.
    fn fixture(dir: &Path) {
        use std::io::Write;
        let files = [("sub/a.txt", "nested"), ("-dash.txt", "dash"), ("st[ar]*.txt", "star")];
        let mut zip = zip::ZipWriter::new(std::fs::File::create(dir.join("a.zip")).unwrap());
        let mut tar = tar::Builder::new(std::fs::File::create(dir.join("a.tar")).unwrap());
        for (name, body) in files {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            tar.append_data(&mut header, name, body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        tar.finish().unwrap();
    }

    #[test]
    fn extract_single_file_returns_the_real_nested_path_for_every_format() {
        let td = tempfile::tempdir().unwrap();
        fixture(td.path());
        for name in ["a.zip", "a.tar"] {
            let path = extract_single_file_temp(&td.path().join(name), "sub/a.txt").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested", "{name}");
            remove_preview_of(&path);
        }
    }

    #[test]
    fn background_extraction_sends_the_result_through_the_channel() {
        let td = tempfile::tempdir().unwrap();
        fixture(td.path());
        let rx = extract_single_file_in_background(td.path().join("a.zip"), "sub/a.txt".to_string());
        let path = rx.recv_blocking().unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested");
        remove_preview_of(&path);
        let rx = extract_single_file_in_background(td.path().join("a.zip"), "nope.txt".to_string());
        assert!(rx.recv_blocking().unwrap().is_err());
    }

    #[test]
    fn listing_cache_reads_each_archive_version_once() {
        let td = tempfile::tempdir().unwrap();
        let arch = td.path().join("a.zip");
        std::fs::write(&arch, "one").unwrap();
        let loads = std::cell::Cell::new(0);
        let load = |_: &Path| {
            loads.set(loads.get() + 1);
            Ok(vec![ArchiveItem { path: "f".into(), is_dir: false, size: 1, mode: 0o644, mtime: 0 }])
        };
        let mut cache = ListingCache::new(2);
        cache.get(&arch, load).unwrap();
        cache.get(&arch, load).unwrap();
        assert_eq!(loads.get(), 1, "unchanged archive: cached");
        std::fs::write(&arch, "changed size").unwrap();
        cache.get(&arch, load).unwrap();
        assert_eq!(loads.get(), 2, "a changed archive is read again");
    }

    #[test]
    fn listing_cache_drops_the_oldest_archive() {
        let td = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = (0..3).map(|i| td.path().join(format!("{i}.zip"))).collect();
        for p in &paths {
            std::fs::write(p, "x").unwrap();
        }
        let loads = std::cell::Cell::new(0);
        let load = |_: &Path| {
            loads.set(loads.get() + 1);
            Ok(Vec::new())
        };
        let mut cache = ListingCache::new(2);
        for p in &paths {
            cache.get(p, load).unwrap();
        }
        cache.get(&paths[2], load).unwrap();
        assert_eq!(loads.get(), 3);
        cache.get(&paths[0], load).unwrap();
        assert_eq!(loads.get(), 4, "the first archive was evicted");
        assert!(cache.get(&td.path().join("missing.zip"), load).is_err());
    }

    #[test]
    fn extract_single_file_reports_a_missing_entry_instead_of_an_empty_file() {
        let td = tempfile::tempdir().unwrap();
        fixture(td.path());
        for name in ["a.zip", "a.tar"] {
            assert!(extract_single_file_temp(&td.path().join(name), "nope.txt").is_err(), "{name}");
        }
    }

    #[test]
    fn extract_rejects_unsupported_formats_and_parent_components() {
        let td = tempfile::tempdir().unwrap();
        let err = extract_single_file_temp(&td.path().join("a.txt"), "x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        let err = extract_items_to_dir(&td.path().join("a.zip"), &["../x".to_string()], td.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn extract_items_treats_names_literally() {
        let td = tempfile::tempdir().unwrap();
        fixture(td.path());
        let items = vec!["-dash.txt".to_string(), "st[ar]*.txt".to_string()];
        for name in ["a.zip", "a.tar"] {
            let out = td.path().join(format!("out-{name}"));
            std::fs::create_dir(&out).unwrap();
            let paths = extract_items_to_dir(&td.path().join(name), &items, &out).unwrap();
            assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), "dash", "{name}");
            assert_eq!(std::fs::read_to_string(&paths[1]).unwrap(), "star", "{name}");
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2, "{name}: nothing else extracted");
        }
    }

    #[test]
    fn extract_items_creates_the_destination_folder() {
        let td = tempfile::tempdir().unwrap();
        fixture(td.path());
        let out = td.path().join("scratch").join("0");
        let items = vec!["-dash.txt".to_string()];
        let paths = extract_items_to_dir(&td.path().join("a.zip"), &items, &out).unwrap();
        assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), "dash");
    }

    /// A listing item for the row tests.
    fn item(path: &str, is_dir: bool, size: u64, mtime: i64) -> ArchiveItem {
        ArchiveItem { path: path.into(), is_dir, size, mode: if is_dir { 0o755 } else { 0o644 }, mtime }
    }

    #[test]
    fn a_folder_with_its_own_entry_and_content_is_listed_once() {
        let items = [item("sub", true, 0, 100), item("sub/b.txt", false, 6, 200), item("a.txt", false, 6, 300)];
        let rows = _rows_in(&items, "");
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["sub", "a.txt"]);
        assert!(rows[0].is_dir);
    }

    #[test]
    fn a_folder_entry_after_its_content_is_still_listed_once() {
        let items = [item("sub/b.txt", false, 6, 200), item("sub", true, 0, 100)];
        let rows = _rows_in(&items, "");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].modified_unix, 100, "its own entry wins, wherever it is");
    }

    #[test]
    fn rows_carry_the_dates_of_the_archive() {
        let items = [item("sub", true, 0, 100), item("sub/b.txt", false, 6, 200), item("a.txt", false, 6, 300)];
        let rows = _rows_in(&items, "");
        assert_eq!(rows[0].modified_unix, 100, "a folder with an entry shows the date of the entry");
        assert_eq!(rows[1].modified_unix, 300);
        assert_eq!(_rows_in(&items, "sub")[0].modified_unix, 200);
    }

    #[test]
    fn a_folder_without_an_entry_takes_the_newest_date_of_its_content() {
        let items = [item("d/old.txt", false, 1, 10), item("d/new.txt", false, 1, 90), item("d/sub/x.txt", false, 1, 50)];
        let rows = _rows_in(&items, "");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].modified_unix, 90);
    }

    #[test]
    fn a_nested_folder_is_listed_once_inside_its_parent() {
        let items = [item("p", true, 0, 1), item("p/q", true, 0, 2), item("p/q/f", false, 3, 4), item("p/g", false, 1, 5)];
        let names: Vec<String> = _rows_in(&items, "p").into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["q", "g"]);
    }

    #[test]
    fn scratch_folder_lives_until_every_job_reply_arrived() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("x");
        std::fs::create_dir(&path).unwrap();
        let dir = tempfile::Builder::new().tempdir_in(td.path()).unwrap();
        let dir_path = dir.path().to_path_buf();
        keep_scratch(dir, 2);
        scratch_job_done();
        assert!(dir_path.exists(), "one of two replies is not enough");
        scratch_job_done();
        assert!(!dir_path.exists(), "gone after the last reply");
        scratch_job_done(); // no folder left: must not panic
    }
}
