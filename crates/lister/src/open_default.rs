//! Hand a local file to its default application.
//!
//! The `file://` URI is built by GIO, which escapes `#`, `%`, `?`, spaces
//! and non-UTF-8 bytes; a hand-made `format!("file://{}")` breaks on them.

use gtk4::gio;
use gtk4::prelude::*;
use std::path::Path;

/// The escaped `file://` URI of the local path `path`.
pub fn file_uri(path: &Path) -> String {
    gio::File::for_path(path).uri().to_string()
}

/// Open `path` with its default application, returning the launch error.
pub fn open_default(path: &Path) -> Result<(), String> {
    gio::AppInfo::launch_default_for_uri(&file_uri(path), gio::AppLaunchContext::NONE)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn plain_paths_keep_their_shape() {
        assert_eq!(file_uri(Path::new("/tmp/a.txt")), "file:///tmp/a.txt");
    }

    #[test]
    fn reserved_characters_are_escaped() {
        let uri = file_uri(Path::new("/tmp/a #1 100%?.txt"));
        assert_eq!(uri, "file:///tmp/a%20%231%20100%25%3F.txt");
    }

    #[test]
    fn non_utf8_bytes_are_escaped() {
        let path = Path::new(OsStr::from_bytes(b"/tmp/caf\xe9"));
        assert_eq!(file_uri(path), "file:///tmp/caf%E9");
    }
}
