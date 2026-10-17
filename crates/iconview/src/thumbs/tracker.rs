//! Bookkeeping for thumbnail requests: which paths are already queued or
//! being made, and which failed. Keeps scrolling from re-submitting the same
//! request on every viewport change, and a broken file from re-running its
//! generator forever. Keyed by path, since indices move on a refresh. A
//! pending request remembers the modification time it was made for, so a
//! file edited while its thumbnail is being made gets a new request.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Pending and failed thumbnail paths for the current generation.
#[derive(Default)]
pub struct ThumbTracker {
    /// Path in flight, with the modification time it was asked for.
    pending: HashMap<PathBuf, i64>,
    failed: HashSet<PathBuf>,
}

impl ThumbTracker {
    /// Forget everything: a new generation (another folder, another size).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.failed.clear();
    }

    /// Whether `path`, modified at `modified`, should be submitted now.
    /// Returns `false` when it failed before or the same version is already
    /// in flight; otherwise marks it pending. A request in flight for another
    /// modification time (the file was edited) does not block a new one.
    pub fn begin(&mut self, path: &Path, modified: i64) -> bool {
        if self.failed.contains(path) || self.pending.get(path) == Some(&modified) {
            return false;
        }
        self.pending.insert(path.to_path_buf(), modified);
        true
    }

    /// A result for `path` made for `modified` arrived. Returns whether it is
    /// the result of the latest request: a stale one (the file was edited
    /// and asked for again meanwhile) must not be shown, and its failure is
    /// not remembered. A current result is no longer pending, and is
    /// remembered as failed when `ok` is false.
    pub fn finish(&mut self, path: &Path, modified: i64, ok: bool) -> bool {
        if self.pending.get(path).is_some_and(|&asked| asked != modified) {
            return false;
        }
        self.pending.remove(path);
        if !ok {
            self.failed.insert(path.to_path_buf());
        }
        true
    }

    /// Keep only the failures `keep` accepts (e.g. files that did not change
    /// in a refresh, so an edited file gets another try).
    pub fn retain_failed(&mut self, keep: impl Fn(&Path) -> bool) {
        self.failed.retain(|p| keep(p));
    }

    /// Forget every failure, so each path may be asked for again (a reload
    /// asked by the user: a missing or busy tool may work now).
    pub fn forget_failed(&mut self) {
        self.failed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_path_is_not_submitted_twice() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/a.png");
        assert!(t.begin(p, 0));
        assert!(!t.begin(p, 0), "second scroll event must not re-queue");
        t.finish(p, 0, true);
        assert!(t.begin(p, 0), "after success it may be asked again (texture evicted)");
    }

    #[test]
    fn a_failed_path_is_not_retried_until_reset() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/broken.mkv");
        assert!(t.begin(p, 0));
        t.finish(p, 0, false);
        assert!(!t.begin(p, 0));
        t.reset();
        assert!(t.begin(p, 0));
    }

    #[test]
    fn retain_failed_drops_the_rejected_failures() {
        let mut t = ThumbTracker::default();
        for p in ["/x.pdf", "/y.pdf"] {
            t.begin(Path::new(p), 0);
            t.finish(Path::new(p), 0, false);
        }
        t.retain_failed(|p| p == Path::new("/x.pdf"));
        assert!(!t.begin(Path::new("/x.pdf"), 0));
        assert!(t.begin(Path::new("/y.pdf"), 0));
    }

    #[test]
    fn forget_failed_lets_every_failure_retry_but_keeps_pending() {
        let mut t = ThumbTracker::default();
        t.begin(Path::new("/x.pdf"), 0);
        t.finish(Path::new("/x.pdf"), 0, false);
        t.begin(Path::new("/busy.mkv"), 0);
        t.forget_failed();
        assert!(t.begin(Path::new("/x.pdf"), 0));
        assert!(!t.begin(Path::new("/busy.mkv"), 0), "a request in flight is not queued twice");
    }

    #[test]
    fn a_file_edited_while_pending_is_asked_again_and_the_old_result_is_stale() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/a.png");
        assert!(t.begin(p, 10));
        assert!(t.begin(p, 11), "the edited file needs its own request");
        assert!(!t.begin(p, 11), "but only one per version");
        assert!(!t.finish(p, 10, true), "the result for the old version is stale");
        assert!(!t.begin(p, 11), "the new request is still in flight");
        assert!(t.finish(p, 11, true));
        assert!(t.begin(p, 11), "after the result it may be asked again");
    }

    #[test]
    fn a_stale_failure_is_not_remembered() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/half-written.png");
        t.begin(p, 10);
        t.begin(p, 11);
        assert!(!t.finish(p, 10, false));
        assert!(t.finish(p, 11, true));
        assert!(t.begin(p, 11), "the old failure did not mark the file as broken");
    }

    #[test]
    fn reset_clears_pending_too() {
        let mut t = ThumbTracker::default();
        let p = Path::new("/a.png");
        t.begin(p, 0);
        t.reset();
        assert!(t.begin(p, 0), "a stale request was skipped by the pool");
    }
}
