//! Directory sizes already calculated (F-45), kept per Lister so a size
//! survives reloads and is not computed twice. "Force" drops the entries it
//! is about to recompute.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Most entries kept; the cache restarts empty when it fills up.
const MAX_ENTRIES: usize = 10_000;

/// One directory to size: its position in the list, its name and its path.
pub type Target = (u32, String, PathBuf);

/// Calculated sizes by folder path.
#[derive(Default)]
pub struct SizeCache {
    sizes: HashMap<PathBuf, u64>,
}

impl SizeCache {
    /// The size remembered for `dir`.
    pub fn get(&self, dir: &Path) -> Option<u64> {
        self.sizes.get(dir).copied()
    }

    /// Remember the size of `dir`.
    pub fn put(&mut self, dir: PathBuf, size: u64) {
        if self.sizes.len() >= MAX_ENTRIES {
            self.sizes.clear();
        }
        self.sizes.insert(dir, size);
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.sizes.clear();
    }

    /// How many sizes are remembered.
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    /// Split `targets` into the ones whose size is known (with it) and the
    /// ones to calculate. With `force` nothing counts as known, and the old
    /// entries of the targets are dropped.
    pub fn split(&mut self, targets: Vec<Target>, force: bool) -> (Vec<(u32, String, u64)>, Vec<Target>) {
        let mut known = Vec::new();
        let mut missing = Vec::new();
        for target in targets {
            if force {
                self.sizes.remove(&target.2);
            }
            match self.get(&target.2) {
                Some(size) => known.push((target.0, target.1, size)),
                None => missing.push(target),
            }
        }
        (known, missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(i: u32, name: &str) -> Target {
        (i, name.to_string(), PathBuf::from("/d").join(name))
    }

    #[test]
    fn known_sizes_are_reused_and_the_rest_is_calculated() {
        let mut cache = SizeCache::default();
        cache.put("/d/a".into(), 10);
        let (known, missing) = cache.split(vec![target(0, "a"), target(1, "b")], false);
        assert_eq!(known, vec![(0, "a".to_string(), 10)]);
        assert_eq!(missing, vec![target(1, "b")]);
    }

    #[test]
    fn force_drops_the_old_entries_of_its_targets_only() {
        let mut cache = SizeCache::default();
        cache.put("/d/a".into(), 10);
        cache.put("/d/other".into(), 5);
        let (known, missing) = cache.split(vec![target(0, "a")], true);
        assert!(known.is_empty());
        assert_eq!(missing, vec![target(0, "a")]);
        assert_eq!(cache.get(Path::new("/d/a")), None);
        assert_eq!(cache.get(Path::new("/d/other")), Some(5));
    }

    #[test]
    fn a_full_cache_starts_over_and_clear_empties_it() {
        let mut cache = SizeCache::default();
        for i in 0..MAX_ENTRIES {
            cache.put(PathBuf::from(format!("/d/{i}")), 1);
        }
        assert_eq!(cache.len(), MAX_ENTRIES);
        cache.put("/d/new".into(), 2);
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }
}
