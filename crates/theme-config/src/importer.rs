//! Theme archive import: extract a `.zip`/`.tar.gz`/`.tar.bz2` into a temp
//! dir, validate it's a theme, then move it into
//! `~/.config/opusdm/themes/<name>/`. All-or-nothing - `themes/` is only
//! ever touched on full success.

use opusdm_ipc::themes::{theme_dir, themes_dir, ThemeManifest};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug)]
pub enum ImportError {
    UnsupportedExtension,
    ExtractFailed(String),
    PathTraversal(String),
    MissingManifest,
    InvalidManifest(String),
    Io(std::io::Error),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::UnsupportedExtension => {
                write!(f, "unsupported file type (expected .zip, .tar.gz or .tar.bz2)")
            }
            ImportError::ExtractFailed(e) => write!(f, "could not read archive: {e}"),
            ImportError::PathTraversal(entry) => {
                write!(f, "archive contains an unsafe path: {entry}")
            }
            ImportError::MissingManifest => write!(f, "not a theme archive (no manifest.json at the root)"),
            ImportError::InvalidManifest(e) => write!(f, "invalid manifest.json: {e}"),
            ImportError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        ImportError::Io(e)
    }
}

/// True if `entry_name` (a `/`-separated archive path) is safe to extract
/// under an extraction root: no absolute path, no `..` component.
fn is_safe_entry(entry_name: &str) -> bool {
    let path = Path::new(entry_name);
    if path.is_absolute() {
        return false;
    }
    !path.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Import the archive at `archive_path`. Returns the installed theme's
/// slug on success.
pub fn import_theme_archive(archive_path: &Path) -> Result<String, ImportError> {
    let name = archive_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let staging = tempfile::tempdir()?;

    if name.ends_with(".zip") {
        extract_zip(archive_path, staging.path())?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive_path, staging.path())?;
    } else if name.ends_with(".tar.bz2") {
        extract_tar_bz2(archive_path, staging.path())?;
    } else {
        return Err(ImportError::UnsupportedExtension);
    }

    let manifest_path = staging.path().join("manifest.json");
    let content = std::fs::read_to_string(&manifest_path).map_err(|_| ImportError::MissingManifest)?;
    let manifest: ThemeManifest =
        serde_json::from_str(&content).map_err(|e| ImportError::InvalidManifest(e.to_string()))?;
    if !opusdm_ipc::themes::valid_slug(&manifest.name) {
        return Err(ImportError::InvalidManifest(format!("invalid name: {:?}", manifest.name)));
    }
    if manifest.label.trim().is_empty() {
        return Err(ImportError::InvalidManifest("label is empty".to_string()));
    }

    std::fs::create_dir_all(themes_dir())?;
    let dest = theme_dir(&manifest.name);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    // `rename` across filesystems can fail (EXDEV) - tempfile's default
    // temp dir is usually the same filesystem as $XDG_CONFIG_HOME under
    // $HOME, but fall back to a recursive copy if not.
    if std::fs::rename(staging.path(), &dest).is_err() {
        copy_dir_recursive(staging.path(), &dest)?;
    }

    Ok(manifest.name)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

fn extract_zip(archive_path: &Path, dest: &Path) -> Result<(), ImportError> {
    let file = File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
        let name = entry.name().to_string();
        if !is_safe_entry(&name) {
            return Err(ImportError::PathTraversal(name));
        }
        let out_path = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out_file = File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out_file)?;
    }
    Ok(())
}

// Threat model: this guards against a malicious *path* (zip-slip / tar-slip)
// escaping the extraction root. It does not attempt to sanitize symlink or
// hardlink entries inside a tarball - `tar::Entry::unpack` handles those
// per the `tar` crate's own defaults. Theme archives are user-supplied and
// imported locally, not processed as untrusted server-side uploads.
fn extract_tar_entries<R: Read>(mut archive: tar::Archive<R>, dest: &Path) -> Result<(), ImportError> {
    for entry in archive.entries().map_err(|e| ImportError::ExtractFailed(e.to_string()))? {
        let mut entry = entry.map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
        let path = entry.path().map_err(|e| ImportError::ExtractFailed(e.to_string()))?.into_owned();
        let name = path.to_string_lossy().to_string();
        if !is_safe_entry(&name) {
            return Err(ImportError::PathTraversal(name));
        }
        let out_path = dest.join(&path);
        entry.unpack(&out_path).map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
    }
    Ok(())
}

fn extract_tar_gz(archive_path: &Path, dest: &Path) -> Result<(), ImportError> {
    let file = File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    extract_tar_entries(tar::Archive::new(decoder), dest)
}

fn extract_tar_bz2(archive_path: &Path, dest: &Path) -> Result<(), ImportError> {
    let file = File::open(archive_path)?;
    let decoder = bzip2::read::BzDecoder::new(file);
    extract_tar_entries(tar::Archive::new(decoder), dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use std::io::Write;

    fn build_zip(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new().suffix(".zip").tempfile().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        file
    }

    fn valid_theme_entries() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("manifest.json", br#"{"name":"my-theme","label":"My Theme"}"#),
            ("icons/dir.png", b"fake-png"),
        ]
    }

    #[test]
    fn imports_a_valid_zip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&valid_theme_entries());

        let slug = import_theme_archive(archive.path()).unwrap();
        assert_eq!(slug, "my-theme");
        let installed = opusdm_ipc::themes::theme_dir("my-theme");
        assert!(installed.join("manifest.json").is_file());
        assert!(installed.join("icons").join("dir.png").is_file());
    }

    #[test]
    fn rejects_unsupported_extension() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let file = tempfile::Builder::new().suffix(".rar").tempfile().unwrap();
        assert!(matches!(import_theme_archive(file.path()), Err(ImportError::UnsupportedExtension)));
    }

    #[test]
    fn rejects_missing_manifest() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&[("icons/dir.png", b"x")]);
        assert!(matches!(import_theme_archive(archive.path()), Err(ImportError::MissingManifest)));
        assert!(
            !opusdm_ipc::themes::themes_dir().exists()
                || std::fs::read_dir(opusdm_ipc::themes::themes_dir()).unwrap().next().is_none()
        );
    }

    #[test]
    fn rejects_invalid_manifest_name() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&[("manifest.json", br#"{"name":"Bad Name","label":"x"}"#)]);
        assert!(matches!(import_theme_archive(archive.path()), Err(ImportError::InvalidManifest(_))));
    }

    #[test]
    fn rejects_a_zip_slip_entry_and_leaves_themes_dir_untouched() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = build_zip(&[
            ("manifest.json", br#"{"name":"evil","label":"Evil"}"#),
            ("../../evil.txt", b"pwned"),
        ]);
        assert!(matches!(import_theme_archive(archive.path()), Err(ImportError::PathTraversal(_))));
        assert!(!opusdm_ipc::themes::theme_dir("evil").exists());
    }

    #[test]
    fn reimporting_overwrites_the_existing_theme() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive1 = build_zip(&[("manifest.json", br#"{"name":"t","label":"V1"}"#)]);
        import_theme_archive(archive1.path()).unwrap();
        let archive2 = build_zip(&[("manifest.json", br#"{"name":"t","label":"V2"}"#)]);
        import_theme_archive(archive2.path()).unwrap();

        let manifest: opusdm_ipc::themes::ThemeManifest = serde_json::from_str(
            &std::fs::read_to_string(opusdm_ipc::themes::theme_dir("t").join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.label, "V2");
    }

    #[test]
    fn imports_a_tar_gz() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let file = tempfile::Builder::new().suffix(".tar.gz").tempfile().unwrap();
        {
            let enc = flate2::write::GzEncoder::new(file.reopen().unwrap(), flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let data = br#"{"name":"gz-theme","label":"Gz Theme"}"#;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, "manifest.json", &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let slug = import_theme_archive(file.path()).unwrap();
        assert_eq!(slug, "gz-theme");
    }

    #[test]
    fn imports_a_tar_bz2() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let file = tempfile::Builder::new().suffix(".tar.bz2").tempfile().unwrap();
        {
            let enc = bzip2::write::BzEncoder::new(file.reopen().unwrap(), bzip2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let data = br#"{"name":"bz2-theme","label":"Bz2 Theme"}"#;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, "manifest.json", &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let slug = import_theme_archive(file.path()).unwrap();
        assert_eq!(slug, "bz2-theme");
    }
}
