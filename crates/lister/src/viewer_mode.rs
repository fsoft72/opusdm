//! Viewer modes, SmartRead and the file list.

use crate::preview::{kind_of, FileKind, IMAGE_EXTS};
use std::path::{Path, PathBuf};

/// How many leading bytes SmartRead looks at.
pub const SNIFF_LEN: usize = 4096;

/// Extensions that mean ANSI art whatever the content.
const ANSI_EXTS: [&str; 3] = ["ans", "diz", "asc"];

/// What the user asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Pick by extension and content.
    Smart,
    Text,
    Ansi,
    Hex,
}

/// What a file is actually shown as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    Image,
    Text,
    Ansi,
    Hex,
}

/// True when `head` holds a CSI escape sequence (`ESC [`).
fn has_csi(head: &[u8]) -> bool {
    head.windows(2).any(|w| w == b"\x1b[")
}

/// Decide how to show a file. Explicit modes are obeyed; `Smart` chooses
/// picture, ANSI (art extension or an `ESC [` in the first `SNIFF_LEN` bytes,
/// checked before the text probe because CP437 art is not valid UTF-8), text
/// or hex.
pub fn resolve(mode: Mode, ext: &str, head: &[u8]) -> Resolved {
    match mode {
        Mode::Text => Resolved::Text,
        Mode::Ansi => Resolved::Ansi,
        Mode::Hex => Resolved::Hex,
        Mode::Smart => {
            let ext = ext.to_lowercase();
            let head = &head[..head.len().min(SNIFF_LEN)];
            if IMAGE_EXTS.contains(&ext.as_str()) {
                Resolved::Image
            } else if ANSI_EXTS.contains(&ext.as_str()) || has_csi(head) {
                Resolved::Ansi
            } else if kind_of(&ext, head) == FileKind::Text {
                Resolved::Text
            } else {
                Resolved::Hex
            }
        }
    }
}

/// The files the Viewer pages through: the selection without directories, or
/// the file under the cursor when nothing is selected.
pub fn viewer_files(selected: &[PathBuf], cursor: Option<&Path>) -> Vec<PathBuf> {
    if !selected.is_empty() {
        return selected.iter().filter(|p| !p.is_dir()).cloned().collect();
    }
    cursor.filter(|p| !p.is_dir()).map(|p| vec![p.to_path_buf()]).unwrap_or_default()
}

/// The pictures among `files`, in order.
pub fn image_files(files: &[PathBuf]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            IMAGE_EXTS.contains(&ext.as_str())
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ESC_HEAD: &[u8] = b"hello \x1b[31mred\x1b[0m";

    #[test]
    fn explicit_modes_resolve_to_themselves() {
        assert_eq!(resolve(Mode::Text, "png", b"x"), Resolved::Text);
        assert_eq!(resolve(Mode::Ansi, "txt", b"x"), Resolved::Ansi);
        assert_eq!(resolve(Mode::Hex, "txt", b"x"), Resolved::Hex);
    }

    #[test]
    fn smart_picks_images_by_extension() {
        assert_eq!(resolve(Mode::Smart, "PNG", b""), Resolved::Image);
        assert_eq!(resolve(Mode::Smart, "jpg", b"\xff\xd8"), Resolved::Image);
    }

    #[test]
    fn smart_picks_ansi_for_art_extensions_and_escape_sequences() {
        assert_eq!(resolve(Mode::Smart, "ans", b"\xc9\xcd\xbb"), Resolved::Ansi);
        assert_eq!(resolve(Mode::Smart, "diz", b"plain"), Resolved::Ansi);
        assert_eq!(resolve(Mode::Smart, "asc", b"plain"), Resolved::Ansi);
        assert_eq!(resolve(Mode::Smart, "txt", ESC_HEAD), Resolved::Ansi);
        assert_eq!(resolve(Mode::Smart, "weird", ESC_HEAD), Resolved::Ansi);
    }

    #[test]
    fn cp437_art_with_escapes_is_ansi_not_hex_even_though_it_is_not_utf8() {
        let head = [&[0xC9u8, 0xCD, 0xBB][..], b"\x1b[31m"].concat();
        assert_eq!(resolve(Mode::Smart, "bin", &head), Resolved::Ansi);
    }

    #[test]
    fn an_escape_sequence_after_the_sniff_window_does_not_count() {
        let mut head = vec![b'a'; SNIFF_LEN];
        head.extend_from_slice(b"\x1b[31m");
        assert_eq!(resolve(Mode::Smart, "weird", &head), Resolved::Text);
    }

    #[test]
    fn smart_picks_text_or_hex_by_content() {
        assert_eq!(resolve(Mode::Smart, "txt", b"words"), Resolved::Text);
        assert_eq!(resolve(Mode::Smart, "weird", b"words\n"), Resolved::Text);
        assert_eq!(resolve(Mode::Smart, "bin", b"a\x00b"), Resolved::Hex);
        assert_eq!(resolve(Mode::Smart, "zip", b"PK\x03\x04"), Resolved::Hex);
    }

    #[test]
    fn viewer_files_uses_the_selection_and_skips_directories() {
        let td = tempfile::tempdir().unwrap();
        let (a, b) = (td.path().join("a.txt"), td.path().join("b.txt"));
        std::fs::write(&a, "x").unwrap();
        std::fs::write(&b, "y").unwrap();
        let d = td.path().join("sub");
        std::fs::create_dir(&d).unwrap();
        let got = viewer_files(&[a.clone(), d, b.clone()], Some(&b));
        assert_eq!(got, vec![a, b], "the cursor is ignored when something is selected");
    }

    #[test]
    fn viewer_files_falls_back_to_the_cursor_file() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("f.txt");
        std::fs::write(&f, "x").unwrap();
        assert_eq!(viewer_files(&[], Some(&f)), vec![f.clone()]);
        assert!(viewer_files(&[], Some(td.path())).is_empty(), "a directory under the cursor is not viewable");
        assert!(viewer_files(&[], None).is_empty());
    }

    #[test]
    fn image_files_keeps_only_pictures_in_order() {
        let files: Vec<PathBuf> = ["/a/one.PNG", "/a/notes.txt", "/a/two.jpg"].iter().map(PathBuf::from).collect();
        assert_eq!(image_files(&files), vec![PathBuf::from("/a/one.PNG"), PathBuf::from("/a/two.jpg")]);
    }
}
