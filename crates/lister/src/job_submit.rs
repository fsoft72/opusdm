//! Job submission from the Lister UI. Every Copy, Move and Delete goes
//! through [`submit_job`], which honours the `recursive_filter` setting:
//! when it is on and a source is a directory, the user is asked for a file
//! pattern first and only matching files are affected.

use crate::window::Ui;
use opusdm_ipc::{JobOptions, Op, ToHub};
use std::path::PathBuf;
use std::rc::Rc;

/// Whether a job must ask for a filter pattern first: the setting is on,
/// the operation walks trees, and at least one source is a directory.
pub fn wants_filter(recursive_filter: bool, op: &Op, srcs: &[PathBuf]) -> bool {
    let walks_trees = matches!(
        op,
        Op::Copy
            | Op::Move
            | Op::Delete
            | Op::CopyAs { .. }
            | Op::MoveAs { .. }
            | Op::Duplicate
            | Op::Chmod { recurse: true, .. }
            | Op::Touch { recurse: true, .. }
    );
    recursive_filter && walks_trees && srcs.iter().any(|s| s.is_dir())
}

/// Send `op` to the hub. With `recursive_filter` on and a directory among
/// `srcs`, a pattern requester comes first: OK with a pattern filters, OK
/// with an empty pattern runs unfiltered, Cancel drops the job.
pub fn submit_job(
    ui: &Rc<Ui>,
    op: Op,
    srcs: Vec<PathBuf>,
    dest_dir: Option<PathBuf>,
    options: JobOptions,
) {
    submit_tagged_job(ui, op, srcs, dest_dir, options, 0);
}

/// [`submit_job`] with a `RunJob` tag, which the hub echoes in every
/// `JobDone` of the request (`0` = untagged). A tag from
/// [`crate::archive::keep_scratch`] has its scratch folder released when
/// the user cancels the pattern requester, as no job will ever end for it.
pub fn submit_tagged_job(
    ui: &Rc<Ui>,
    op: Op,
    srcs: Vec<PathBuf>,
    dest_dir: Option<PathBuf>,
    options: JobOptions,
    tag: u64,
) {
    ui.remember_selection();
    let enabled = ui.hub.settings.borrow().behavior.recursive_filter;
    if !wants_filter(enabled, &op, &srcs) {
        send(ui, op, srcs, dest_dir, options, tag);
        return;
    }
    let ui_clone = ui.clone();
    opusdm_ui::requester_entry(
        &ui.window,
        "Only files matching (e.g. *.rs;*.toml), empty = all files",
        "",
        &[("OK", true), ("Cancel", false)],
        move |ok, text| {
            if !ok {
                crate::archive::release_scratch(tag);
                return;
            }
            let mut options = options.clone();
            let pattern = text.trim();
            if !pattern.is_empty() {
                options.include_glob = Some(pattern.to_string());
            }
            send(&ui_clone, op.clone(), srcs.clone(), dest_dir.clone(), options, tag);
        },
    );
}

/// Put the job on the wire.
fn send(ui: &Rc<Ui>, op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>, options: JobOptions, tag: u64) {
    ui.send_hub(&ToHub::RunJob { op, srcs, dest_dir, options, tag });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_only_when_enabled_and_a_directory_is_involved() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().to_path_buf();
        let file = td.path().join("f");
        std::fs::write(&file, "x").unwrap();

        assert!(wants_filter(true, &Op::Copy, std::slice::from_ref(&dir)));
        assert!(wants_filter(true, &Op::Delete, &[file.clone(), dir.clone()]));
        assert!(!wants_filter(false, &Op::Copy, std::slice::from_ref(&dir)));
        assert!(!wants_filter(true, &Op::Copy, &[file]));
        assert!(wants_filter(true, &Op::Chmod { mode: 0o644, recurse: true }, std::slice::from_ref(&dir)));
        assert!(!wants_filter(true, &Op::Chmod { mode: 0o644, recurse: false }, std::slice::from_ref(&dir)));
        assert!(!wants_filter(true, &Op::MakeDir { name: "x".into() }, &[dir]));
    }
}
