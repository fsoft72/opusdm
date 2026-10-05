//! Turn a command line argument (path or URL) into a URI.

use crate::error::MediaError;
use gstreamer::glib;
use std::path::Path;

/// URI for `arg`: a value with a scheme (`file://`, `http://`, ...) is kept, a
/// path becomes an absolute `file://` URI (relative paths use the current folder).
pub fn to_uri(arg: &str) -> Result<String, MediaError> {
    if arg.is_empty() { return Err(MediaError::pipeline("empty file name")); }
    if arg.contains("://") { return Ok(arg.to_string()); }
    let abs = std::path::absolute(Path::new(arg)).map_err(|e| MediaError::pipeline(format!("bad path {arg}: {e}")))?;
    glib::filename_to_uri(&abs, None).map(|u| u.to_string()).map_err(|e| MediaError::pipeline(format!("bad path {arg}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_kept() {
        assert_eq!(to_uri("https://x.org/a.mp4").unwrap(), "https://x.org/a.mp4");
        assert_eq!(to_uri("file:///tmp/a.mp3").unwrap(), "file:///tmp/a.mp3");
    }

    #[test]
    fn absolute_paths_are_escaped() {
        assert_eq!(to_uri("/tmp/a b.mp3").unwrap(), "file:///tmp/a%20b.mp3");
    }

    #[test]
    fn relative_paths_become_absolute() {
        let uri = to_uri("movie.mkv").unwrap();
        assert!(uri.starts_with("file:///") && uri.ends_with("/movie.mkv"), "{uri}");
    }

    #[test]
    fn empty_is_rejected() {
        assert!(to_uri("").is_err());
    }
}
