//! LRU cache of directory listings, so revisiting a directory shows its rows
//! at once. Holds the raw rows from disk, before any Show / Hide filtering.

use crate::model::{Listing, Row};
use std::borrow::Borrow;
use std::path::{Path, PathBuf};

/// Recently read directory listings, most recent first.
pub struct DirCache {
    capacity: usize,
    entries: Vec<(PathBuf, Listing)>,
}

impl DirCache {
    /// An empty cache keeping at most `capacity` directories (0 keeps none).
    pub fn new(capacity: usize) -> Self {
        Self { capacity, entries: Vec::new() }
    }

    /// The cached rows of `dir`, marking it the most recently used. The
    /// listing is shared, not copied.
    pub fn get(&mut self, dir: &Path) -> Option<Listing> {
        let pos = self.entries.iter().position(|(p, _)| p == dir)?;
        let entry = self.entries.remove(pos);
        let rows = entry.1.clone();
        self.entries.insert(0, entry);
        Some(rows)
    }

    /// Store the rows of `dir` as the most recent entry, evicting the oldest
    /// entries beyond the capacity. With capacity 0 nothing is stored.
    pub fn put(&mut self, dir: PathBuf, rows: Listing) -> Listing {
        self.entries.retain(|(p, _)| *p != dir);
        self.entries.insert(0, (dir, rows.clone()));
        self.entries.truncate(self.capacity);
        rows
    }

    /// Drop the entry for `dir`. True when there was one.
    pub fn remove(&mut self, dir: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(p, _)| p != dir);
        self.entries.len() != before
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Change the capacity, evicting the oldest entries that no longer fit.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.entries.truncate(capacity);
    }

    /// The cached directories, most recently used first.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|(p, _)| p.clone()).collect()
    }
}

/// The fields of a row that make a listing look different on screen.
type RowKey<'a> = (&'a str, bool, bool, bool, u64, i64, u32, u32, u32);

/// Comparable summary of a row.
fn row_key(r: &Row) -> RowKey<'_> {
    (&r.name, r.is_dir, r.is_symlink, r.broken_link, r.size, r.modified_unix, r.uid, r.gid, r.mode)
}

/// True when `a` and `b` hold the same entries with the same attributes,
/// whatever order the filesystem returned them in. Either side can hold
/// shared (`Rc<Row>`) or plain rows.
pub fn same_listing<A: Borrow<Row>, B: Borrow<Row>>(a: &[A], b: &[B]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut ka: Vec<RowKey> = a.iter().map(|r| row_key(r.borrow())).collect();
    let mut kb: Vec<RowKey> = b.iter().map(|r| row_key(r.borrow())).collect();
    ka.sort();
    kb.sort();
    ka == kb
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use crate::model::{listing, RowAttrs};

    fn row(name: &str, size: u64) -> Row {
        Row::new(name.to_string(), RowAttrs { size, modified_unix: 100, mode: 0o644, ..Default::default() })
    }

    fn dir(n: &str) -> PathBuf {
        PathBuf::from(format!("/d/{n}"))
    }

    #[test]
    fn a_put_directory_can_be_fetched() {
        let mut c = DirCache::new(3);
        c.put(dir("a"), listing(vec![row("x", 1)]));
        assert_eq!(c.get(&dir("a")).unwrap().len(), 1);
        assert!(c.get(&dir("b")).is_none());
    }

    #[test]
    fn the_oldest_entry_is_evicted_past_the_capacity() {
        let mut c = DirCache::new(2);
        c.put(dir("a"), listing(vec![]));
        c.put(dir("b"), listing(vec![]));
        c.put(dir("c"), listing(vec![]));
        assert_eq!(c.paths(), vec![dir("c"), dir("b")]);
        assert!(c.get(&dir("a")).is_none());
    }

    #[test]
    fn get_refreshes_recency() {
        let mut c = DirCache::new(2);
        c.put(dir("a"), listing(vec![]));
        c.put(dir("b"), listing(vec![]));
        c.get(&dir("a"));
        c.put(dir("c"), listing(vec![]));
        assert_eq!(c.paths(), vec![dir("c"), dir("a")], "b was the least recently used");
    }

    #[test]
    fn putting_an_existing_directory_replaces_it_without_growing() {
        let mut c = DirCache::new(3);
        c.put(dir("a"), listing(vec![row("old", 1)]));
        c.put(dir("a"), listing(vec![row("new", 2), row("new2", 3)]));
        assert_eq!(c.paths().len(), 1);
        assert_eq!(c.get(&dir("a")).unwrap().len(), 2);
    }

    #[test]
    fn capacity_zero_stores_nothing() {
        let mut c = DirCache::new(0);
        let rows = c.put(dir("a"), listing(vec![row("x", 1)]));
        assert_eq!(rows.len(), 1, "the caller still gets its rows back");
        assert!(c.paths().is_empty());
        assert!(c.get(&dir("a")).is_none());
    }

    #[test]
    fn shrinking_the_capacity_evicts_the_oldest() {
        let mut c = DirCache::new(3);
        for n in ["a", "b", "c"] {
            c.put(dir(n), listing(vec![]));
        }
        c.set_capacity(1);
        assert_eq!(c.paths(), vec![dir("c")]);
        c.set_capacity(0);
        assert!(c.paths().is_empty());
    }

    #[test]
    fn remove_and_clear_report_and_empty() {
        let mut c = DirCache::new(3);
        c.put(dir("a"), listing(vec![]));
        assert!(c.remove(&dir("a")));
        assert!(!c.remove(&dir("a")));
        c.put(dir("b"), listing(vec![]));
        c.clear();
        assert!(c.paths().is_empty());
    }

    #[test]
    fn a_cache_hit_shares_the_rows_instead_of_copying_them() {
        let mut c = DirCache::new(2);
        let stored = c.put(dir("a"), listing(vec![row("x", 1), row("y", 2)]));
        let hit = c.get(&dir("a")).unwrap();
        assert!(Rc::ptr_eq(&stored, &hit), "the same listing, not a copy");
        assert!(Rc::ptr_eq(&stored[0], &hit[0]));
    }

    #[test]
    fn shared_and_plain_rows_compare_alike() {
        let shared = listing(vec![row("x", 1), row("y", 2)]);
        assert!(same_listing(&shared, &[row("y", 2), row("x", 1)]));
        assert!(!same_listing(&shared, &[row("x", 1)]));
    }

    #[test]
    fn listings_in_a_different_order_are_the_same() {
        let a = vec![row("x", 1), row("y", 2)];
        let b = vec![row("y", 2), row("x", 1)];
        assert!(same_listing(&a, &b));
    }

    #[test]
    fn a_changed_size_added_or_removed_entry_is_a_different_listing() {
        let base = vec![row("x", 1), row("y", 2)];
        assert!(!same_listing(&base, &[row("x", 1), row("y", 3)]));
        assert!(!same_listing(&base, &[row("x", 1)]));
        assert!(!same_listing(&base, &[row("x", 1), row("z", 2)]));
        assert!(same_listing::<Row, Row>(&[], &[]));
    }
}
