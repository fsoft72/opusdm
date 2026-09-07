//! Plain data types shared across the crate and its consumers. No GTK.

use std::path::PathBuf;

/// What a directory entry is, for icon and glyph selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Dir,
    File,
    Symlink,
    Broken,
}

/// How `IconField` arranges its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Grid,
    Free,
}

/// Freedesktop thumbnail bucket. Used in phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    Normal,
    Large,
}

/// One entry shown by `IconField`. Built by the consumer, never mutated here.
#[derive(Debug, Clone)]
pub struct IconEntry {
    /// File name, no directory part.
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
    /// Entry kind.
    pub kind: IconKind,
    /// Unix mode bits (for exec / socket / fifo glyphs).
    pub mode: u32,
    /// Lowercase final extension, if any.
    pub ext: Option<String>,
}

impl IconEntry {
    /// The lowercase final extension of `name`, or `None`. A leading dot does
    /// not start an extension, so `.bashrc` has none.
    pub fn ext_of(name: &str) -> Option<String> {
        let stem = name.strip_prefix('.').unwrap_or(name);
        let (_, ext) = stem.rsplit_once('.')?;
        if ext.is_empty() {
            return None;
        }
        Some(ext.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_of_rules() {
        assert_eq!(IconEntry::ext_of("main.RS").as_deref(), Some("rs"));
        assert_eq!(IconEntry::ext_of("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(IconEntry::ext_of(".bashrc"), None);
        assert_eq!(IconEntry::ext_of("Makefile"), None);
        assert_eq!(IconEntry::ext_of("trailing."), None);
    }
}
