//! What a file is for the viewer, decided by its extension.

use std::path::Path;

/// Picture extensions (lower case).
pub const IMAGE_EXTS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "tiff"];
/// Video extensions (lower case).
pub const VIDEO_EXTS: [&str; 11] = ["mp4", "mkv", "webm", "avi", "mov", "m4v", "mpg", "mpeg", "wmv", "flv", "ogv"];
/// Document extension (lower case).
pub const PDF_EXT: &str = "pdf";

/// The things the viewer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    /// A PDF document, shown one page at a time like a picture.
    Pdf,
}

/// Picture, video or PDF by extension (case-insensitive); `None` for anything else.
pub fn kind_of(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) { return Some(MediaKind::Image); }
    if VIDEO_EXTS.contains(&ext.as_str()) { return Some(MediaKind::Video); }
    if ext == PDF_EXT { return Some(MediaKind::Pdf); }
    None
}

/// Kind used to show a file: an unknown extension is tried as a picture (the
/// file the user opened is always shown, whatever its name).
pub fn kind_or_image(path: &Path) -> MediaKind {
    kind_of(path).unwrap_or(MediaKind::Image)
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
        assert_eq!(kind_of(Path::new("notes.txt")), None);
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
    fn the_default_pdf_type_matches_the_pdf_extension() {
        let types = default_filetypes();
        let pdf = types.types.iter().find(|t| t.name == "PDF file").expect("default PDF file type");
        let MatchRule::Glob { pattern } = &pdf.rule else { panic!("PDF file type is not a glob") };
        assert_eq!(pattern, &format!("*.{PDF_EXT}"));
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
