//! Extract archive: unpack the selected archives into the DEST Lister, or,
//! after a confirmation, into the folder of the host.

use crate::host::FileActionHost;
use opusdm_ipc::Op;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Keep only the real archives `Extract` can unpack: not inside another
/// archive (`inside_archive`) and with a supported suffix.
pub fn extractable(paths: Vec<PathBuf>, inside_archive: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|p| !inside_archive(p))
        .filter(|p| p.file_name().is_some_and(|n| opusdm_fsops::extract::is_extractable(&n.to_string_lossy())))
        .collect()
}

/// Extract `srcs` into the DEST Lister. With no DEST, ask whether to extract
/// into the folder of the host instead.
pub fn open_extract(host: &Rc<dyn FileActionHost>, srcs: Vec<PathBuf>) {
    let srcs = extractable(srcs, |p| host.inside_archive(p));
    if srcs.is_empty() {
        host.notify("no supported archive selected");
        return;
    }
    if crate::host::dest_or_folder(&**host).is_none() {
        host.submit_job(Op::Extract, srcs, None);
        return;
    }
    let cwd = host.folder();
    let message = format!("No DEST Lister set. Extract into the current folder?\n{}", cwd.display());
    let host = host.clone();
    opusdm_ui::requester(&host.parent_window(), "Request", &message, None, &[("Extract", true), ("Cancel", false)], move |ok| {
        if !ok {
            return;
        }
        host.submit_job(Op::Extract, srcs.clone(), Some(cwd.clone()));
    });
}

/// Extract here: unpack `srcs` into the folder of the host, with
/// no question (an archive without one top folder gets its own folder, see
/// `opusdm_fsops::extract`).
pub fn extract_here(host: &Rc<dyn FileActionHost>, srcs: Vec<PathBuf>) {
    let srcs = extractable(srcs, |p| host.inside_archive(p));
    if srcs.is_empty() {
        host.notify("no supported archive selected");
        return;
    }
    let cwd = host.folder();
    host.submit_job(Op::Extract, srcs, Some(cwd));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_supported_archives_are_kept() {
        let kept = extractable(vec![PathBuf::from("/x/a.zip"), PathBuf::from("/x/b.txt"), PathBuf::from("/x/c.tar.gz")], |_| false);
        assert_eq!(kept, vec![PathBuf::from("/x/a.zip"), PathBuf::from("/x/c.tar.gz")]);
    }

    #[test]
    fn an_archive_inside_an_archive_is_not_kept() {
        let kept = extractable(vec![PathBuf::from("/x/a.zip"), PathBuf::from("/x/a.zip#b.zip")], |p| p.to_string_lossy().contains('#'));
        assert_eq!(kept, vec![PathBuf::from("/x/a.zip")]);
    }
}
