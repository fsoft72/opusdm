//! Extract archive: unpack the selected archives into the DEST Lister, or,
//! after a confirmation, into the folder of this Lister.

use crate::window::Ui;
use opusdm_ipc::{JobOptions, Op};
use std::path::PathBuf;
use std::rc::Rc;

/// Keep only the real archives `Extract` can unpack: not inside another
/// archive and with a supported suffix.
pub fn extractable(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|p| crate::archive::parse_archive_path(p).is_none())
        .filter(|p| p.file_name().is_some_and(|n| opusdm_fsops::extract::is_extractable(&n.to_string_lossy())))
        .collect()
}

/// Extract `srcs` into the DEST Lister. With no DEST, ask whether to extract
/// into the current folder of this (SOURCE) Lister instead.
pub fn open_extract(ui: &Rc<Ui>, srcs: Vec<PathBuf>) {
    let srcs = extractable(srcs);
    if srcs.is_empty() {
        crate::actions::flash(ui, "no supported archive selected");
        return;
    }
    if crate::link_dialogs::dest_or_cwd(ui).is_none() {
        crate::job_submit::submit_job(ui, Op::Extract, srcs, None, JobOptions::default());
        return;
    }
    let cwd = ui.cwd_path();
    let message = format!("No DEST Lister set. Extract into the current folder?\n{}", cwd.display());
    let ui = ui.clone();
    opusdm_ui::requester(&ui.window.clone(), "Request", &message, None, &[("Extract", true), ("Cancel", false)], move |ok| {
        if !ok {
            return;
        }
        crate::job_submit::submit_job(&ui, Op::Extract, srcs.clone(), Some(cwd.clone()), JobOptions::default());
    });
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
