//! Runtime registration of the font files shipped inside a theme, for the
//! current process only (nothing is installed system-wide). Uses fontconfig
//! directly because gtk4-rs does not expose it; pango, already linked by GTK,
//! sits on top of the same fontconfig.

use std::collections::HashSet;
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[link(name = "fontconfig")]
extern "C" {
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const c_char) -> c_int;
}

#[link(name = "pangocairo-1.0")]
extern "C" {
    fn pango_cairo_font_map_get_default() -> *mut c_void;
}

#[link(name = "pangoft2-1.0")]
extern "C" {
    fn pango_fc_font_map_config_changed(fontmap: *mut c_void);
}

/// Font files seen so far in this process: accepted ones (so a repaint does
/// not add the same file twice) and rejected ones (so a bad file is not
/// retried and re-logged on every restyle).
#[derive(Default)]
struct Registry {
    accepted: HashSet<PathBuf>,
    rejected: HashSet<PathBuf>,
}

static REGISTERED: Mutex<Option<Registry>> = Mutex::new(None);

/// The `.ttf` / `.otf` files directly inside `dir`, sorted by name. A missing
/// or unreadable folder yields an empty list.
pub fn font_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
        })
        .collect();
    files.sort();
    files
}

/// Hand one file to the default fontconfig configuration. True when
/// fontconfig accepted it.
fn register_font_file(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.to_string_lossy().as_bytes()) else { return false };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the
    // call; a null config means "the current configuration".
    unsafe { FcConfigAppFontAddFile(std::ptr::null_mut(), c_path.as_ptr().cast()) != 0 }
}

/// Register every font file directly inside `dir` with this process. Returns
/// how many files were newly accepted; files already seen (accepted or
/// rejected) are skipped, and rejected ones are logged once.
pub(crate) fn register_font_dir(dir: &Path) -> usize {
    let mut guard = REGISTERED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let reg = guard.get_or_insert_with(Registry::default);
    let mut added = 0;
    for file in font_files(dir) {
        if reg.accepted.contains(&file) || reg.rejected.contains(&file) {
            continue;
        }
        if register_font_file(&file) {
            reg.accepted.insert(file);
            added += 1;
        } else {
            opusdm_ipc::err_log!("ui", "cannot register font file {}", file.display());
            reg.rejected.insert(file);
        }
    }
    added
}

/// Register every font file of theme `slug` (its `fonts/` folder) with this
/// process and tell pango to re-read the configuration. Returns how many
/// files were newly registered; unreadable files are logged and skipped.
pub fn register_theme_fonts(slug: &str) -> usize {
    let added = register_font_dir(&opusdm_ipc::themes::theme_dir(slug).join("fonts"));
    if added > 0 {
        // SAFETY: the default pangocairo font map is a fontconfig map on
        // Linux; a null map is skipped.
        unsafe {
            let map = pango_cairo_font_map_get_default();
            if !map.is_null() {
                pango_fc_font_map_config_changed(map);
            }
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_files_lists_only_ttf_and_otf_sorted() {
        let td = tempfile::tempdir().unwrap();
        for n in ["b.OTF", "a.ttf", "c.txt", "d.woff"] {
            std::fs::write(td.path().join(n), b"x").unwrap();
        }
        std::fs::create_dir(td.path().join("sub.ttf")).unwrap();
        let names: Vec<_> = font_files(td.path()).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["a.ttf", "b.OTF"]);
    }

    #[test]
    fn a_missing_folder_has_no_fonts() {
        assert!(font_files(std::path::Path::new("/no/such/dir")).is_empty());
    }

    #[test]
    fn a_broken_font_file_is_not_counted_as_registered() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("bad.ttf"), b"not a font").unwrap();
        assert_eq!(register_font_dir(td.path()), 0);
        // Rejected files are remembered: still 0, no retry.
        assert_eq!(register_font_dir(td.path()), 0);
    }

    /// First system font file found under /usr/share/fonts, if any.
    fn system_font() -> Option<PathBuf> {
        let mut stack = vec![PathBuf::from("/usr/share/fonts")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for p in entries.flatten().map(|e| e.path()) {
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf")) {
                    return Some(p);
                }
            }
        }
        None
    }

    #[test]
    fn a_good_font_file_is_registered_once() {
        let Some(font) = system_font() else {
            eprintln!("skipped: no system font found");
            return;
        };
        let td = tempfile::tempdir().unwrap();
        std::fs::copy(&font, td.path().join(font.file_name().unwrap())).unwrap();
        assert_eq!(register_font_dir(td.path()), 1);
        assert_eq!(register_font_dir(td.path()), 0);
    }
}
