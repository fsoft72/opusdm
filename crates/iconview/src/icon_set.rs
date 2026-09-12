//! Real icon image lookup, sibling of `glyphs::glyph_for`. Reuses the same
//! `IconKind` + extension classification; returns PNG bytes instead of a
//! Nerd Font `char`. No icon set is compiled into this crate - `None`
//! yields no bytes (caller falls back to the glyph) and every other set is
//! a user-imported theme loaded from disk on first use per process.

use crate::types::IconKind;
use opusdm_ipc::IconSetId;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const S_IFMT: u32 = 0o170_000;
const EXEC_BITS: u32 = 0o111;

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

/// Which fixed icon filename a `(kind, mode, ext)` combination maps to.
fn icon_filename(kind: IconKind, mode: u32, ext: Option<&str>) -> &'static str {
    match kind {
        IconKind::Dir => "dir.png",
        IconKind::Symlink => "symlink.png",
        IconKind::Broken => "broken.png",
        IconKind::File => {
            if mode & EXEC_BITS != 0 && mode & S_IFMT != 0o140_000 {
                "exec.png"
            } else {
                match ext {
                    Some(e) if ARCHIVE_EXTS.contains(&e) => "archive.png",
                    Some(e) if IMAGE_EXTS.contains(&e) => "image.png",
                    Some(e) if AUDIO_EXTS.contains(&e) => "audio.png",
                    Some(e) if VIDEO_EXTS.contains(&e) => "video.png",
                    Some(e) if CODE_EXTS.contains(&e) => "code.png",
                    Some(e) if TEXT_EXTS.contains(&e) => "text.png",
                    Some("pdf") => "pdf.png",
                    _ => "file.png",
                }
            }
        }
    }
}

/// A custom theme's loaded icon bytes, keyed by fixed filename (e.g.
/// `"dir.png"`). Only files that exist on disk are present.
type CustomIcons = HashMap<&'static str, Vec<u8>>;

static CUSTOM_CACHE: OnceLock<Mutex<HashMap<String, std::sync::Arc<CustomIcons>>>> = OnceLock::new();

const ALL_ICON_FILENAMES: [&str; 12] = [
    "dir.png", "file.png", "symlink.png", "broken.png", "exec.png", "archive.png",
    "image.png", "audio.png", "video.png", "code.png", "text.png", "pdf.png",
];

/// Load (or fetch from the process-local cache) every present icon PNG for
/// `slug`'s `icons/` folder.
fn load_custom(slug: &str) -> std::sync::Arc<CustomIcons> {
    let cache = CUSTOM_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(existing) = guard.get(slug) {
        return existing.clone();
    }
    let icons_dir = opusdm_ipc::themes::theme_dir(slug).join("icons");
    let mut loaded = CustomIcons::new();
    for name in ALL_ICON_FILENAMES {
        if let Ok(bytes) = std::fs::read(icons_dir.join(name)) {
            loaded.insert(name, bytes);
        }
    }
    let arc = std::sync::Arc::new(loaded);
    guard.insert(slug.to_string(), arc.clone());
    arc
}

/// PNG bytes for `kind`/`mode`/`ext` under icon set `set`, or `None` when
/// `set` is [`IconSetId::None`] or the theme has no file for that icon
/// (caller falls back to `glyphs::glyph_for` in both cases).
pub fn icon_bytes_for(kind: IconKind, mode: u32, ext: Option<&str>, set: &IconSetId) -> Option<Cow<'static, [u8]>> {
    let slug = match set {
        IconSetId::None => return None,
        IconSetId::Custom(slug) => slug,
    };
    let filename = icon_filename(kind, mode, ext);
    let icons = load_custom(slug);
    icons.get(filename).map(|bytes| Cow::Owned(bytes.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn none_set_returns_nothing() {
        assert!(icon_bytes_for(IconKind::Dir, 0, None, &IconSetId::None).is_none());
    }

    #[test]
    fn custom_set_returns_bytes_for_a_present_icon_and_none_for_an_absent_one() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let icons_dir = td.path().join("opusdm").join("themes").join("mytheme").join("icons");
        std::fs::create_dir_all(&icons_dir).unwrap();
        std::fs::write(icons_dir.join("dir.png"), b"fake-png-bytes").unwrap();

        let set = IconSetId::Custom("mytheme".to_string());
        let dir_icon = icon_bytes_for(IconKind::Dir, 0, None, &set);
        assert_eq!(dir_icon.as_deref(), Some(&b"fake-png-bytes"[..]));

        let file_icon = icon_bytes_for(IconKind::File, 0o100_644, None, &set);
        assert!(file_icon.is_none());
    }

    #[test]
    fn custom_set_with_no_theme_dir_returns_none_for_everything() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let set = IconSetId::Custom("does-not-exist".to_string());
        assert!(icon_bytes_for(IconKind::Dir, 0, None, &set).is_none());
    }
}
