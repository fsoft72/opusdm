//! Browser style navigation history of one Lister.

use std::path::PathBuf;

/// Most entries kept; the oldest falls off.
pub const HISTORY_LIMIT: usize = 50;

/// The directories a Lister visited, with a position in them.
#[derive(Default)]
pub struct History {
    entries: Vec<PathBuf>,
    index: usize,
}

impl History {
    /// An empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a navigation to `dir`. Pushing the current entry again does
    /// nothing; otherwise everything after the current entry is dropped and
    /// `dir` becomes the new current entry.
    pub fn push(&mut self, dir: PathBuf) {
        if self.entries.get(self.index) == Some(&dir) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(dir);
        if self.entries.len() > HISTORY_LIMIT {
            self.entries.remove(0);
        }
        self.index = self.entries.len() - 1;
    }

    /// Step back and return the directory to go to.
    pub fn back(&mut self) -> Option<PathBuf> {
        if !self.can_back() {
            return None;
        }
        self.index -= 1;
        Some(self.entries[self.index].clone())
    }

    /// Step forward and return the directory to go to.
    pub fn forward(&mut self) -> Option<PathBuf> {
        if !self.can_forward() {
            return None;
        }
        self.index += 1;
        Some(self.entries[self.index].clone())
    }

    /// Move to entry `index` and return its directory.
    pub fn jump(&mut self, index: usize) -> Option<PathBuf> {
        let dir = self.entries.get(index)?.clone();
        self.index = index;
        Some(dir)
    }

    /// Every entry, oldest first.
    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    /// Position of the current entry, `None` when the history is empty.
    pub fn index(&self) -> Option<usize> {
        if self.entries.is_empty() { None } else { Some(self.index) }
    }

    /// True when there is an older entry.
    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    /// True when there is a newer entry.
    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn visited(names: &[&str]) -> History {
        let mut h = History::new();
        for n in names {
            h.push(p(n));
        }
        h
    }

    #[test]
    fn an_empty_history_goes_nowhere() {
        let mut h = History::new();
        assert_eq!(h.index(), None);
        assert!(!h.can_back() && !h.can_forward());
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), None);
        assert_eq!(h.jump(0), None);
    }

    #[test]
    fn back_and_forward_walk_the_stack() {
        let mut h = visited(&["/a", "/b", "/c"]);
        assert_eq!(h.index(), Some(2));
        assert_eq!(h.back(), Some(p("/b")));
        assert_eq!(h.back(), Some(p("/a")));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some(p("/b")));
        assert!(h.can_back() && h.can_forward());
    }

    #[test]
    fn pushing_the_current_directory_again_does_nothing() {
        let mut h = visited(&["/a", "/b"]);
        h.push(p("/b"));
        assert_eq!(h.entries().len(), 2);
    }

    #[test]
    fn navigating_back_then_to_the_same_entry_keeps_the_forward_part() {
        let mut h = visited(&["/a", "/b", "/c"]);
        h.back();
        h.push(p("/b"));
        assert!(h.can_forward(), "going back and landing on /b must not erase /c");
        assert_eq!(h.entries().len(), 3);
    }

    #[test]
    fn a_new_directory_drops_the_forward_part() {
        let mut h = visited(&["/a", "/b", "/c"]);
        h.back();
        h.push(p("/x"));
        assert_eq!(h.entries(), &[p("/a"), p("/b"), p("/x")]);
        assert!(!h.can_forward());
    }

    #[test]
    fn jump_moves_to_an_entry() {
        let mut h = visited(&["/a", "/b", "/c"]);
        assert_eq!(h.jump(0), Some(p("/a")));
        assert_eq!(h.index(), Some(0));
        assert_eq!(h.jump(9), None);
        assert_eq!(h.index(), Some(0));
    }

    #[test]
    fn the_oldest_entry_falls_off_at_the_limit() {
        let mut h = History::new();
        for i in 0..(HISTORY_LIMIT + 5) {
            h.push(p(&format!("/d{i}")));
        }
        assert_eq!(h.entries().len(), HISTORY_LIMIT);
        assert_eq!(h.entries()[0], p("/d5"));
        assert_eq!(h.index(), Some(HISTORY_LIMIT - 1));
    }
}
