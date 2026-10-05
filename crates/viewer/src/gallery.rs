//! The gallery: the pictures and videos of one folder, in natural order.

use crate::kind::kind_of;
use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

/// Sorted files plus the index of the one on screen. Never empty until the last
/// file is removed.
#[derive(Debug)]
pub struct Gallery {
    items: Vec<PathBuf>,
    index: usize,
}

/// Compare names in natural order: digit runs by value, the rest case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ca, cb) = (_chunks(a), _chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let order = _cmp_chunk(x, y);
        if order != Ordering::Equal { return order; }
    }
    ca.len().cmp(&cb.len()).then_with(|| a.cmp(b))
}

/// Split into runs of digits and runs of anything else, lower-cased.
fn _chunks(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut last_digit = None;
    for c in s.chars() {
        let digit = c.is_ascii_digit();
        match out.last_mut() {
            Some(chunk) if last_digit == Some(digit) => chunk.extend(c.to_lowercase()),
            _ => out.push(c.to_lowercase().collect()),
        }
        last_digit = Some(digit);
    }
    out
}

/// Two numbers compare by value (no overflow: by length after the leading zeros),
/// anything else as text.
fn _cmp_chunk(x: &str, y: &str) -> Ordering {
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !(digits(x) && digits(y)) { return x.cmp(y); }
    let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
    x.len().cmp(&y.len()).then_with(|| x.cmp(y))
}

impl Gallery {
    /// Build the gallery of the folder that holds `start`. `start` is always part
    /// of it, even when its extension is not a known one.
    pub fn build(start: &Path) -> io::Result<Gallery> {
        let dir = match start.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let mut items = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden || kind_of(&path).is_none() || !path.is_file() { continue; }
            items.push(path);
        }
        // `start` may come as a relative path: compare the way read_dir joined it.
        let start = dir.join(start.file_name().unwrap_or_default());
        if !items.contains(&start) { items.push(start.clone()); }
        Ok(Gallery::from_paths(items, &start))
    }

    /// Sort `items` and put the index on `start` (the first item when absent).
    pub fn from_paths(mut items: Vec<PathBuf>, start: &Path) -> Gallery {
        items.sort_by(|a, b| _name_cmp(a, b));
        let index = items.iter().position(|p| p == start).unwrap_or(0);
        Gallery { items, index }
    }

    /// The file on screen; `None` once the gallery is empty.
    pub fn current(&self) -> Option<&Path> {
        self.items.get(self.index).map(|p| p.as_path())
    }

    /// Position of the current file, from 0.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Number of files.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when no file is left.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// All files in order.
    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }

    /// Position of `path`, if it is in the gallery.
    pub fn position(&self, path: &Path) -> Option<usize> {
        self.items.iter().position(|p| p == path)
    }

    /// Move to the next file, wrapping to the first.
    pub fn next(&mut self) {
        if self.items.is_empty() { return; }
        self.index = (self.index + 1) % self.items.len();
    }

    /// Move to the previous file, wrapping to the last.
    pub fn prev(&mut self) {
        if self.items.is_empty() { return; }
        self.index = (self.index + self.items.len() - 1) % self.items.len();
    }

    /// Go to `index`; false (nothing changes) when it is out of range.
    pub fn jump_to(&mut self, index: usize) -> bool {
        if index >= self.items.len() { return false; }
        self.index = index;
        true
    }

    /// Drop the current file. The next file takes its place, or the new last one
    /// when it was the last. Returns the removed path.
    pub fn remove_current(&mut self) -> Option<PathBuf> {
        if self.index >= self.items.len() { return None; }
        let removed = self.items.remove(self.index);
        if self.index >= self.items.len() { self.index = self.items.len().saturating_sub(1); }
        Some(removed)
    }
}

/// Natural order of two paths by file name.
fn _name_cmp(a: &Path, b: &Path) -> Ordering {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    natural_cmp(&name(a), &name(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| PathBuf::from(format!("/d/{n}"))).collect()
    }

    fn names(g: &Gallery) -> Vec<String> {
        g.items().iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect()
    }

    #[test]
    fn natural_order() {
        let g = Gallery::from_paths(paths(&["img10.png", "img2.png", "IMG1.png", "a.png"]), Path::new("/d/a.png"));
        assert_eq!(names(&g), ["a.png", "IMG1.png", "img2.png", "img10.png"]);
    }

    #[test]
    fn leading_zeros_and_huge_numbers() {
        assert_eq!(natural_cmp("a007", "a7"), Ordering::Less);
        assert_eq!(natural_cmp("a7", "a007"), Ordering::Greater);
        assert_eq!(natural_cmp("a02", "a10"), Ordering::Less);
        assert_eq!(natural_cmp("x99999999999999999999999", "x100000000000000000000000"), Ordering::Less);
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    }

    #[test]
    fn start_index_and_missing_start() {
        let g = Gallery::from_paths(paths(&["a.png", "b.png", "c.png"]), Path::new("/d/b.png"));
        assert_eq!(g.index(), 1);
        assert_eq!(g.current(), Some(Path::new("/d/b.png")));
        let g = Gallery::from_paths(paths(&["a.png", "b.png"]), Path::new("/d/zzz.png"));
        assert_eq!(g.index(), 0);
    }

    #[test]
    fn next_and_prev_wrap() {
        let mut g = Gallery::from_paths(paths(&["a.png", "b.png", "c.png"]), Path::new("/d/c.png"));
        g.next();
        assert_eq!(g.index(), 0);
        g.prev();
        assert_eq!(g.index(), 2);
    }

    #[test]
    fn single_file_stays_put() {
        let mut g = Gallery::from_paths(paths(&["a.png"]), Path::new("/d/a.png"));
        g.next();
        assert_eq!(g.index(), 0);
        g.prev();
        assert_eq!(g.index(), 0);
    }

    #[test]
    fn jump_checks_the_range() {
        let mut g = Gallery::from_paths(paths(&["a.png", "b.png"]), Path::new("/d/a.png"));
        assert!(g.jump_to(1));
        assert!(!g.jump_to(2));
        assert_eq!(g.index(), 1);
    }

    #[test]
    fn remove_at_the_start_the_middle_and_the_end() {
        let mut g = Gallery::from_paths(paths(&["a.png", "b.png", "c.png", "d.png"]), Path::new("/d/a.png"));
        assert_eq!(g.remove_current(), Some(PathBuf::from("/d/a.png")));
        assert_eq!(g.current(), Some(Path::new("/d/b.png")));
        g.jump_to(1);
        g.remove_current();
        assert_eq!(g.current(), Some(Path::new("/d/d.png")));
        g.remove_current();
        assert_eq!(g.current(), Some(Path::new("/d/b.png")));
        assert_eq!(g.index(), 0);
    }

    #[test]
    fn removing_the_last_item_empties_the_gallery() {
        let mut g = Gallery::from_paths(paths(&["a.png"]), Path::new("/d/a.png"));
        g.remove_current();
        assert!(g.is_empty());
        assert_eq!(g.current(), None);
        assert_eq!(g.remove_current(), None);
        g.next();
        g.prev();
    }

    #[test]
    fn build_filters_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["b2.png", "b10.JPG", "clip.mp4", "notes.txt", ".hidden.png", "odd.xyz"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("sub.png")).unwrap();
        let g = Gallery::build(&dir.path().join("b10.JPG")).unwrap();
        let got = names(&g);
        assert_eq!(got, ["b2.png", "b10.JPG", "clip.mp4"]);
        assert_eq!(g.current().unwrap().file_name().unwrap(), "b10.JPG");
    }

    #[test]
    fn build_always_includes_the_start_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), b"x").unwrap();
        std::fs::write(dir.path().join("odd.xyz"), b"x").unwrap();
        let g = Gallery::build(&dir.path().join("odd.xyz")).unwrap();
        assert_eq!(names(&g), ["a.png", "odd.xyz"]);
        assert_eq!(g.index(), 1);
    }

    #[test]
    fn build_reports_an_unreadable_folder() {
        assert!(Gallery::build(Path::new("/nonexistent-dir-for-test/a.png")).is_err());
    }
}
