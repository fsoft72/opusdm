//! Extract archive: unpack the selected archives into the DEST Lister, or,
//! after a confirmation, into the folder of this Lister. Shared with the
//! Desktop, see `opusdm_fileactions::extract`.

use crate::window::Ui;
use std::path::PathBuf;
use std::rc::Rc;

/// Keep only the real archives `Extract` can unpack: not inside another
/// archive and with a supported suffix.
pub fn extractable(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    opusdm_fileactions::extract::extractable(paths, |p| crate::archive::parse_archive_path(p).is_some())
}

/// Extract `srcs` into the DEST Lister. With no DEST, ask whether to extract
/// into the current folder of this (SOURCE) Lister instead.
pub fn open_extract(ui: &Rc<Ui>, srcs: Vec<PathBuf>) {
    opusdm_fileactions::extract::open_extract(&crate::file_host::ListerHost::rc(ui), srcs);
}

/// Extract here: unpack `srcs` into the current folder of this Lister, with
/// no question.
pub fn extract_here(ui: &Rc<Ui>, srcs: Vec<PathBuf>) {
    opusdm_fileactions::extract::extract_here(&crate::file_host::ListerHost::rc(ui), srcs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_supported_archives_are_kept() {
        let kept = extractable(vec![PathBuf::from("/x/a.zip"), PathBuf::from("/x/b.txt"), PathBuf::from("/x/c.tar.gz")]);
        assert_eq!(kept, vec![PathBuf::from("/x/a.zip"), PathBuf::from("/x/c.tar.gz")]);
    }
}
