//! Virtual archive filesystem support for navigating and extracting Zip, Tar, 7z.

use crate::model::{Row, RowAttrs};
use std::collections::{HashSet, VecDeque};
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ArchiveItem {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mode: u32,
}

/// Single-extension markers recognized as archive-like content, for icon and
/// preview classification (`preview::detect_file_kind`). A narrower subset -
/// what this app can actually extract/browse - is checked by
/// [`is_archive_file`], which also recognizes `.tar.<ext>` compound suffixes.
pub const ARCHIVE_EXTS: &[&str] =
    &["zip", "tar", "tgz", "7z", "gz", "bz2", "tbz2", "xz", "txz", "zst", "rar"];

/// Check if a path points to a supported archive file.
pub fn is_archive_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if name.ends_with(".tar.gz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".tar.zst")
        || name.ends_with(".tgz")
    {
        return true;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    matches!(ext.as_str(), "zip" | "tar" | "tgz" | "7z")
}

/// Parse a virtual archive path formatted like `/path/to/archive.zip#sub/dir`.
/// Returns `(archive_path, inner_path)`.
pub fn parse_archive_path(path: &Path) -> Option<(PathBuf, String)> {
    let s = path.to_string_lossy();
    if let Some((arch_str, inner_str)) = s.split_once('#') {
        let arch_path = PathBuf::from(arch_str);
        if is_archive_file(&arch_path) {
            return Some((arch_path, inner_str.trim_matches('/').to_string()));
        }
    }
    None
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
    let ext = arch_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let name = arch_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "zip" {
        read_zip_entries(arch_path)
    } else if name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.xz")
        || name.ends_with(".tar.zst")
        || ext == "tar"
    {
        read_tar_entries(arch_path)
    } else if ext == "7z" {
        read_7z_entries(arch_path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unsupported archive format",
        ))
    }
}

fn read_zip_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    let output = Command::new("zipinfo")
        .arg("-s")
        .arg(arch_path)
        .output()?;

    if !output.status.success() {
        // Fallback to unzip -Z -1
        let out2 = Command::new("unzip")
            .arg("-Z")
            .arg("-1")
            .arg(arch_path)
            .output()?;
        let mut items = Vec::new();
        for line in out2.stdout.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let is_dir = trimmed.ends_with('/');
            items.push(ArchiveItem {
                path: trimmed.trim_end_matches('/').to_string(),
                is_dir,
                size: 0,
                mode: if is_dir { 0o755 } else { 0o644 },
            });
        }
        return Ok(items);
    }

    let mut items = Vec::new();
    for line in output.stdout.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.starts_with("Archive:")
            || trimmed.starts_with("Zip file size:")
            || trimmed.ends_with("compressed:")
            || trimmed.is_empty()
        {
            continue;
        }

        // Example: -rw-rw-r--  3.0 unx        5 tx stor 26-Sep-03 15:27 file.txt
        let is_dir = trimmed.starts_with('d');
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 7 {
            let size = parts.get(3).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let path = parts[7..].join(" ");
            let clean_path = path.trim_end_matches('/').to_string();
            items.push(ArchiveItem {
                path: clean_path,
                is_dir: is_dir || path.ends_with('/'),
                size,
                mode: if is_dir { 0o755 } else { 0o644 },
            });
        }
    }

    Ok(items)
}

fn read_tar_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    let output = Command::new("tar")
        .arg("-tvf")
        .arg(arch_path)
        .output()?;

    let mut items = Vec::new();
    for line in output.stdout.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Example: -rw-rw-r-- fabio/fabio 6 2026-09-03 15:27 hello.txt
        let is_dir = trimmed.starts_with('d');
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 6 {
            let size = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let path = parts[5..].join(" ");
            let clean_path = path.trim_end_matches('/').to_string();
            items.push(ArchiveItem {
                path: clean_path,
                is_dir: is_dir || path.ends_with('/'),
                size,
                mode: if is_dir { 0o755 } else { 0o644 },
            });
        }
    }

    Ok(items)
}

fn read_7z_entries(arch_path: &Path) -> io::Result<Vec<ArchiveItem>> {
    let output = Command::new("7z")
        .arg("l")
        .arg(arch_path)
        .output()?;

    let mut items = Vec::new();
    let mut in_files = false;
    for line in output.stdout.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.starts_with("-------------------") {
            in_files = !in_files;
            continue;
        }
        if in_files && trimmed.len() > 53 {
            let is_dir = trimmed.chars().nth(20) == Some('D');
            let size_str = &trimmed[26..38].trim();
            let size = size_str.parse::<u64>().unwrap_or(0);
            let path = trimmed[53..].trim().trim_end_matches('/').to_string();
            if !path.is_empty() {
                items.push(ArchiveItem {
                    path,
                    is_dir,
                    size,
                    mode: if is_dir { 0o755 } else { 0o644 },
                });
            }
        }
    }

    Ok(items)
}

/// Read rows for the given inner prefix within an archive.
pub fn read_archive_rows(arch_path: &Path, inner_prefix: &str) -> io::Result<Vec<Row>> {
    let items = LISTINGS.lock().unwrap_or_else(|e| e.into_inner()).get(arch_path, read_archive_entries)?;

    let prefix = if inner_prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", inner_prefix.trim_matches('/'))
    };

    let mut seen_dirs = HashSet::new();
    let mut rows = Vec::new();

    for item in items.iter() {
        if !item.path.starts_with(&prefix) && !prefix.is_empty() {
            continue;
        }
        let rel = if prefix.is_empty() {
            &item.path[..]
        } else {
            &item.path[prefix.len()..]
        };

        if rel.is_empty() {
            continue;
        }

        if let Some((dir_name, _)) = rel.split_once('/') {
            if seen_dirs.insert(dir_name.to_string()) {
                rows.push(Row::new(
                    dir_name.to_string(),
                    RowAttrs { is_dir: true, uid: 1000, gid: 1000, mode: 0o755, ..Default::default() },
                ));
            }
        } else {
            let is_dir = item.is_dir;
            let name = rel.to_string();
            rows.push(Row::new(
                name,
                RowAttrs {
                    is_dir,
                    size: if is_dir { 0 } else { item.size },
                    uid: 1000,
                    gid: 1000,
                    mode: item.mode,
                    ..Default::default()
                },
            ));
        }
    }

    Ok(rows)
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

/// Archive formats the extraction commands know how to handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    Tar,
    SevenZ,
}

/// Classify `arch` by its name, or fail with `Unsupported` for any other format.
fn _archive_kind(arch: &Path) -> io::Result<ArchiveKind> {
    let name = arch.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    let tar_suffixes = [".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar.zst"];
    if name.ends_with(".zip") {
        Ok(ArchiveKind::Zip)
    } else if tar_suffixes.iter().any(|s| name.ends_with(s)) {
        Ok(ArchiveKind::Tar)
    } else if name.ends_with(".7z") {
        Ok(ArchiveKind::SevenZ)
    } else {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Unsupported archive format"))
    }
}

/// Escape an entry name for `unzip`, which reads `[`, `*` and `?` as wildcards
/// and a leading `-` as an option.
fn _unzip_pattern(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (i, c) in name.chars().enumerate() {
        if matches!(c, '[' | ']' | '*' | '?' | '\\') || (i == 0 && c == '-') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Fail with a descriptive error unless `status` is a success.
fn _check_status(status: std::process::ExitStatus, what: &str) -> io::Result<()> {
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!("{what} failed ({status})")))
}

/// Fail unless `path` exists (`7z` exits with success when an entry is not in the archive).
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
    let kind = _archive_kind(arch)?;
    _check_inner_path(inner_file)?;
    let temp_dir = tempfile::Builder::new().prefix("opusdm-preview-").tempdir()?.keep();
    PREVIEW_DIRS.lock().unwrap_or_else(|e| e.into_inner()).push(temp_dir.clone());
    let extracted = _extract_one(kind, arch, inner_file, &temp_dir);
    if extracted.is_err() {
        if let Err(e) = std::fs::remove_dir_all(&temp_dir) {
            opusdm_ipc::err_log!("opusdm-lister", "cannot remove preview folder {}: {e}", temp_dir.display());
        }
    }
    extracted
}

/// [`extract_single_file_temp`] on a worker thread, so the external tool never
/// blocks the GTK main loop. The returned channel yields the one result.
pub fn extract_single_file_in_background(arch: PathBuf, inner_file: String) -> async_channel::Receiver<io::Result<PathBuf>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(extract_single_file_temp(&arch, &inner_file));
    });
    rx
}

/// Extract `inner_file` of `arch` under `temp_dir` and return its path.
fn _extract_one(kind: ArchiveKind, arch: &Path, inner_file: &str, temp_dir: &Path) -> io::Result<PathBuf> {
    match kind {
        ArchiveKind::Zip => {
            let name = Path::new(inner_file).file_name().unwrap_or_default();
            let dest_path = temp_dir.join(name);
            let output = Command::new("unzip").arg("-p").arg(arch).arg(_unzip_pattern(inner_file)).output()?;
            _check_status(output.status, "unzip")?;
            std::fs::write(&dest_path, &output.stdout)?;
            Ok(dest_path)
        }
        ArchiveKind::Tar => {
            let output = Command::new("tar")
                .arg("-xf")
                .arg(arch)
                .arg("--no-wildcards")
                .arg("-C")
                .arg(temp_dir)
                .arg("--")
                .arg(inner_file)
                .output()?;
            _check_status(output.status, "tar")?;
            let extracted = temp_dir.join(inner_file);
            _check_extracted(&extracted)?;
            Ok(extracted)
        }
        ArchiveKind::SevenZ => {
            let output = Command::new("7z")
                .args(["x", "-y", "-spd", "-bso0", "-bsp0"])
                .arg(format!("-o{}", temp_dir.display()))
                .arg("--")
                .arg(arch)
                .arg(inner_file)
                .output()?;
            _check_status(output.status, "7z")?;
            let extracted = temp_dir.join(inner_file);
            _check_extracted(&extracted)?;
            Ok(extracted)
        }
    }
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

/// Extract items from archive into `dest_dir`. Entry names are passed so they
/// can never be read as options or wildcards.
pub fn extract_items_to_dir(
    arch: &Path,
    items: &[String],
    dest_dir: &Path,
) -> io::Result<Vec<PathBuf>> {
    let kind = _archive_kind(arch)?;
    items.iter().try_for_each(|i| _check_inner_path(i))?;

    match kind {
        ArchiveKind::Zip => {
            let mut cmd = Command::new("unzip");
            cmd.arg("-o").arg(arch);
            for item in items {
                cmd.arg(_unzip_pattern(item));
            }
            cmd.arg("-d").arg(dest_dir);
            _check_status(cmd.status()?, "unzip")?;
        }
        ArchiveKind::Tar => {
            let mut cmd = Command::new("tar");
            cmd.arg("-xf").arg(arch).arg("--no-wildcards").arg("-C").arg(dest_dir).arg("--");
            cmd.args(items);
            _check_status(cmd.status()?, "tar")?;
        }
        ArchiveKind::SevenZ => {
            let mut cmd = Command::new("7z");
            cmd.args(["x", "-y", "-spd", "-bso0", "-bsp0"]).arg(format!("-o{}", dest_dir.display()));
            cmd.arg("--").arg(arch);
            cmd.args(items);
            _check_status(cmd.status()?, "7z")?;
        }
    }

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

    /// Delete the preview folder holding `file` (tests run in parallel, so they
    /// must not call the global cleanup).
    fn remove_preview_of(file: &Path) {
        let dir = file.ancestors().find(|a| a.file_name().is_some_and(|n| n.to_string_lossy().starts_with("opusdm-preview-")));
        let _ = std::fs::remove_dir_all(dir.expect("a preview folder"));
    }

    /// Build `a.zip`, `a.tar` and `a.7z` holding a nested file plus names that
    /// look like options and wildcards. Returns `None` when a tool is missing.
    fn fixture(dir: &Path) -> Option<()> {
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("sub")).ok()?;
        std::fs::write(src.join("sub/a.txt"), "nested").ok()?;
        std::fs::write(src.join("-dash.txt"), "dash").ok()?;
        std::fs::write(src.join("st[ar]*.txt"), "star").ok()?;
        let run = |prog: &str, args: &[&str]| {
            Command::new(prog).current_dir(&src).args(args).output().is_ok_and(|o| o.status.success())
        };
        let ok = run("zip", &["-q", "../a.zip", "sub/a.txt", "--", "-dash.txt", "st[ar]*.txt"])
            && run("tar", &["cf", "../a.tar", "sub/a.txt", "--", "-dash.txt", "st[ar]*.txt"])
            && run("7z", &["a", "-bso0", "-spd", "../a.7z", "sub/a.txt", "--", "-dash.txt", "st[ar]*.txt"]);
        ok.then_some(())
    }

    #[test]
    fn extract_single_file_returns_the_real_nested_path_for_every_format() {
        let td = tempfile::tempdir().unwrap();
        if fixture(td.path()).is_none() {
            return;
        }
        for name in ["a.zip", "a.tar", "a.7z"] {
            let path = extract_single_file_temp(&td.path().join(name), "sub/a.txt").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested", "{name}");
            remove_preview_of(&path);
        }
    }

    #[test]
    fn background_extraction_sends_the_result_through_the_channel() {
        let td = tempfile::tempdir().unwrap();
        if fixture(td.path()).is_none() {
            return;
        }
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
            Ok(vec![ArchiveItem { path: "f".into(), is_dir: false, size: 1, mode: 0o644 }])
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
        if fixture(td.path()).is_none() {
            return;
        }
        for name in ["a.zip", "a.tar", "a.7z"] {
            assert!(extract_single_file_temp(&td.path().join(name), "nope.txt").is_err(), "{name}");
        }
    }

    #[test]
    fn extract_rejects_unsupported_formats_and_parent_components() {
        let td = tempfile::tempdir().unwrap();
        let err = extract_single_file_temp(&td.path().join("a.rar"), "x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        let err = extract_items_to_dir(&td.path().join("a.zip"), &["../x".to_string()], td.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn extract_items_treats_names_literally() {
        let td = tempfile::tempdir().unwrap();
        if fixture(td.path()).is_none() {
            return;
        }
        let items = vec!["-dash.txt".to_string(), "st[ar]*.txt".to_string()];
        for name in ["a.zip", "a.tar", "a.7z"] {
            let out = td.path().join(format!("out-{name}"));
            std::fs::create_dir(&out).unwrap();
            let paths = extract_items_to_dir(&td.path().join(name), &items, &out).unwrap();
            assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), "dash", "{name}");
            assert_eq!(std::fs::read_to_string(&paths[1]).unwrap(), "star", "{name}");
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2, "{name}: nothing else extracted");
        }
    }

    #[test]
    fn unzip_pattern_escapes_wildcards_and_a_leading_dash() {
        assert_eq!(_unzip_pattern("-a[1]*?"), "\\-a\\[1\\]\\*\\?");
        assert_eq!(_unzip_pattern("dir/-a"), "dir/-a");
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
