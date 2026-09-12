//! Name-column decoration: converts a `Row` to a display string with an
//! optional Nerd Font glyph and `ls -F` type indicator. The glyph lookup
//! itself lives in `opusdm_iconview::glyphs`.

use crate::model::Row;
use opusdm_iconview::glyphs::glyph_for;
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

/// Real icon bytes for `row` under icon set `set`, or `None` (caller falls
/// back to the glyph). Extension is derived the same way `IconEntry` does.
pub fn icon_bytes_for_row(row: &Row, set: opusdm_ipc::IconSetId) -> Option<&'static [u8]> {
    let ext = opusdm_iconview::IconEntry::ext_of(&row.name);
    opusdm_iconview::icon_set::icon_bytes_for(kind_of(row), row.mode, ext.as_deref(), set)
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

/// Build the Name-column text: optional glyph, the name, then either the type
/// indicator or the plain symlink marker.
pub fn decorate_name(row: &Row, icons: bool, indicators: bool) -> String {
    let mut out = String::with_capacity(row.name.len() + 16);
    if icons {
        out.push(glyph_for(kind_of(row), row.mode, &row.name));
        out.push(' ');
    }
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
    use opusdm_iconview::glyphs::GLYPH_DIR;

    fn row(name: &str) -> Row {
        Row {
            name: name.into(),
            is_dir: false,
            is_symlink: false,
            broken_link: false,
            size: 0,
            modified_unix: 0,
            uid: 0,
            gid: 0,
            mode: 0o100_644,
        }
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
        assert_eq!(decorate_name(&row("a.txt"), false, false), "a.txt");

        let mut link = row("l");
        link.is_symlink = true;
        assert_eq!(decorate_name(&link, false, false), "l ->");

        let mut broken = row("b");
        broken.is_symlink = true;
        broken.broken_link = true;
        assert_eq!(decorate_name(&broken, false, false), "b (broken link)");
    }

    #[test]
    fn decorate_name_adds_glyph_and_indicator_when_enabled() {
        let mut dir = row("src");
        dir.is_dir = true;
        assert_eq!(decorate_name(&dir, false, true), "src/");
        assert_eq!(decorate_name(&dir, true, true), format!("{GLYPH_DIR} src/"));

        // With indicators on, the '@' replaces the " ->" marker.
        let mut link = row("l");
        link.is_symlink = true;
        assert_eq!(decorate_name(&link, false, true), "l@");
    }

    #[test]
    fn icon_bytes_for_row_is_none_without_a_real_icon_set() {
        assert_eq!(icon_bytes_for_row(&row("a.rs"), opusdm_ipc::IconSetId::None), None);
    }

    #[test]
    fn icon_bytes_for_row_matches_the_extension() {
        assert!(icon_bytes_for_row(&row("a.rs"), opusdm_ipc::IconSetId::BreezeDark).is_some());
    }
}
