//! Remote locations in the Lister (F-39), the pure part: telling a remote
//! path from a local one, turning server entries into rows, parents, and the
//! scratch folders downloads land in. The GTK flows are in `remote_ui.rs`.

use crate::model::Row;
use opusdm_ftp::{ConnectParams, RemoteEntry, RemoteUrl};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Prefix of the scratch folders downloads are written to.
const DOWNLOAD_DIR_PREFIX: &str = "opusdm-remote-";

/// Scratch folders older than this are removed when a new one is made.
const DOWNLOAD_DIR_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The remote location `path` stands for, or `None` for a local path.
pub fn url_of(path: &Path) -> Option<RemoteUrl> {
    let text = path.to_string_lossy();
    if !opusdm_ftp::is_remote(&text) {
        return None;
    }
    RemoteUrl::parse(&text).ok()
}

/// The path of `url` as the Lister keeps it.
pub fn path_of(url: &RemoteUrl) -> PathBuf {
    PathBuf::from(url.to_string())
}

/// The folder above `path`, remote or local; `None` at a root.
pub fn parent_of(path: &Path) -> Option<PathBuf> {
    match url_of(path) {
        Some(url) => url.parent().map(|p| path_of(&p)),
        None => path.parent().map(PathBuf::from),
    }
}

/// Rows for a server listing. Owner and group are not shown by the protocols,
/// so rows carry the same placeholder ids archive rows do.
pub fn rows_from(entries: Vec<RemoteEntry>) -> Vec<Row> {
    entries
        .into_iter()
        .map(|e| {
            let mode = if e.mode != 0 { e.mode } else if e.is_dir { 0o755 } else { 0o644 };
            Row::new(e.name, e.is_dir, e.is_symlink, false, if e.is_dir { 0 } else { e.size }, e.modified, 1000, 1000, mode)
        })
        .collect()
}

/// Connection settings for `url`: the address book entry for the same
/// host, port and protocol supplies passive mode and certificate checking.
pub fn params_for(url: &RemoteUrl, book: &opusdm_ftp::AddressBook) -> ConnectParams {
    let mut params = ConnectParams::new(url.clone());
    if let Some(site) = book.sites.iter().find(|s| s.host.eq_ignore_ascii_case(&url.host) && s.port == url.port && s.protocol == url.scheme) {
        params.passive = site.passive;
        params.tls_verify = site.tls_verify;
    }
    params
}

/// Create a fresh scratch folder for a download, first removing the stale
/// ones left by earlier sessions.
pub fn new_download_dir() -> std::io::Result<PathBuf> {
    sweep_old_downloads(&std::env::temp_dir(), SystemTime::now());
    let dir = std::env::temp_dir().join(format!("{DOWNLOAD_DIR_PREFIX}{}-{}", std::process::id(), unique_suffix()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A number that differs between calls in one process.
fn unique_suffix() -> u128 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Remove scratch folders in `base` last modified more than a day before `now`.
pub fn sweep_old_downloads(base: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(base) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(DOWNLOAD_DIR_PREFIX) {
            continue;
        }
        let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok());
        if age.is_some_and(|a| a > DOWNLOAD_DIR_MAX_AGE) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, size: u64) -> RemoteEntry {
        RemoteEntry { name: name.into(), is_dir, is_symlink: false, size, modified: 5, mode: 0 }
    }

    #[test]
    fn url_of_only_recognises_remote_paths() {
        assert!(url_of(Path::new("sftp://me@h/a/b")).is_some());
        assert!(url_of(Path::new("/home/me")).is_none());
        assert!(url_of(Path::new("/a.zip#x")).is_none());
        assert!(url_of(Path::new("ftp://")).is_none());
    }

    #[test]
    fn parent_of_walks_up_remote_and_local_paths() {
        assert_eq!(parent_of(Path::new("sftp://h/a/b")), Some(PathBuf::from("sftp://h/a")));
        assert_eq!(parent_of(Path::new("sftp://h/a")), Some(PathBuf::from("sftp://h/")));
        assert_eq!(parent_of(Path::new("sftp://h/")), None);
        assert_eq!(parent_of(Path::new("/a/b")), Some(PathBuf::from("/a")));
        assert_eq!(parent_of(Path::new("/")), None);
    }

    #[test]
    fn rows_get_modes_sizes_and_times() {
        let rows = rows_from(vec![entry("d", true, 4096), entry("f", false, 7)]);
        assert_eq!((rows[0].is_dir, rows[0].size, rows[0].mode), (true, 0, 0o755));
        assert_eq!((rows[1].size, rows[1].mode, rows[1].modified_unix), (7, 0o644, 5));
    }

    #[test]
    fn the_address_book_supplies_passive_and_tls_settings() {
        let book = opusdm_ftp::AddressBook {
            sites: vec![opusdm_ftp::Site {
                name: "x".into(),
                host: "H".into(),
                port: 990,
                protocol: opusdm_ftp::Scheme::Ftps,
                passive: false,
                tls_verify: false,
                ..Default::default()
            }],
        };
        let p = params_for(&RemoteUrl::parse("ftps://h:990/").unwrap(), &book);
        assert!(!p.passive && !p.tls_verify);
        let p = params_for(&RemoteUrl::parse("ftps://other/").unwrap(), &book);
        assert!(p.passive && p.tls_verify);
    }

    #[test]
    fn only_old_scratch_folders_are_swept() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join(format!("{DOWNLOAD_DIR_PREFIX}1-1"));
        let other = base.path().join("unrelated");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&other).unwrap();
        // Two days later everything with the prefix is stale.
        sweep_old_downloads(base.path(), SystemTime::now() + Duration::from_secs(2 * 24 * 3600));
        assert!(!old.exists());
        assert!(other.exists());
        // Right now nothing is.
        let fresh = base.path().join(format!("{DOWNLOAD_DIR_PREFIX}2-2"));
        std::fs::create_dir(&fresh).unwrap();
        sweep_old_downloads(base.path(), SystemTime::now());
        assert!(fresh.exists());
    }
}
