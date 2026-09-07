//! Name-column / icon decoration: Nerd Font glyphs by entry kind and by file
//! extension. Pure and GTK-free. The glyphs live in the Unicode Private Use
//! Area and only render with a patched "Nerd Font" installed.

use crate::types::IconKind;

const S_IFMT: u32 = 0o170_000;
const S_IFSOCK: u32 = 0o140_000;
const S_IFBLK: u32 = 0o060_000;
const S_IFCHR: u32 = 0o020_000;
const S_IFIFO: u32 = 0o010_000;

/// Any execute bit set (owner, group or other).
const EXEC_BITS: u32 = 0o111;

/// Glyph shown for a directory.
pub const GLYPH_DIR: char = '\u{f07b}';
/// Glyph shown for a symbolic link.
pub const GLYPH_LINK: char = '\u{f481}';
/// Glyph shown for a regular file with no better match.
pub const GLYPH_FILE: char = '\u{f15b}';
/// Glyph shown for an executable regular file.
pub const GLYPH_EXEC: char = '\u{f489}';
/// Glyph shown for a socket.
pub const GLYPH_SOCKET: char = '\u{f1e6}';
/// Glyph shown for a fifo or character device.
pub const GLYPH_PIPE: char = '\u{f120}';
/// Glyph shown for a block device.
pub const GLYPH_BLOCK: char = '\u{f0a0}';

/// Extension to glyph, lowercase and without the dot. Lookup is on the final
/// extension only.
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

/// The lowercase final extension of `name`, or `None`. A leading dot does not
/// start an extension.
fn extension_of(name: &str) -> Option<String> {
    let stem = name.strip_prefix('.').unwrap_or(name);
    let (_, ext) = stem.rsplit_once('.')?;
    if ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// The glyph for an entry: kind first (dir, symlink, socket, fifo, block),
/// then the file extension, then executable, then a generic file.
pub fn glyph_for(kind: IconKind, mode: u32, name: &str) -> char {
    if kind == IconKind::Dir {
        return GLYPH_DIR;
    }
    if kind == IconKind::Symlink || kind == IconKind::Broken {
        return GLYPH_LINK;
    }
    match mode & S_IFMT {
        S_IFSOCK => return GLYPH_SOCKET,
        S_IFIFO | S_IFCHR => return GLYPH_PIPE,
        S_IFBLK => return GLYPH_BLOCK,
        _ => {}
    }
    if let Some(ext) = extension_of(name) {
        if let Some((_, g)) = EXT_GLYPHS.iter().find(|(e, _)| *e == ext) {
            return *g;
        }
    }
    if mode & EXEC_BITS != 0 {
        return GLYPH_EXEC;
    }
    GLYPH_FILE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_follow_kind_then_extension() {
        assert_eq!(glyph_for(IconKind::Dir, 0o040_755, "src"), GLYPH_DIR);
        assert_eq!(glyph_for(IconKind::Symlink, 0o120_777, "link"), GLYPH_LINK);
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "main.rs"), '\u{e7a8}');
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "app.py"), '\u{e73c}');
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "clip.mkv"), '\u{f1c8}');
        assert_eq!(glyph_for(IconKind::File, 0o100_644, "notes"), GLYPH_FILE);
        assert_eq!(glyph_for(IconKind::File, 0o100_755, "runme"), GLYPH_EXEC);
    }

    #[test]
    fn special_files_get_their_own_glyph() {
        assert_eq!(glyph_for(IconKind::File, S_IFSOCK | 0o644, "app.sock"), GLYPH_SOCKET);
        assert_eq!(glyph_for(IconKind::File, S_IFIFO | 0o644, "pipe"), GLYPH_PIPE);
        assert_eq!(glyph_for(IconKind::File, S_IFBLK | 0o644, "sda"), GLYPH_BLOCK);
    }
}
