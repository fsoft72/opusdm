//! Loading a file for the Viewer, off the GTK thread, into plain data.

use crate::ansi::{parse_ansi, AnsiDoc};
use crate::model::format_size;
use crate::preview::generate_hex_dump;
use crate::viewer_mode::{resolve, Mode, Resolved, SNIFF_LEN};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Most bytes of a file shown in text or ANSI mode.
pub const VIEWER_TEXT_MAX: u64 = 16 * 1024 * 1024;

/// Most bytes of a file shown in hex mode.
pub const VIEWER_HEX_MAX: u64 = 1024 * 1024;

/// A file ready to render; plain data, so it can cross from the loader thread.
#[derive(Debug)]
pub enum ViewData {
    Text { meta: String, text: String },
    Ansi { meta: String, doc: AnsiDoc },
    Hex { meta: String, text: String },
    Image { bytes: Vec<u8>, meta: String },
    Error { message: String },
}

/// The first `cap` bytes of `path`.
fn read_prefix(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    File::open(path)?.take(cap).read_to_end(&mut buf)?;
    Ok(buf)
}

/// " - showing the first N" when the file is bigger than the cap, else empty.
fn truncation_note(size: u64, cap: u64) -> String {
    if size > cap { format!("  \u{2022}  showing the first {}", format_size(false, cap)) } else { String::new() }
}

/// Load `path` for the Viewer with the standard caps.
pub fn load(path: &Path, mode: Mode) -> ViewData {
    load_with_caps(path, mode, VIEWER_TEXT_MAX, VIEWER_HEX_MAX)
}

/// Load `path`, reading at most `text_cap` bytes for text and ANSI and
/// `hex_cap` for hex. Blocking: call it off the GTK thread.
pub fn load_with_caps(path: &Path, mode: Mode, text_cap: u64, hex_cap: u64) -> ViewData {
    let error = |e: io::Error| ViewData::Error { message: format!("Cannot open: {e}") };
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => return error(e),
    };
    let head = match read_prefix(path, SNIFF_LEN as u64) {
        Ok(h) => h,
        Err(e) => return error(e),
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let size_str = format_size(false, size);
    match resolve(mode, &ext, &head) {
        Resolved::Image => match std::fs::read(path) {
            Ok(bytes) => ViewData::Image { bytes, meta: format!("{size_str}  \u{2022}  {}", ext.to_uppercase()) },
            Err(e) => error(e),
        },
        Resolved::Text => match read_prefix(path, text_cap) {
            Ok(buf) => {
                let text = String::from_utf8_lossy(&buf).into_owned();
                let meta = format!("{} lines  \u{2022}  {size_str}{}", text.lines().count(), truncation_note(size, text_cap));
                ViewData::Text { meta, text }
            }
            Err(e) => error(e),
        },
        Resolved::Ansi => match read_prefix(path, text_cap) {
            Ok(buf) => {
                let meta = format!("ANSI  \u{2022}  {size_str}{}", truncation_note(size, text_cap));
                ViewData::Ansi { meta, doc: parse_ansi(&buf) }
            }
            Err(e) => error(e),
        },
        Resolved::Hex => match read_prefix(path, hex_cap) {
            Ok(buf) => {
                let meta = format!("Hex  \u{2022}  {size_str}{}", truncation_note(size, hex_cap));
                ViewData::Hex { meta, text: generate_hex_dump(&buf) }
            }
            Err(e) => error(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: u64 = 1024;

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn text_loads_whole_with_a_line_count() {
        let td = tempfile::tempdir().unwrap();
        let p = write(&td, "n.txt", b"one\ntwo\nthree\n");
        match load(&p, Mode::Smart) {
            ViewData::Text { meta, text } => {
                assert_eq!(text, "one\ntwo\nthree\n");
                assert!(meta.contains("3 lines"), "{meta}");
                assert!(!meta.contains("showing the first"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn a_file_larger_than_the_cap_is_truncated_with_a_notice() {
        let td = tempfile::tempdir().unwrap();
        let p = write(&td, "big.txt", &vec![b'a'; 3000]);
        match load_with_caps(&p, Mode::Text, SMALL, SMALL) {
            ViewData::Text { meta, text } => {
                assert_eq!(text.len(), SMALL as usize);
                assert!(meta.contains("showing the first"), "{meta}");
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn a_file_exactly_at_the_cap_has_no_notice() {
        let td = tempfile::tempdir().unwrap();
        let p = write(&td, "exact.txt", &vec![b'a'; SMALL as usize]);
        match load_with_caps(&p, Mode::Text, SMALL, SMALL) {
            ViewData::Text { meta, .. } => assert!(!meta.contains("showing the first"), "{meta}"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn invalid_utf8_under_text_mode_is_decoded_lossily() {
        let td = tempfile::tempdir().unwrap();
        let p = write(&td, "l1.txt", b"caf\xe9\n");
        match load(&p, Mode::Text) {
            ViewData::Text { text, .. } => assert!(text.contains('\u{FFFD}')),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn smart_read_of_ansi_art_returns_coloured_spans() {
        let td = tempfile::tempdir().unwrap();
        let p = write(&td, "art.ans", b"\x1b[1;31m\xc9\xcd\xbb\x1b[0m\n");
        match load(&p, Mode::Smart) {
            ViewData::Ansi { doc, .. } => {
                assert_eq!(doc.text, "\u{2554}\u{2550}\u{2557}\n");
                assert_eq!(doc.spans[0].fg, Some(9));
            }
            other => panic!("expected Ansi, got {other:?}"),
        }
    }

    #[test]
    fn hex_mode_dumps_and_caps_the_bytes() {
        let td = tempfile::tempdir().unwrap();
        let p = write(&td, "b.bin", &[0u8, 1, 2, 3, 255]);
        match load(&p, Mode::Smart) {
            ViewData::Hex { text, .. } => assert!(text.contains("00 01 02 03 ff")),
            other => panic!("expected Hex, got {other:?}"),
        }
        let big = write(&td, "big.bin", &vec![7u8; 3000]);
        match load_with_caps(&big, Mode::Hex, SMALL, 32) {
            ViewData::Hex { meta, text } => {
                assert_eq!(text.lines().count(), 2, "32 bytes make two dump lines");
                assert!(meta.contains("showing the first"), "{meta}");
            }
            other => panic!("expected Hex, got {other:?}"),
        }
    }

    #[test]
    fn an_image_returns_its_bytes() {
        let td = tempfile::tempdir().unwrap();
        let p = write(&td, "p.png", b"not really a png");
        match load(&p, Mode::Smart) {
            ViewData::Image { bytes, meta } => {
                assert_eq!(bytes, b"not really a png");
                assert!(meta.contains("PNG"), "{meta}");
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_file_is_an_error() {
        match load(std::path::Path::new("/nonexistent/opusdm-viewer/x.txt"), Mode::Smart) {
            ViewData::Error { message } => assert!(message.contains("Cannot open")),
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
