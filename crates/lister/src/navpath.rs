//! Where "parent" and "root" lead from a folder (F-44): local folders,
//! folders inside an archive and remote folders.

use crate::archive::{make_archive_path, parse_archive_path};
use crate::remote;
use std::path::{Path, PathBuf};

/// The folder above `cwd`; `None` at the top. Inside an archive that is the
/// enclosing folder of the archive's inner folder, and from the archive's
/// root the folder that holds the archive file.
pub fn parent(cwd: &Path) -> Option<PathBuf> {
    if let Some((archive, inner)) = parse_archive_path(cwd) {
        if inner.is_empty() {
            return archive.parent().map(PathBuf::from);
        }
        let up = Path::new(&inner).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        return Some(make_archive_path(&archive, &up));
    }
    remote::parent_of(cwd)
}

/// The top of the tree `cwd` is in: `/` for a local folder, the archive's
/// root, or the server's root.
pub fn root(cwd: &Path) -> PathBuf {
    if let Some((archive, _)) = parse_archive_path(cwd) {
        return make_archive_path(&archive, "");
    }
    match remote::url_of(cwd) {
        Some(mut url) => {
            url.path = "/".to_string();
            remote::path_of(&url)
        }
        None => PathBuf::from("/"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn local_parent_and_root() {
        assert_eq!(parent(Path::new("/a/b")), Some(p("/a")));
        assert_eq!(parent(Path::new("/")), None);
        assert_eq!(root(Path::new("/a/b")), p("/"));
    }

    #[test]
    fn archive_parent_climbs_inside_then_out() {
        assert_eq!(parent(Path::new("/d/x.zip#a/b")), Some(p("/d/x.zip#a")));
        assert_eq!(parent(Path::new("/d/x.zip#a")), Some(p("/d/x.zip#")));
        assert_eq!(parent(Path::new("/d/x.zip#")), Some(p("/d")));
        assert_eq!(root(Path::new("/d/x.zip#a/b")), p("/d/x.zip#"));
    }

    #[test]
    fn remote_parent_and_root() {
        assert_eq!(parent(Path::new("sftp://u@h/a/b")), Some(p("sftp://u@h/a")));
        assert_eq!(parent(Path::new("sftp://u@h/")), None);
        assert_eq!(root(Path::new("sftp://u@h:2200/a/b")), p("sftp://u@h:2200/"));
    }
}
