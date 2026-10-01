//! "Export theme" (the BuildTheme equivalent): pack an installed theme folder
//! into a distributable `.zip`.

use opusdm_ipc::themes::theme_dir;
use std::path::Path;

/// Pack theme `slug` into the archive `dest`. The error text names the
/// failing path or the missing theme. A partial archive left behind by a
/// failed pack is removed.
pub fn export_theme(slug: &str, dest: &Path) -> Result<(), String> {
    let dir = theme_dir(slug);
    if !dir.join("manifest.json").is_file() {
        return Err(format!("theme \"{slug}\" is not installed"));
    }
    opusdm_themepack::pack_theme_dir(&dir, dest).map_err(|e| {
        // The packer only fails after (possibly) creating `dest`, so what is there is partial.
        let _ = std::fs::remove_file(dest);
        format!("cannot write {}: {e}", dest.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use opusdm_ipc::themes::theme_dir;

    #[test]
    fn exports_an_installed_theme_as_an_importable_zip() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let dir = theme_dir("pack-me");
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        std::fs::write(dir.join("manifest.json"), br#"{"name":"pack-me","label":"Pack Me"}"#).unwrap();
        std::fs::write(dir.join("icons/a.png"), b"x").unwrap();
        let zip = td.path().join("out.zip");

        export_theme("pack-me", &zip).unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(crate::importer::import_theme_archive(&zip).unwrap(), "pack-me");
        assert!(theme_dir("pack-me").join("icons/a.png").is_file());
    }

    #[test]
    fn exporting_an_unknown_theme_fails_with_a_message() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let err = export_theme("nope", &td.path().join("x.zip")).unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn a_failed_pack_leaves_no_partial_archive() {
        let _g = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let dir = theme_dir("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), br#"{"name":"broken"}"#).unwrap();
        // A dangling symlink makes reading the entry fail after the zip was created.
        std::os::unix::fs::symlink(td.path().join("missing"), dir.join("zz-dangling")).unwrap();
        let zip = td.path().join("out.zip");

        let err = export_theme("broken", &zip).unwrap_err();

        assert!(err.contains("cannot write"), "{err}");
        assert!(!zip.exists(), "partial archive must be removed");
    }
}
