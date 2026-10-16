//! A value loaded from a config file or directory, kept in memory and loaded
//! again only when the file (or an entry of the directory) changed on disk.
//!
//! Checking costs one `stat` per file instead of reading and parsing it, so
//! a click handler can ask for the current value every time and still see an
//! edit at once. A change is noticed through the modification time, the size
//! or the inode: the config writers save through a temp file and a rename,
//! which always gives a new inode.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// What identifies one version of one file: path, modification time, size
/// and inode.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    ino: u64,
}

/// The stamp of `path` alone, or `None` when it cannot be read.
fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileStamp { path: path.to_path_buf(), modified: meta.modified().ok(), len: meta.len(), ino: meta.ino() })
}

/// What identifies the current version of `path`: the file itself, or for a
/// directory the directory plus every entry in it (sorted). A missing path
/// has an empty stamp, so its creation is a change too.
fn stamp_of(path: &Path) -> Vec<FileStamp> {
    let Some(own) = file_stamp(path) else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(path) else { return vec![own] };
    let mut entries: Vec<FileStamp> = read.flatten().filter_map(|e| file_stamp(&e.path())).collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.insert(0, own);
    entries
}

/// A value loaded from a file or directory, reloaded only when it changed.
/// Put it in a `static Mutex<StampCache<T>>` to share it across threads.
#[derive(Debug)]
pub struct StampCache<T> {
    stamp: Vec<FileStamp>,
    value: Option<Arc<T>>,
}

impl<T> StampCache<T> {
    /// An empty cache: the first [`StampCache::get_or_load`] loads.
    pub const fn new() -> Self {
        Self { stamp: Vec::new(), value: None }
    }

    /// The value for `path`: the cached one when `path` is unchanged since
    /// the last load, else the result of `load`, which is then cached.
    pub fn get_or_load(&mut self, path: &Path, load: impl FnOnce() -> T) -> Arc<T> {
        let stamp = stamp_of(path);
        if let Some(value) = self.value.as_ref().filter(|_| stamp == self.stamp) {
            return value.clone();
        }
        let value = Arc::new(load());
        self.stamp = stamp;
        self.value = Some(value.clone());
        value
    }
}

impl<T> Default for StampCache<T> {
    /// Same as [`StampCache::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Load `path` through `cache`, counting the loads in `loads`.
    fn read(cache: &mut StampCache<String>, path: &Path, loads: &Cell<u32>) -> String {
        let value = cache.get_or_load(path, || {
            loads.set(loads.get() + 1);
            std::fs::read_to_string(path).unwrap_or_default()
        });
        (*value).clone()
    }

    #[test]
    fn an_unchanged_file_is_loaded_once() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.json");
        std::fs::write(&path, "one").unwrap();
        let (mut cache, loads) = (StampCache::new(), Cell::new(0));
        assert_eq!(read(&mut cache, &path, &loads), "one");
        assert_eq!(read(&mut cache, &path, &loads), "one");
        assert_eq!(loads.get(), 1);
    }

    #[test]
    fn a_file_replaced_through_a_rename_is_loaded_again() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.json");
        std::fs::write(&path, "one").unwrap();
        let (mut cache, loads) = (StampCache::new(), Cell::new(0));
        read(&mut cache, &path, &loads);
        // Same size, possibly the same mtime: only the inode tells.
        crate::settings::save_json(&path, "two").unwrap();
        assert_eq!(read(&mut cache, &path, &loads), "\"two\"");
        assert_eq!(loads.get(), 2);
    }

    #[test]
    fn a_file_appearing_or_disappearing_is_a_change() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.json");
        let (mut cache, loads) = (StampCache::new(), Cell::new(0));
        assert_eq!(read(&mut cache, &path, &loads), "");
        assert_eq!(read(&mut cache, &path, &loads), "");
        std::fs::write(&path, "one").unwrap();
        assert_eq!(read(&mut cache, &path, &loads), "one");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(read(&mut cache, &path, &loads), "");
        assert_eq!(loads.get(), 3);
    }

    #[test]
    fn a_directory_is_loaded_again_when_an_entry_changes() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a.json"), "1").unwrap();
        let mut cache = StampCache::new();
        let loads = Cell::new(0);
        let load = |cache: &mut StampCache<u32>| {
            cache.get_or_load(td.path(), || {
                loads.set(loads.get() + 1);
                loads.get()
            })
        };
        load(&mut cache);
        load(&mut cache);
        assert_eq!(loads.get(), 1);
        crate::settings::save_json(&td.path().join("a.json"), "22").unwrap();
        load(&mut cache);
        std::fs::write(td.path().join("b.json"), "1").unwrap();
        load(&mut cache);
        assert_eq!(loads.get(), 3);
    }
}
