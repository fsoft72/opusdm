//! Marks for the image slideshow (F-17).

use std::path::{Path, PathBuf};

/// The pictures marked for deletion, in marking order.
#[derive(Default)]
pub struct Marks {
    files: Vec<PathBuf>,
}

impl Marks {
    /// No marks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `path`, or unmark it if it was marked. True when it is now marked.
    pub fn toggle(&mut self, path: PathBuf) -> bool {
        if let Some(pos) = self.files.iter().position(|p| *p == path) {
            self.files.remove(pos);
            return false;
        }
        self.files.push(path);
        true
    }

    /// Whether `path` is marked.
    pub fn is_marked(&self, path: &Path) -> bool {
        self.files.iter().any(|p| p == path)
    }

    /// The marked files, in marking order.
    pub fn list(&self) -> Vec<PathBuf> {
        self.files.clone()
    }

    /// How many files are marked.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// True when nothing is marked.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn toggling_marks_then_unmarks() {
        let mut m = Marks::new();
        assert!(m.toggle(PathBuf::from("/a")));
        assert!(m.is_marked(Path::new("/a")));
        assert!(!m.toggle(PathBuf::from("/a")));
        assert!(!m.is_marked(Path::new("/a")));
        assert!(m.is_empty());
    }

    #[test]
    fn the_list_keeps_marking_order_and_drops_unmarked_files() {
        let mut m = Marks::new();
        m.toggle(PathBuf::from("/c"));
        m.toggle(PathBuf::from("/a"));
        m.toggle(PathBuf::from("/b"));
        m.toggle(PathBuf::from("/a"));
        assert_eq!(m.list(), vec![PathBuf::from("/c"), PathBuf::from("/b")]);
        assert_eq!(m.len(), 2);
    }
}
