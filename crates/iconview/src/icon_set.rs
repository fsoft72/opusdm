//! Real icon image lookup, sibling of `glyphs::glyph_for`. Reuses the same
//! `IconKind` + extension classification; returns PNG bytes instead of a
//! Nerd Font `char`.

use crate::types::IconKind;
use opusdm_ipc::IconSetId;

const S_IFMT: u32 = 0o170_000;
const EXEC_BITS: u32 = 0o111;

const BREEZE_DIR: &[u8] = include_bytes!("../assets/breeze-dark/dir.png");
const BREEZE_FILE: &[u8] = include_bytes!("../assets/breeze-dark/file.png");
const BREEZE_SYMLINK: &[u8] = include_bytes!("../assets/breeze-dark/symlink.png");
const BREEZE_BROKEN: &[u8] = include_bytes!("../assets/breeze-dark/broken.png");
const BREEZE_EXEC: &[u8] = include_bytes!("../assets/breeze-dark/exec.png");
const BREEZE_ARCHIVE: &[u8] = include_bytes!("../assets/breeze-dark/archive.png");
const BREEZE_IMAGE: &[u8] = include_bytes!("../assets/breeze-dark/image.png");
const BREEZE_AUDIO: &[u8] = include_bytes!("../assets/breeze-dark/audio.png");
const BREEZE_VIDEO: &[u8] = include_bytes!("../assets/breeze-dark/video.png");
const BREEZE_CODE: &[u8] = include_bytes!("../assets/breeze-dark/code.png");
const BREEZE_TEXT: &[u8] = include_bytes!("../assets/breeze-dark/text.png");
const BREEZE_PDF: &[u8] = include_bytes!("../assets/breeze-dark/pdf.png");

const ARCHIVE_EXTS: &[&str] = &["zip", "tar", "gz", "bz2", "xz", "7z", "rar"];
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "svg", "webp"];
const AUDIO_EXTS: &[&str] = &["mp3", "flac", "ogg", "opus", "wav", "m4a"];
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "avi", "mov", "webm"];
const CODE_EXTS: &[&str] = &[
    "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "go", "java", "kt", "rb", "php", "c", "h",
    "cpp", "cc", "hpp", "html", "htm", "css", "scss", "json", "toml", "yaml", "yml", "ini",
    "conf",
];
const TEXT_EXTS: &[&str] = &["md", "txt"];

/// PNG bytes for `kind`/`mode`/`ext` under icon set `set`, or `None` for
/// [`IconSetId::None`] (caller falls back to `glyphs::glyph_for`).
pub fn icon_bytes_for(kind: IconKind, mode: u32, ext: Option<&str>, set: IconSetId) -> Option<&'static [u8]> {
    if set != IconSetId::BreezeDark {
        return None;
    }
    Some(match kind {
        IconKind::Dir => BREEZE_DIR,
        IconKind::Symlink => BREEZE_SYMLINK,
        IconKind::Broken => BREEZE_BROKEN,
        IconKind::File => {
            if mode & EXEC_BITS != 0 && mode & S_IFMT != 0o140_000 {
                BREEZE_EXEC
            } else {
                match ext {
                    Some(e) if ARCHIVE_EXTS.contains(&e) => BREEZE_ARCHIVE,
                    Some(e) if IMAGE_EXTS.contains(&e) => BREEZE_IMAGE,
                    Some(e) if AUDIO_EXTS.contains(&e) => BREEZE_AUDIO,
                    Some(e) if VIDEO_EXTS.contains(&e) => BREEZE_VIDEO,
                    Some(e) if CODE_EXTS.contains(&e) => BREEZE_CODE,
                    Some(e) if TEXT_EXTS.contains(&e) => BREEZE_TEXT,
                    Some("pdf") => BREEZE_PDF,
                    _ => BREEZE_FILE,
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_set_returns_nothing() {
        assert_eq!(icon_bytes_for(IconKind::Dir, 0, None, IconSetId::None), None);
    }

    #[test]
    fn breeze_dark_covers_every_kind() {
        assert_eq!(icon_bytes_for(IconKind::Dir, 0, None, IconSetId::BreezeDark), Some(BREEZE_DIR));
        assert_eq!(icon_bytes_for(IconKind::Symlink, 0, None, IconSetId::BreezeDark), Some(BREEZE_SYMLINK));
        assert_eq!(icon_bytes_for(IconKind::Broken, 0, None, IconSetId::BreezeDark), Some(BREEZE_BROKEN));
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, None, IconSetId::BreezeDark),
            Some(BREEZE_FILE)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_755, None, IconSetId::BreezeDark),
            Some(BREEZE_EXEC)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("rs"), IconSetId::BreezeDark),
            Some(BREEZE_CODE)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("png"), IconSetId::BreezeDark),
            Some(BREEZE_IMAGE)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("pdf"), IconSetId::BreezeDark),
            Some(BREEZE_PDF)
        );
        assert_eq!(
            icon_bytes_for(IconKind::File, 0o100_644, Some("zip"), IconSetId::BreezeDark),
            Some(BREEZE_ARCHIVE)
        );
    }
}
