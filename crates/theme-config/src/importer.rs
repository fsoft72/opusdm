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

    prune_theme_assets(staging.path());
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

/// Unpack the entries of a tarball under `dest`. Guards against a malicious
/// *path* (tar-slip) and only unpacks regular files and folders: symlink,
/// hardlink and device entries are skipped with a log line.
fn extract_tar_entries<R: Read>(mut archive: tar::Archive<R>, dest: &Path) -> Result<(), ImportError> {
    for entry in archive.entries().map_err(|e| ImportError::ExtractFailed(e.to_string()))? {
        let mut entry = entry.map_err(|e| ImportError::ExtractFailed(e.to_string()))?;
        let path = entry.path().map_err(|e| ImportError::ExtractFailed(e.to_string()))?.into_owned();
        let name = path.to_string_lossy().to_string();
        if !is_safe_entry(&name) {
            return Err(ImportError::PathTraversal(name));
        }
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            opusdm_ipc::err_log!("theme-config", "import: skipping non-regular archive entry {name}");
            continue;
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

/// Remove from the staged theme every file that has no business in its
/// `fonts/` (only `.ttf` / `.otf`), `backgrounds/` or `wallpapers/` (only
/// picture types) folder. Skipped files are logged; the import itself still succeeds.
fn prune_theme_assets(root: &Path) {
    prune_dir(&root.join("fonts"), |p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
    });
    // `backgrounds/` also holds the start menu icon, which may be an SVG.
    prune_dir(&root.join("backgrounds"), |p| {
        opusdm_ipc::desktop::is_background_image(p) || p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    });
    prune_dir(&root.join(opusdm_ipc::themes::WALLPAPERS_DIR), opusdm_ipc::desktop::is_background_image);
}

/// Delete what is directly inside `dir` unless it is a regular file `keep`
/// accepts. Symbolic links (never followed) and subfolders are removed too.
/// A missing folder is fine.
fn prune_dir(dir: &Path, keep: impl Fn(&Path) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let kind = entry.file_type();
        if kind.as_ref().is_ok_and(|k| k.is_file()) && keep(&path) {
            continue;
        }
        opusdm_ipc::err_log!("theme-config", "import: dropping unexpected entry {}", path.display());
        let removed = match kind {
            Ok(k) if k.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(e) = removed {
            opusdm_ipc::err_log!("theme-config", "cannot remove {}: {e}", path.display());
        }
    }
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

    /// End-to-end check against the real, xtask-built `breeze-dark.zip` (not
    /// a synthetic fixture): import it into a scratch config dir and verify
    /// the result is scannable and its icons load through the same
    /// `opusdm_iconview` lookup the running app uses. Skipped if the zip
    /// hasn't been built yet (`cargo run -p xtask -- package-themes`).
    #[test]
    fn imports_the_real_breeze_dark_package_end_to_end() {
        let _guard = ENV_LOCK.lock().unwrap();
        let zip_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/themes/breeze-dark.zip");
        if !zip_path.is_file() {
            eprintln!("skipping: {} not built (run cargo run -p xtask -- package-themes)", zip_path.display());
            return;
        }

        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());

        let slug = import_theme_archive(&zip_path).unwrap();
        assert_eq!(slug, "breeze-dark");

        let found = opusdm_ipc::themes::scan_themes();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug, "breeze-dark");
        assert_eq!(found[0].label, "Breeze Dark");

        let set = opusdm_ipc::IconSetId::Custom("breeze-dark".to_string());
        let dir_icon = opusdm_iconview::icon_set::icon_bytes_for(
            opusdm_iconview::IconKind::Dir,
            0,
            None,
            &set,
        );
        assert!(dir_icon.is_some(), "dir icon should load through the real icon lookup path");
        assert!(dir_icon.unwrap().len() > 100, "should be real PNG bytes, not a stub");
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

    #[test]
    fn import_drops_files_that_are_not_fonts_or_pictures_from_those_folders() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let mut entries = valid_theme_entries();
        entries.extend([
            ("fonts/ok.ttf", &b"f"[..]),
            ("fonts/evil.sh", b"s"),
            ("backgrounds/ok.png", b"p"),
            ("backgrounds/hub-menu-icon.svg", b"<svg/>"),
            ("backgrounds/readme.txt", b"t"),
            ("sounds/a.wav", b"w"),
        ]);
        let zip = build_zip(&entries);

        let slug = import_theme_archive(zip.path()).unwrap();

        let dir = theme_dir(&slug);
        assert!(dir.join("fonts/ok.ttf").is_file());
        assert!(!dir.join("fonts/evil.sh").exists());
        assert!(dir.join("backgrounds/ok.png").is_file());
        assert!(dir.join("backgrounds/hub-menu-icon.svg").is_file());
        assert!(!dir.join("backgrounds/readme.txt").exists());
        assert!(dir.join("sounds/a.wav").is_file(), "sounds are an opaque container");
    }

    /// Build a `.tar.gz` holding a manifest and a symlink entry `fonts/x.ttf`.
    fn tar_gz_with_symlink() -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new().suffix(".tar.gz").tempfile().unwrap();
        let enc = flate2::write::GzEncoder::new(file.reopen().unwrap(), flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        let data = br#"{"name":"linky","label":"Linky"}"#;
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, "manifest.json", &data[..]).unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        builder.append_link(&mut link, "fonts/x.ttf", "/etc/passwd").unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        file
    }

    /// A symlink entry in a tarball is never unpacked into the installed theme.
    #[test]
    fn a_tar_symlink_entry_is_never_unpacked() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let archive = tar_gz_with_symlink();
        assert_eq!(import_theme_archive(archive.path()).unwrap(), "linky");
        assert!(std::fs::symlink_metadata(theme_dir("linky").join("fonts/x.ttf")).is_err(), "no link may be installed");
    }

    /// Pruning drops symlinks and subfolders from `fonts/` and `backgrounds/`,
    /// and anything but pictures from `wallpapers/`.
    #[test]
    fn prune_removes_symlinks_and_subfolders_from_fonts_and_backgrounds() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("fonts/sub")).unwrap();
        std::fs::create_dir_all(root.join("backgrounds")).unwrap();
        std::fs::write(root.join("fonts/ok.ttf"), b"f").unwrap();
        std::fs::write(root.join("fonts/sub/inner.ttf"), b"f").unwrap();
        let secret = root.join("secret.txt");
        std::fs::write(&secret, b"s").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("fonts/x.ttf")).unwrap();
        std::os::unix::fs::symlink(&secret, root.join("backgrounds/y.png")).unwrap();
        std::fs::create_dir_all(root.join("wallpapers")).unwrap();
        std::fs::write(root.join("wallpapers/w.jpg"), b"w").unwrap();
        std::fs::write(root.join("wallpapers/run.sh"), b"x").unwrap();
        prune_theme_assets(root);
        assert!(root.join("wallpapers/w.jpg").is_file());
        assert!(!root.join("wallpapers/run.sh").exists());
        assert!(root.join("fonts/ok.ttf").is_file());
        assert!(std::fs::symlink_metadata(root.join("fonts/x.ttf")).is_err());
        assert!(std::fs::symlink_metadata(root.join("backgrounds/y.png")).is_err());
        assert!(!root.join("fonts/sub").exists());
    }
}
