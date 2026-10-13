//! What a file is for the viewer, decided by its extension.

use std::io::Read;
use std::path::Path;

/// Picture extensions (lower case).
pub const IMAGE_EXTS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "tiff"];
/// Video extensions (lower case).
pub const VIDEO_EXTS: [&str; 11] = ["mp4", "mkv", "webm", "avi", "mov", "m4v", "mpg", "mpeg", "wmv", "flv", "ogv"];
/// Document extension (lower case).
pub const PDF_EXT: &str = "pdf";
/// Markdown extensions (lower case).
pub const MARKDOWN_EXTS: [&str; 2] = ["md", "markdown"];
/// Source code and plain text extensions (lower case).
pub const SOURCE_EXTS: [&str; 85] = [
    "txt", "log", "rs", "c", "h", "cc", "cpp", "cxx", "hh", "hpp", "hxx", "m", "mm", "cs", "java", "kt", "kts",
    "scala", "groovy", "gradle", "go", "swift", "dart", "zig", "nim", "d", "py", "pyw", "rb", "php", "pl", "pm",
    "lua", "tcl", "r", "jl", "hs", "ml", "mli", "clj", "el", "lisp", "erl", "ex", "exs", "js", "mjs", "cjs", "ts",
    "jsx", "tsx", "vue", "svelte", "html", "htm", "xml", "xsl", "css", "scss", "sass", "less", "json",
    "jsonc", "toml", "yaml", "yml", "ini", "conf", "cfg", "properties", "desktop", "service", "sh", "bash", "zsh",
    "fish", "ps1", "bat", "cmd", "sql", "diff", "patch", "cmake", "mk", "tex",
];
/// Source file names without a useful extension (lower case).
pub const SOURCE_NAMES: [&str; 6] = ["makefile", "gnumakefile", "dockerfile", "containerfile", "justfile", "pkgbuild"];
/// Bytes read from a file with an unknown extension to tell text from a picture.
const SNIFF_BYTES: usize = 4096;

/// The things the viewer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    /// A PDF document, shown one page at a time like a picture.
    Pdf,
    /// A Markdown text, shown rendered and scrolled.
    Markdown,
    /// Source code or plain text, shown syntax coloured and scrolled.
    Source,
}

/// Picture, video, PDF, Markdown or source by extension (case-insensitive), or
/// source by a known file name (`Makefile`); `None` for anything else.
pub fn kind_of(path: &Path) -> Option<MediaKind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if SOURCE_NAMES.contains(&name.as_str()) { return Some(MediaKind::Source); }
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) { return Some(MediaKind::Image); }
    if VIDEO_EXTS.contains(&ext.as_str()) { return Some(MediaKind::Video); }
    if ext == PDF_EXT { return Some(MediaKind::Pdf); }
    if MARKDOWN_EXTS.contains(&ext.as_str()) { return Some(MediaKind::Markdown); }
    if SOURCE_EXTS.contains(&ext.as_str()) { return Some(MediaKind::Source); }
    None
}

/// Kind used to show a file: an unknown extension is shown as source when the
/// file starts like text (a `#!` script, a `LICENSE`), else tried as a picture
/// (the file the user opened is always shown, whatever its name).
pub fn kind_or_image(path: &Path) -> MediaKind {
    if let Some(kind) = kind_of(path) { return kind; }
    if _head(path).is_some_and(|head| looks_like_text(&head)) { MediaKind::Source } else { MediaKind::Image }
}

/// True for the start of a text: a `#!` shebang, or UTF-8 without NUL bytes (a
/// character cut at the end of `head` is fine).
pub fn looks_like_text(head: &[u8]) -> bool {
    if head.starts_with(b"#!") { return true; }
    if head.contains(&0) { return false; }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

/// The first `SNIFF_BYTES` of a regular file; `None` when it cannot be read.
fn _head(path: &Path) -> Option<Vec<u8>> {
    if !path.is_file() { return None; }
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    std::fs::File::open(path).ok()?.take(SNIFF_BYTES as u64).read_to_end(&mut head).ok()?;
    Some(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::filetypes::{default_filetypes, MatchRule};

    #[test]
    fn table() {
        assert_eq!(kind_of(Path::new("a.png")), Some(MediaKind::Image));
        assert_eq!(kind_of(Path::new("/x/y/A.JPG")), Some(MediaKind::Image));
        assert_eq!(kind_of(Path::new("clip.MKV")), Some(MediaKind::Video));
        assert_eq!(kind_of(Path::new("manual.PDF")), Some(MediaKind::Pdf));
        assert_eq!(kind_of(Path::new("README.MD")), Some(MediaKind::Markdown));
        assert_eq!(kind_of(Path::new("a/b.markdown")), Some(MediaKind::Markdown));
        assert_eq!(kind_of(Path::new("notes.txt")), Some(MediaKind::Source));
        assert_eq!(kind_of(Path::new("src/Main.RS")), Some(MediaKind::Source));
        assert_eq!(kind_of(Path::new("a/Makefile")), Some(MediaKind::Source));
        assert_eq!(kind_of(Path::new("Dockerfile")), Some(MediaKind::Source));
        assert_eq!(kind_of(Path::new("archive.zip")), None);
        assert_eq!(kind_of(Path::new("noext")), None);
        assert_eq!(kind_of(Path::new(".png")), None);
    }

    #[test]
    fn unknown_extension_is_tried_as_a_picture() {
        assert_eq!(kind_or_image(Path::new("photo.xyz")), MediaKind::Image);
        assert_eq!(kind_or_image(Path::new("a.mp4")), MediaKind::Video);
        assert_eq!(kind_or_image(Path::new("a.pdf")), MediaKind::Pdf);
    }

    #[test]
    fn an_unknown_file_with_text_is_source() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("deploy");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        let notes = dir.path().join("LICENSE");
        std::fs::write(&notes, "Permission is granted, è\n").unwrap();
        let blob = dir.path().join("photo.xyz");
        std::fs::write(&blob, [0x89, b'P', b'N', b'G', 0, 0, 1]).unwrap();
        assert_eq!(kind_or_image(&script), MediaKind::Source);
        assert_eq!(kind_or_image(&notes), MediaKind::Source);
        assert_eq!(kind_or_image(&blob), MediaKind::Image);
        assert_eq!(kind_or_image(&dir.path().join("missing")), MediaKind::Image);
    }

    #[test]
    fn text_is_utf8_without_nul_or_a_shebang() {
        assert!(looks_like_text(b"plain words\n"));
        assert!(looks_like_text(b""));
        assert!(looks_like_text(b"#!/usr/bin/perl\n\x00"));
        assert!(looks_like_text("cut in the middle: \u{e8}".as_bytes().split_last().unwrap().1), "a character cut at the end of the head is fine");
        assert!(!looks_like_text(b"a\x00b"));
        assert!(!looks_like_text(&[0xC9, 0xCD, 0xBB, b'a']));
    }

    #[test]
    fn the_default_source_type_matches_the_source_extensions() {
        let types = default_filetypes();
        let src = types.types.iter().find(|t| t.name == "Source file").expect("default Source file type");
        let MatchRule::Glob { pattern } = &src.rule else { panic!("Source file type is not a glob") };
        let globs: Vec<&str> = pattern.split(';').collect();
        for ext in SOURCE_EXTS {
            assert!(globs.contains(&format!("*.{ext}").as_str()), "{ext} missing from the default Source file type");
        }
        for name in SOURCE_NAMES {
            assert!(globs.iter().any(|g| g.eq_ignore_ascii_case(name)), "{name} missing from the default Source file type");
        }
    }

    #[test]
    fn the_default_pdf_type_matches_the_pdf_extension() {
        let types = default_filetypes();
        let pdf = types.types.iter().find(|t| t.name == "PDF file").expect("default PDF file type");
        let MatchRule::Glob { pattern } = &pdf.rule else { panic!("PDF file type is not a glob") };
        assert_eq!(pattern, &format!("*.{PDF_EXT}"));
    }

    #[test]
    fn the_default_markdown_type_matches_the_markdown_extensions() {
        let types = default_filetypes();
        let md = types.types.iter().find(|t| t.name == "Markdown file").expect("default Markdown file type");
        let MatchRule::Glob { pattern } = &md.rule else { panic!("Markdown file type is not a glob") };
        for ext in MARKDOWN_EXTS {
            assert!(pattern.contains(&format!("*.{ext}")), "{ext} missing from the default Markdown file type");
        }
    }

    #[test]
    fn every_default_video_type_extension_is_a_video() {
        let types = default_filetypes();
        let video = types.types.iter().find(|t| t.name == "Video file").expect("default Video file type");
        let MatchRule::Glob { pattern } = &video.rule else { panic!("Video file type is not a glob") };
        for glob in pattern.split(';') {
            let ext = glob.trim_start_matches("*.");
            assert!(VIDEO_EXTS.contains(&ext), "{ext} is not a viewer video extension");
        }
    }

    #[test]
    fn every_image_extension_is_in_the_default_image_type() {
        let types = default_filetypes();
        let image = types.types.iter().find(|t| t.name == "Image file").expect("default Image file type");
        let MatchRule::Glob { pattern } = &image.rule else { panic!("Image file type is not a glob") };
        for ext in IMAGE_EXTS {
            assert!(pattern.contains(&format!("*.{ext}")), "{ext} missing from the default Image file type");
        }
    }
}
