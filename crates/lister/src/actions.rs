//! The Lister button row and the dialogs it drives.
//!
//! Task 13 wires `window.rs` to these entry points; Task 14 fills the bodies.

use crate::client::Roles;
use crate::window::Ui;
use opusdm_ipc::{FileMeta, ListerId, Outcome, Resolution, ToHub};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Decide the destination directory for a Copy/Move from this Lister.
pub fn resolve_copy_target(
    roles: Roles,
    me: ListerId,
    _my_cwd: &Path,
    dest_cwd: Option<&Path>,
) -> Result<PathBuf, String> {
    if roles.source != Some(me) {
        return Err("this Lister is not the SOURCE".into());
    }
    match (roles.dest, dest_cwd) {
        (Some(d), _) if d == me => Err("SOURCE and DEST are the same Lister".into()),
        (Some(_), Some(p)) => Ok(p.to_path_buf()),
        _ => Err("no DEST Lister set".into()),
    }
}

/// Build the button row and wire every control. Populated in Task 14.
pub fn attach(_ui: &Rc<Ui>) {}

/// Sync the Source/Dest/Lock toggles from the hub's role state. Populated in Task 14.
pub fn sync_role_buttons(_ui: &Rc<Ui>) {}

/// Update the in-window job status line.
pub fn on_job_progress(ui: &Rc<Ui>, _job_id: u64, done: u64, total: u64, current: &Path) {
    let pct = if total > 0 { done * 100 / total } else { 0 };
    ui.job_line
        .set_text(&format!("{pct}%  {}", current.display()));
}

/// Finish a job: clear the line, reload the view.
pub fn on_job_done(ui: &Rc<Ui>, _job_id: u64, outcome: &Outcome, errors: &[(PathBuf, String)]) {
    let text = match outcome {
        Outcome::Error { message } => format!("job failed: {message}"),
        _ if !errors.is_empty() => format!("job completed with {} error(s)", errors.len()),
        _ => String::new(),
    };
    ui.job_line.set_text(&text);
    ui.reload();
}

/// Reply to an overwrite conflict. Populated in Task 14 (dialog); v13 auto-skips.
pub fn on_conflict(ui: &Rc<Ui>, req_id: u64, _path: &Path, _src: FileMeta, _dst: FileMeta) {
    let _ = ui
        .client
        .send(&ToHub::ConflictReply { req_id, resolution: Resolution::Skip });
}
