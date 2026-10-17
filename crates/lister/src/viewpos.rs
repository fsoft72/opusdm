//! Where the user was in each folder left behind (scroll offset, cursor and
//! selection), so Back / Forward / Parent can put them there again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Folders remembered at most; the memory starts over beyond this.
const MAX_FOLDERS: usize = 200;

/// The view of one folder at the moment it was left.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedView {
    /// Vertical scroll offset of the active view.
    pub scroll: f64,
    /// Name of the row under the keyboard cursor.
    pub cursor: Option<String>,
    /// Names of the selected rows.
    pub selected: Vec<String>,
}

/// The saved view of every folder left behind.
#[derive(Default)]
pub struct SavedViews {
    views: HashMap<PathBuf, SavedView>,
}

impl SavedViews {
    /// Remember `view` as the state of `dir`.
    pub fn put(&mut self, dir: &Path, view: SavedView) {
        if self.views.len() >= MAX_FOLDERS && !self.views.contains_key(dir) {
            self.views.clear();
        }
        self.views.insert(dir.to_path_buf(), view);
    }

    /// The state `dir` was left in, if remembered.
    pub fn get(&self, dir: &Path) -> Option<SavedView> {
        self.views.get(dir).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(scroll: f64) -> SavedView {
        SavedView { scroll, cursor: Some("a".into()), selected: vec!["a".into()] }
    }

    #[test]
    fn a_saved_view_is_returned_and_replaced() {
        let mut s = SavedViews::default();
        assert_eq!(s.get(Path::new("/a")), None);
        s.put(Path::new("/a"), view(10.0));
        s.put(Path::new("/a"), view(20.0));
        assert_eq!(s.get(Path::new("/a")), Some(view(20.0)));
    }

    #[test]
    fn the_memory_starts_over_when_full() {
        let mut s = SavedViews::default();
        for i in 0..MAX_FOLDERS {
            s.put(Path::new(&format!("/d{i}")), view(1.0));
        }
        s.put(Path::new("/d0"), view(2.0));
        assert_eq!(s.get(Path::new("/d1")), Some(view(1.0)), "an update must not clear the rest");
        s.put(Path::new("/new"), view(3.0));
        assert_eq!(s.get(Path::new("/d1")), None);
        assert_eq!(s.get(Path::new("/new")), Some(view(3.0)));
    }
}
