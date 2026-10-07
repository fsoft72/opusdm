//! Cleanup of partial output after a failed operation.

use std::io::ErrorKind;
use std::path::Path;

/// Removes a partial or temporary file left by a failed operation. A file that
/// is already gone is fine; any other failure is logged with the path, so a
/// leftover file can be found.
pub fn remove_partial(path: &Path) {
    let Err(e) = std::fs::remove_file(path) else { return };
    if e.kind() == ErrorKind::NotFound {
        return;
    }
    opusdm_ipc::err_log!("opusdm-fsops", "cannot remove partial file {}: {e}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("part");
        std::fs::write(&file, b"x").unwrap();
        remove_partial(&file);
        assert!(!file.exists());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        remove_partial(&dir.path().join("none"));
    }
}
