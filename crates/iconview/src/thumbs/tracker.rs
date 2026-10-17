//! Bookkeeping for thumbnail requests: which paths are already queued or
//! being made, and which failed. Keeps scrolling from re-submitting the same
//! request on every viewport change, and a broken file from re-running its
//! generator forever. Keyed by path, since indices move on a refresh.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Pending and failed thumbnail paths for the current generation.
#[derive(Default)]
pub struct ThumbTracker {
    pending: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
}

impl ThumbTracker {
    /// Forget everything: a new generation (another folder, another size).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.failed.clear();
    }

    /// Whether `path` should be submitted now. Returns `false` when it is
    /// already in flight or failed before; otherwise marks it pending.
    pub fn begin(&mut self, path: &Path) -> bool {
        if self.failed.contains(path) || self.pending.contains(path) {
            return false;
        }
        self.pending.insert(path.to_path_buf());
        true
    }

    /// A result for `path` arrived: it is no longer pending, and is
    /// remembered as failed when `ok` is false.
    pub fn finish(&mut self, path: &Path, ok: bool) {
        self.pending.remove(path);
        if !ok {
            self.failed.insert(path.to_path_buf());
        }
    }

    /// Keep only the failures `keep` accepts (e.g. files that did not change
    /// in a refresh, so an edited file gets another try).
    pub fn retain_failed(&mut self, keep: impl Fn(&Path) -> bool) {
        self.failed.retain(|p| keep(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_path_is_not_submitted_twice() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/a.png");
        assert!(t.begin(p));
        assert!(!t.begin(p), "second scroll event must not re-queue");
        t.finish(p, true);
        assert!(t.begin(p), "after success it may be asked again (texture evicted)");
    }

    #[test]
    fn a_failed_path_is_not_retried_until_reset() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/broken.mkv");
        assert!(t.begin(p));
        t.finish(p, false);
        assert!(!t.begin(p));
        t.reset();
        assert!(t.begin(p));
    }

    #[test]
    fn retain_failed_drops_the_rejected_failures() {
        let mut t = ThumbTracker::default();
        for p in ["/x.pdf", "/y.pdf"] {
            t.begin(Path::new(p));
            t.finish(Path::new(p), false);
        }
        t.retain_failed(|p| p == Path::new("/x.pdf"));
        assert!(!t.begin(Path::new("/x.pdf")));
        assert!(t.begin(Path::new("/y.pdf")));
    }

    #[test]
    fn reset_clears_pending_too() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/a.png");
        t.begin(p);
        t.reset();
        assert!(t.begin(p), "a stale request was skipped by the pool");
    }
}
