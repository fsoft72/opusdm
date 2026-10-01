//! Packs a theme folder into the `.zip` archive `opusdm-settings` imports.
//! One implementation shared by the app's Export and `xtask package-themes`.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

/// Zip the contents of `src` into `zip_path`. Entries use `/` separators and
/// are sorted by name, so the same folder always yields the same archive
/// layout. The folder itself is not an entry: `manifest.json` ends up at the
/// archive root, as the importer expects.
pub fn pack_theme_dir(src: &Path, zip_path: &Path) -> io::Result<()> {
    if !src.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} is not a folder", src.display())));
    }
    let mut zip = zip::ZipWriter::new(File::create(zip_path)?);
    let options = zip::write::SimpleFileOptions::default();
    for entry in walkdir::WalkDir::new(src).sort_by_file_name() {
        let entry = entry.map_err(io::Error::other)?;
        let rel = entry.path().strip_prefix(src).map_err(io::Error::other)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let name = rel.to_string_lossy().replace('\\', "/");
        if entry.file_type().is_dir() {
            zip.add_directory(format!("{name}/"), options).map_err(io::Error::other)?;
        } else {
            zip.start_file(name, options).map_err(io::Error::other)?;
            zip.write_all(&std::fs::read(entry.path())?)?;
        }
    }
    zip.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn packs_the_folder_contents_with_relative_slash_paths() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("icons")).unwrap();
        std::fs::write(src.path().join("manifest.json"), b"{}").unwrap();
        std::fs::write(src.path().join("icons/a.png"), b"png").unwrap();
        let out = tempfile::tempdir().unwrap();
        let zip_path = out.path().join("t.zip");

        pack_theme_dir(src.path(), &zip_path).unwrap();

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().to_string()).collect();
        assert!(names.contains(&"manifest.json".to_string()));
        assert!(names.contains(&"icons/a.png".to_string()));
        let mut body = String::new();
        archive.by_name("manifest.json").unwrap().read_to_string(&mut body).unwrap();
        assert_eq!(body, "{}");
    }

    #[test]
    fn a_missing_source_folder_is_an_error() {
        let out = tempfile::tempdir().unwrap();
        assert!(pack_theme_dir(std::path::Path::new("/no/such/theme"), &out.path().join("t.zip")).is_err());
    }
}
