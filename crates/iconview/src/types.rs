//! Plain data types shared across the crate and its consumers. No GTK.

use std::path::PathBuf;

/// Icon pixel sizes selectable via Ctrl+scroll, smallest to largest.
pub const ICON_ZOOM_STEPS: [i32; 4] = [48, 64, 96, 128];

/// The icon size a new `IconField` and a fresh Lister window start at.
/// Must be one of `ICON_ZOOM_STEPS`.
pub const DEFAULT_ICON_SIZE: i32 = 64;

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

/// Freedesktop thumbnail bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    Normal,
    Large,
}

impl ThumbSize {
    /// The square pixel size of this bucket.
    pub fn px(self) -> i32 {
        match self {
            ThumbSize::Normal => 128,
            ThumbSize::Large => 256,
        }
    }

    /// The freedesktop cache subdirectory name.
    pub fn dir_name(self) -> &'static str {
        match self {
            ThumbSize::Normal => "normal",
            ThumbSize::Large => "large",
        }
    }

    /// The bucket to use for an icon drawn `icon_px` wide.
    pub fn for_icon(icon_px: i32) -> ThumbSize {
        if icon_px <= 128 {
            ThumbSize::Normal
        } else {
            ThumbSize::Large
        }
    }
}

/// One entry shown by `IconField`. Built by the consumer, never mutated here.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Whether this entry is a "left out" desktop icon (drawn with an arrow
    /// badge, position pinned by path).
    pub left_out: bool,
    /// Whether this entry is a program group icon (drawn with a group badge,
    /// position pinned by its virtual `group:<id>` path).
    pub group: bool,
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
    fn thumb_size_mapping() {
        assert_eq!(ThumbSize::Normal.px(), 128);
        assert_eq!(ThumbSize::Large.px(), 256);
        assert_eq!(ThumbSize::Normal.dir_name(), "normal");
        assert_eq!(ThumbSize::Large.dir_name(), "large");
        assert_eq!(ThumbSize::for_icon(64), ThumbSize::Normal);
        assert_eq!(ThumbSize::for_icon(128), ThumbSize::Normal);
        assert_eq!(ThumbSize::for_icon(129), ThumbSize::Large);
    }

    #[test]
    fn ext_of_rules() {
        assert_eq!(IconEntry::ext_of("main.RS").as_deref(), Some("rs"));
        assert_eq!(IconEntry::ext_of("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(IconEntry::ext_of(".bashrc"), None);
        assert_eq!(IconEntry::ext_of("Makefile"), None);
        assert_eq!(IconEntry::ext_of("trailing."), None);
    }
}
