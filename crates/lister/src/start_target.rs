//! What the Lister opens at start: a folder, or a file shown in its folder.

use std::path::PathBuf;

/// The folder to open and the names to select in it once it is listed.
#[derive(Debug, PartialEq, Eq)]
pub struct StartTarget {
    pub dir: PathBuf,
    pub select: Vec<String>,
}

/// Resolve the first command line argument. A folder opens as is; a file
/// opens its parent folder with the file selected; anything else (missing
/// or unusable) falls back to `home`, then to `/`.
pub fn resolve(arg: Option<PathBuf>, home: Option<PathBuf>) -> StartTarget {
    if let Some(path) = arg {
        if path.is_dir() {
            return StartTarget { dir: path, select: Vec::new() };
        }
        let parent = path.parent().filter(|p| p.is_dir());
        if let (true, Some(parent), Some(name)) = (path.exists(), parent, path.file_name()) {
            return StartTarget { dir: parent.to_path_buf(), select: vec![name.to_string_lossy().into_owned()] };
        }
    }
    StartTarget { dir: home.unwrap_or_else(|| PathBuf::from("/")), select: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_opens_as_is() {
        let td = tempfile::tempdir().unwrap();
        let t = resolve(Some(td.path().to_path_buf()), None);
        assert_eq!(t, StartTarget { dir: td.path().to_path_buf(), select: vec![] });
    }

    #[test]
    fn a_file_opens_its_folder_with_the_file_selected() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("report.pdf");
        std::fs::write(&file, b"x").unwrap();
        let t = resolve(Some(file), None);
        assert_eq!(t, StartTarget { dir: td.path().to_path_buf(), select: vec!["report.pdf".to_string()] });
    }

    #[test]
    fn a_missing_path_falls_back_to_home() {
        let td = tempfile::tempdir().unwrap();
        let t = resolve(Some(td.path().join("gone.txt")), Some(PathBuf::from("/home/x")));
        assert_eq!(t, StartTarget { dir: PathBuf::from("/home/x"), select: vec![] });
    }

    #[test]
    fn no_argument_and_no_home_is_the_root() {
        assert_eq!(resolve(None, None).dir, PathBuf::from("/"));
    }
}
