//! What kind of file a name and its first bytes say it is (picture, text,
//! archive, binary) and the hex dump of a binary, for the text Viewer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Text,
    Archive,
    Binary,
}

/// File extensions treated as text.
const TEXT_EXTS: &[&str] = &[
    "rs", "c", "h", "cpp", "hpp", "cc", "py", "js", "ts", "jsx", "tsx", "html", "css", "scss", "json", "toml",
    "yaml", "yml", "md", "txt", "sh", "bash", "xml", "ini", "conf", "log", "diff", "patch", "sql", "csv", "tsv",
    "lock",
];


/// File extensions treated as pictures.
pub const IMAGE_EXTS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "tiff"];

/// Classify a file from its lower-case extension and its first bytes: the
/// extension decides for archives, pictures and known text types; otherwise
/// the first 512 bytes decide between text (no NUL, valid UTF-8) and binary.
pub fn kind_of(ext: &str, head: &[u8]) -> FileKind {
    if crate::archive::ARCHIVE_EXTS.contains(&ext) {
        return FileKind::Archive;
    }
    if IMAGE_EXTS.contains(&ext) {
        return FileKind::Image;
    }
    if TEXT_EXTS.contains(&ext) {
        return FileKind::Text;
    }
    let probe = &head[..head.len().min(512)];
    if !probe.is_empty() && !probe.contains(&0) && std::str::from_utf8(probe).is_ok() {
        return FileKind::Text;
    }
    FileKind::Binary
}

/// Generate a formatted hex dump string for a buffer.
pub fn generate_hex_dump(buf: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (chunk_idx, chunk) in buf.chunks(16).enumerate() {
        let offset = chunk_idx * 16;
        let _ = write!(out, "{offset:08x}:  ");
        // Hex bytes
        for i in 0..16 {
            if i < chunk.len() {
                let _ = write!(out, "{:02x} ", chunk[i]);
            } else {
                out.push_str("   ");
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        // ASCII printable
        for &b in chunk {
            if (32..=126).contains(&b) {
                out.push(b as char);
            } else {
                out.push('.');
            }
        }
        out.push_str("|\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_hex_dump() {
        let data = b"Hello, World!\n";
        let dump = generate_hex_dump(data);
        assert!(dump.contains("48 65 6c 6c 6f"));
        assert!(dump.contains("|Hello, World!.|"));
    }

    #[test]
    fn kind_of_classifies_by_extension_first_then_by_content() {
        assert_eq!(kind_of("png", b"\x89PNG"), FileKind::Image);
        assert_eq!(kind_of("rs", b"\x00\x01"), FileKind::Text, "a known text extension wins");
        assert_eq!(kind_of("zip", b"PK"), FileKind::Archive);
        assert_eq!(kind_of("weird", b"plain words\n"), FileKind::Text);
        assert_eq!(kind_of("weird", b"a\x00b"), FileKind::Binary);
        assert_eq!(kind_of("weird", b""), FileKind::Binary);
        assert_eq!(kind_of("weird", &[0xC9, 0xCD, 0xBB]), FileKind::Binary, "CP437 bytes are not UTF-8");
    }
}
