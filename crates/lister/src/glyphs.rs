//! Name-column decoration: converts a `Row` to a display string with an
//! optional `ls -F` type indicator. Icons come from the active theme (see
//! `icon_texture_for_row`).

use crate::model::Row;
use gtk4 as gtk;
use opusdm_iconview::IconKind;

/// Unix file-type mask and the type values we distinguish for the indicator.
const S_IFMT: u32 = 0o170_000;
const S_IFSOCK: u32 = 0o140_000;
const S_IFIFO: u32 = 0o010_000;

/// Any execute bit set (owner, group or other).
const EXEC_BITS: u32 = 0o111;

/// The `IconKind` for a `Row`, following the same precedence the icon view uses.
fn kind_of(row: &Row) -> IconKind {
    if row.broken_link {
        return IconKind::Broken;
    }
    if row.is_symlink {
        return IconKind::Symlink;
    }
    if row.is_dir {
        return IconKind::Dir;
    }
    IconKind::File
}

/// Cached decoded icon for `row` under icon set `set` (see
/// `opusdm_iconview::icon_set::icon_texture_for`), or `None` when the theme has none.
pub fn icon_texture_for_row(row: &Row, set: &opusdm_ipc::IconSetId) -> Option<gtk::gdk::Texture> {
    let ext = opusdm_iconview::IconEntry::ext_of(&row.name);
    opusdm_iconview::icon_set::icon_texture_for(kind_of(row), row.mode, ext.as_deref(), set)
}

/// The `ls -F` style type character: `/` directory, `@` symlink, `=` socket,
/// `|` fifo, `*` executable, or `None` for a plain file.
pub fn type_indicator(row: &Row) -> Option<char> {
    if row.is_symlink {
        return Some('@');
    }
    if row.is_dir {
        return Some('/');
    }
    match row.mode & S_IFMT {
        S_IFSOCK => return Some('='),
        S_IFIFO => return Some('|'),
        _ => {}
    }
    (row.mode & EXEC_BITS != 0).then_some('*')
}

/// Build the Name-column text: the name, then either the type indicator or
/// the plain symlink marker.
pub fn decorate_name(row: &Row, indicators: bool) -> String {
    let mut out = String::with_capacity(row.name.len() + 16);
    out.push_str(&row.name);
    if indicators {
        if let Some(c) = type_indicator(row) {
            out.push(c);
        }
    }
    if row.broken_link {
        out.push_str(" (broken link)");
    } else if row.is_symlink && !indicators {
        out.push_str(" ->");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> Row {
        Row::new(name.into(), false, false, false, 0, 0, 0, 0, 0o100_644)
    }

    #[test]
    fn special_files_get_their_own_indicator() {
        let mut sock = row("app.sock");
        sock.mode = S_IFSOCK | 0o644;
        assert_eq!(type_indicator(&sock), Some('='));

        let mut fifo = row("pipe");
        fifo.mode = S_IFIFO | 0o644;
        assert_eq!(type_indicator(&fifo), Some('|'));
    }

    #[test]
    fn indicators_match_ls_dash_f() {
        let mut dir = row("d");
        dir.is_dir = true;
        assert_eq!(type_indicator(&dir), Some('/'));

        let mut link = row("l");
        link.is_symlink = true;
        assert_eq!(type_indicator(&link), Some('@'));

        let mut exe = row("x");
        exe.mode = 0o100_755;
        assert_eq!(type_indicator(&exe), Some('*'));

        assert_eq!(type_indicator(&row("plain.txt")), None);
    }

    #[test]
    fn decorate_name_defaults_match_the_previous_rendering() {
        assert_eq!(decorate_name(&row("a.txt"), false), "a.txt");

        let mut link = row("l");
        link.is_symlink = true;
        assert_eq!(decorate_name(&link, false), "l ->");

        let mut broken = row("b");
        broken.is_symlink = true;
        broken.broken_link = true;
        assert_eq!(decorate_name(&broken, false), "b (broken link)");
    }

    #[test]
    fn decorate_name_adds_indicator_when_enabled() {
        let mut dir = row("src");
        dir.is_dir = true;
        assert_eq!(decorate_name(&dir, true), "src/");

        // With indicators on, the '@' replaces the " ->" marker.
        let mut link = row("l");
        link.is_symlink = true;
        assert_eq!(decorate_name(&link, true), "l@");
    }

    /// A valid 1x1 PNG.
    const ONE_PIXEL_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
        0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
        0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
        0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn icon_texture_for_row_matches_the_extension_and_is_cached() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let icons_dir = td.path().join("opusdm").join("themes").join("t").join("icons");
        std::fs::create_dir_all(&icons_dir).unwrap();
        std::fs::write(icons_dir.join("code.png"), ONE_PIXEL_PNG).unwrap();
        let set = opusdm_ipc::IconSetId::Custom("t".to_string());
        let first = icon_texture_for_row(&row("a.rs"), &set).expect("code.png decodes");
        let second = icon_texture_for_row(&row("b.py"), &set).expect("same icon");
        assert_eq!(first, second, "the decoded texture is shared");
        assert!(icon_texture_for_row(&row("a.txt"), &set).is_none());
    }
}
