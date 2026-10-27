//! Finding the applets installed on disk.

use crate::manifest::{self, Manifest};
use std::path::{Path, PathBuf};

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Name of the manifest file inside an applet folder.
pub const MANIFEST_FILE: &str = "applet.json";

/// One applet folder found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Discovered {
    /// Folder name.
    pub id: String,
    /// Absolute or as-given path of the folder.
    pub dir: PathBuf,
    /// The parsed manifest, or a message naming the file and the problem.
    pub manifest: Result<Manifest, String>,
}

/// Directory holding the installed applets.
pub fn applets_dir() -> PathBuf {
    opusdm_ipc::settings::config_dir().join("applets")
}

/// List the applet folders in `dir`, sorted by id.
///
/// A missing `dir` is normal (no applets yet) and gives an empty list. Plain
/// files and names starting with `.` are skipped; symlinks to folders are
/// followed. A folder whose manifest is missing or invalid is still listed,
/// with an `Err` naming the file.
pub fn discover(dir: &Path) -> Vec<Discovered> {
    if !dir.exists() {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            opusdm_ipc::err_log!(APP_TAG, "cannot read {}: {e}", dir.display());
            return Vec::new();
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => found.extend(_discover_entry(&entry.path())),
            Err(e) => {
                opusdm_ipc::err_log!(APP_TAG, "cannot read an entry of {}: {e}", dir.display())
            }
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// Turn one directory entry into a `Discovered`, or `None` for files and dot names.
fn _discover_entry(path: &Path) -> Option<Discovered> {
    let id = path.file_name()?.to_string_lossy().into_owned();
    if id.starts_with('.') || !path.is_dir() {
        return None;
    }
    Some(Discovered {
        id,
        dir: path.to_path_buf(),
        manifest: _read_manifest(path),
    })
}

/// Read and parse `applet.json` in `dir`; every error names the file.
fn _read_manifest(dir: &Path) -> Result<Manifest, String> {
    let file = dir.join(MANIFEST_FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("{}: not found", file.display()))
        }
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    manifest::parse(&text).map_err(|e| format!("{}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const GOOD: &str = r#"{ "name": "Clock", "version": "1.0" }"#;

    #[test]
    fn discover_lists_a_valid_and_a_broken_applet() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("clock");
        fs::create_dir(&good).unwrap();
        fs::write(good.join("applet.json"), GOOD).unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();
        let bad = dir.path().join("bad");
        fs::create_dir(&bad).unwrap();
        fs::write(bad.join("applet.json"), "{ nope").unwrap();

        let found = discover(dir.path());
        let ids: Vec<&str> = found.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["bad", "clock", "empty"]);
        assert_eq!(found[1].dir, good);
        assert_eq!(found[1].manifest.as_ref().unwrap().name, "Clock");
        let err = found[0].manifest.as_ref().unwrap_err();
        assert!(err.contains("applet.json"), "{err}");
        let err = found[2].manifest.as_ref().unwrap_err();
        assert!(
            err.contains("applet.json") && err.contains("not found"),
            "{err}"
        );
    }

    #[test]
    fn discover_ignores_files_and_dot_folders() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("readme.txt"), "x").unwrap();
        fs::create_dir(dir.path().join(".hidden")).unwrap();
        fs::create_dir(dir.path().join("clock")).unwrap();
        let found = discover(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "clock");
    }

    #[test]
    fn a_missing_dir_gives_no_applets() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(&dir.path().join("nope")).is_empty());
    }

    #[test]
    fn a_symlinked_folder_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        fs::write(real.path().join("applet.json"), GOOD).unwrap();
        std::os::unix::fs::symlink(real.path(), dir.path().join("linked")).unwrap();
        let found = discover(dir.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].manifest.is_ok());
    }
}
