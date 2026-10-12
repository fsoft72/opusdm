//! Import of VPN configuration files, and the folder that keeps the
//! certificates and keys of the imported profiles.

use crate::error::NetError;
use crate::profile::{new_uuid, Profile};
use crate::{ovpn, wgconf};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Mode of the folder of a profile's files: only the user.
const DIR_MODE: u32 = 0o700;
/// Mode of a certificate or key file: only the user.
const FILE_MODE: u32 = 0o600;
/// The largest configuration file read, in bytes.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// A file to write when the imported profile is saved.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingFile {
    pub path: PathBuf,
    pub contents: String,
}

// A private key may be inside: `Debug` shows only the path.
impl std::fmt::Debug for PendingFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PendingFile({})", self.path.display())
    }
}

/// The result of an import: a profile not saved yet, the directives that
/// were left out, and the files to write on save.
#[derive(Clone, Debug)]
pub struct Imported {
    pub profile: Profile,
    pub ignored: Vec<String>,
    pub files: Vec<PendingFile>,
}

/// The folder that keeps the files of the profile `uuid`:
/// `~/.local/share/opusdm/vpn/<uuid>`.
pub fn files_dir(uuid: &str) -> Result<PathBuf, NetError> {
    let data = dirs::data_dir().ok_or_else(|| NetError::other("No data folder (HOME is not set)"))?;
    Ok(data.join("opusdm").join("vpn").join(uuid))
}

/// Read a WireGuard `.conf` or an OpenVPN `.ovpn` file.
pub fn import_file(path: &Path) -> Result<Imported, NetError> {
    let fail = |e: std::io::Error| NetError::other(format!("{}: {e}", path.display()));
    let size = fs::metadata(path).map_err(fail)?.len();
    if size > MAX_FILE_BYTES { return Err(NetError::other(format!("{} is too large for a VPN configuration", path.display()))); }
    let text = fs::read_to_string(path).map_err(fail)?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "VPN".to_string());
    if is_wireguard(&text) { return wgconf::parse(&text, &name); }
    let uuid = new_uuid()?;
    let base_dir = path.parent().unwrap_or(Path::new("/"));
    let mut imported = ovpn::parse(&text, &name, base_dir, &files_dir(&uuid)?)?;
    imported.profile.uuid = uuid;
    Ok(imported)
}

/// Whether `text` is a WireGuard configuration: it has an `[Interface]` section.
fn is_wireguard(text: &str) -> bool {
    text.lines().any(|l| l.trim().eq_ignore_ascii_case("[interface]"))
}

/// Write the files of an import, readable only by the user. Their folder is
/// created when missing.
pub fn write_files(files: &[PendingFile]) -> Result<(), NetError> {
    for f in files {
        let fail = |e: std::io::Error| NetError::other(format!("{}: {e}", f.path.display()));
        if let Some(dir) = f.path.parent() {
            fs::DirBuilder::new().recursive(true).mode(DIR_MODE).create(dir).map_err(fail)?;
        }
        let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(FILE_MODE).open(&f.path).map_err(fail)?;
        file.write_all(f.contents.as_bytes()).map_err(fail)?;
    }
    Ok(())
}

/// Remove the files folder of the profile `uuid`; no folder is not an error.
pub fn remove_files(uuid: &str) -> Result<(), NetError> {
    // A UUID never holds a path separator; refuse one so nothing outside
    // the folder can be removed.
    if uuid.is_empty() || uuid.contains(['/', '.']) { return Err(NetError::other(format!("{uuid} is not a profile UUID"))); }
    let dir = files_dir(uuid)?;
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(NetError::other(format!("{}: {e}", dir.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileKind;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn files_are_written_for_the_user_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("u1").join("ca.pem");
        write_files(&[PendingFile { path: path.clone(), contents: "pem".into() }]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "pem");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, FILE_MODE);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, DIR_MODE);
    }

    #[test]
    fn the_kind_of_file_is_found_from_its_text() {
        let tmp = tempfile::tempdir().unwrap();
        let wg = tmp.path().join("home.conf");
        fs::write(&wg, "[Interface]\nPrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\n[Peer]\nPublicKey = HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=\n").unwrap();
        assert_eq!(import_file(&wg).unwrap().profile.kind(), ProfileKind::WireGuard);
        let ovpn = tmp.path().join("work.ovpn");
        fs::write(&ovpn, "remote vpn.example.com 1194\n<ca>\nx\n</ca>\n").unwrap();
        let imported = import_file(&ovpn).unwrap();
        assert_eq!(imported.profile.kind(), ProfileKind::OpenVpn);
        assert_eq!(imported.profile.id, "work");
        assert!(imported.files[0].path.to_string_lossy().contains(&imported.profile.uuid));
    }

    #[test]
    fn a_missing_file_names_itself() {
        assert!(import_file(Path::new("/nonexistent/x.ovpn")).unwrap_err().text().contains("/nonexistent/x.ovpn"));
    }

    #[test]
    fn removing_refuses_anything_but_a_uuid() {
        assert!(remove_files("../x").is_err());
        assert!(remove_files("").is_err());
        assert!(remove_files("0000-not-there").is_ok());
    }

    #[test]
    fn debug_shows_only_the_path() {
        let f = PendingFile { path: "/a/key.pem".into(), contents: "PRIVATE".into() };
        assert!(!format!("{f:?}").contains("PRIVATE"));
    }
}
