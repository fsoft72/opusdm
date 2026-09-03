//! Name-column decoration: Nerd Font glyphs and `ls -F` type indicators.
//!
//! Pure and GTK-free so it can be unit tested. The glyphs live in the Unicode
//! Private Use Area and only render with a patched "Nerd Font" installed,
//! which is why `Appearance::file_icons` defaults to off.

use crate::model::Row;

/// Unix file-type mask and the type values we distinguish.
const S_IFMT: u32 = 0o170_000;
const S_IFSOCK: u32 = 0o140_000;
const S_IFBLK: u32 = 0o060_000;
const S_IFCHR: u32 = 0o020_000;
const S_IFIFO: u32 = 0o010_000;

/// Any execute bit set (owner, group or other).
const EXEC_BITS: u32 = 0o111;

/// Glyph shown for a directory.
const GLYPH_DIR: char = '\u{f07b}';
/// Glyph shown for a symbolic link.
const GLYPH_LINK: char = '\u{f481}';
/// Glyph shown for a regular file with no better match.
const GLYPH_FILE: char = '\u{f15b}';
/// Glyph shown for an executable regular file.
const GLYPH_EXEC: char = '\u{f489}';
/// Glyph shown for a socket.
const GLYPH_SOCKET: char = '\u{f1e6}';
/// Glyph shown for a fifo or character device.
const GLYPH_PIPE: char = '\u{f120}';
/// Glyph shown for a block device.
const GLYPH_BLOCK: char = '\u{f0a0}';

/// Extension to glyph, lowercase and without the dot. Longest suffixes first
/// is not needed: lookup is on the final extension only.
const EXT_GLYPHS: &[(&str, char)] = &[
    ("rs", '\u{e7a8}'),
    ("py", '\u{e73c}'),
    ("js", '\u{e781}'),
    ("mjs", '\u{e781}'),
    ("ts", '\u{e628}'),
    ("tsx", '\u{e628}'),
    ("jsx", '\u{e781}'),
    ("go", '\u{e627}'),
    ("java", '\u{e738}'),
    ("kt", '\u{e634}'),
    ("rb", '\u{e739}'),
    ("php", '\u{e73d}'),
    ("c", '\u{e61e}'),
    ("h", '\u{e61e}'),
    ("cpp", '\u{e61d}'),
    ("cc", '\u{e61d}'),
    ("hpp", '\u{e61d}'),
    ("html", '\u{e736}'),
    ("htm", '\u{e736}'),
    ("css", '\u{e749}'),
    ("scss", '\u{e749}'),
    ("json", '\u{e60b}'),
    ("toml", '\u{e615}'),
    ("yaml", '\u{e615}'),
    ("yml", '\u{e615}'),
    ("ini", '\u{e615}'),
    ("conf", '\u{e615}'),
    ("md", '\u{e73e}'),
    ("pdf", '\u{f1c1}'),
    ("png", '\u{f1c5}'),
    ("jpg", '\u{f1c5}'),
    ("jpeg", '\u{f1c5}'),
    ("gif", '\u{f1c5}'),
    ("bmp", '\u{f1c5}'),
    ("svg", '\u{f1c5}'),
    ("webp", '\u{f1c5}'),
    ("mp3", '\u{f1c7}'),
    ("flac", '\u{f1c7}'),
    ("ogg", '\u{f1c7}'),
    ("opus", '\u{f1c7}'),
    ("wav", '\u{f1c7}'),
    ("m4a", '\u{f1c7}'),
    ("mp4", '\u{f1c8}'),
    ("mkv", '\u{f1c8}'),
    ("avi", '\u{f1c8}'),
    ("mov", '\u{f1c8}'),
    ("webm", '\u{f1c8}'),
    ("zip", '\u{f1c6}'),
    ("tar", '\u{f1c6}'),
    ("gz", '\u{f1c6}'),
    ("bz2", '\u{f1c6}'),
    ("xz", '\u{f1c6}'),
    ("zst", '\u{f1c6}'),
    ("7z", '\u{f1c6}'),
    ("rar", '\u{f1c6}'),
    ("sh", '\u{f489}'),
    ("bash", '\u{f489}'),
    ("zsh", '\u{f489}'),
    ("fish", '\u{f489}'),
    ("txt", '\u{f15c}'),
    ("log", '\u{f15c}'),
];

/// The lowercase extension of `name`, or `None` when there is none. A leading
/// dot does not count as an extension separator, so `.bashrc` has none.
fn extension_of(name: &str) -> Option<String> {
    let stem = name.strip_prefix('.').unwrap_or(name);
    let (_, ext) = stem.rsplit_once('.')?;
    if ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// The Nerd Font glyph standing for this row: entry kind first, then the file
/// extension, then a generic file.
pub fn glyph_for(row: &Row) -> char {
    if row.is_dir {
        return GLYPH_DIR;
    }
    if row.is_symlink {
        return GLYPH_LINK;
    }
    match row.mode & S_IFMT {
        S_IFSOCK => return GLYPH_SOCKET,
        S_IFIFO | S_IFCHR => return GLYPH_PIPE,
        S_IFBLK => return GLYPH_BLOCK,
        _ => {}
    }
    if let Some(ext) = extension_of(&row.name) {
        if let Some((_, g)) = EXT_GLYPHS.iter().find(|(e, _)| *e == ext) {
            return *g;
        }
    }
    if row.mode & EXEC_BITS != 0 {
        return GLYPH_EXEC;
    }
    GLYPH_FILE
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
        out.push(glyph_for(row));
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
    fn extension_lookup_ignores_case_and_leading_dot() {
        assert_eq!(extension_of("main.RS").as_deref(), Some("rs"));
        assert_eq!(extension_of("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension_of(".bashrc"), None);
        assert_eq!(extension_of("Makefile"), None);
        assert_eq!(extension_of("trailing."), None);
    }

    #[test]
    fn glyphs_follow_kind_then_extension() {
        let mut dir = row("src");
        dir.is_dir = true;
        assert_eq!(glyph_for(&dir), GLYPH_DIR);

        let mut link = row("link");
        link.is_symlink = true;
        link.mode = 0o120_777;
        assert_eq!(glyph_for(&link), GLYPH_LINK);

        assert_eq!(glyph_for(&row("main.rs")), '\u{e7a8}');
        assert_eq!(glyph_for(&row("app.py")), '\u{e73c}');
        assert_eq!(glyph_for(&row("clip.mkv")), '\u{f1c8}');
        assert_eq!(glyph_for(&row("notes")), GLYPH_FILE);

        let mut exe = row("runme");
        exe.mode = 0o100_755;
        assert_eq!(glyph_for(&exe), GLYPH_EXEC);
    }

    #[test]
    fn special_files_get_their_own_glyph_and_indicator() {
        let mut sock = row("app.sock");
        sock.mode = S_IFSOCK | 0o644;
        assert_eq!(glyph_for(&sock), GLYPH_SOCKET);
        assert_eq!(type_indicator(&sock), Some('='));

        let mut fifo = row("pipe");
        fifo.mode = S_IFIFO | 0o644;
        assert_eq!(glyph_for(&fifo), GLYPH_PIPE);
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
}
