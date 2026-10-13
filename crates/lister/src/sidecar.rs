//! Sidecar: an `opusdm-viewer --sidecar` that follows this Lister. Every
//! change of the selection (or of the cursor row when nothing is selected)
//! writes the new paths to the viewer's standard input. Closing the pipe, by
//! toggling it off or by closing the Lister, closes the viewer. The viewer
//! writes back the file on screen, which Delete takes instead of the selection.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::sidecar::{encode, read_update, SIDECAR_FLAG};
use opusdm_ipc::{JobOptions, Op};
use std::cell::RefCell;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::rc::Rc;
use std::time::Duration;

/// Milliseconds before the Lister takes the keyboard back from a new Sidecar,
/// so the arrows keep walking the list.
const SIDECAR_REFOCUS_MS: u64 = 400;

/// A running Sidecar viewer.
pub struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    /// The paths of the last update, so an unchanged selection is not resent.
    last: Option<Vec<PathBuf>>,
    /// The file the viewer last said is on screen.
    shown: Rc<RefCell<Option<PathBuf>>>,
    /// Send the paths again when the running delete is done: the file went
    /// from a folder the viewer shows, which the Lister does not list.
    resend_after_job: bool,
}

impl Sidecar {
    /// Write `paths` unless they are what the viewer already shows.
    fn send(&mut self, paths: Vec<PathBuf>) -> io::Result<()> {
        if self.last.as_ref() == Some(&paths) { return Ok(()); }
        self.stdin.write_all(&encode(&paths))?;
        self.stdin.flush()?;
        self.last = Some(paths);
        Ok(())
    }

    /// True while the viewer process runs.
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Close the pipe (the viewer closes its window) and reap the process.
    fn close(self) {
        let Sidecar { mut child, stdin, .. } = self;
        drop(stdin);
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Start the Sidecar, or close the one that is open.
pub fn toggle(ui: &Rc<Ui>) {
    let old = ui.sidecar.borrow_mut().take();
    if let Some(mut old) = old {
        let alive = old.alive();
        old.close();
        if alive { return crate::actions::flash(ui, "Sidecar closed"); }
    }
    start(ui);
}

/// Close the Sidecar, if there is one: the Lister is going away.
pub fn stop(ui: &Ui) {
    if let Some(sidecar) = ui.sidecar.borrow_mut().take() { sidecar.close(); }
}

/// Send the current selection to the Sidecar, if one is open. A viewer the
/// user closed is forgotten.
pub fn follow(ui: &Ui) {
    let mut slot = ui.sidecar.borrow_mut();
    let Some(sidecar) = slot.as_mut() else { return };
    let cursor = ui.row_at(ui.view.cursor.get()).map(|r| ui.cwd_path().join(&r.name));
    if let Err(e) = sidecar.send(sidecar_paths(ui.selected_paths(), cursor)) {
        crate::dlog!("sidecar gone: {e}");
        if let Some(gone) = slot.take() { gone.close(); }
    }
}

/// Launch the viewer with pipes on its stdin and stdout and send it the selection.
fn start(ui: &Rc<Ui>) {
    let exe = std::env::current_exe().ok();
    let mut cmd = crate::image_viewer::viewer_command(exe.as_deref().and_then(Path::parent), &[]);
    cmd.arg(SIDECAR_FLAG).stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return crate::actions::flash(ui, &format!("cannot start {}: {e}", crate::image_viewer::VIEWER_BIN)),
    };
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return crate::actions::flash(ui, "cannot talk to the Sidecar: no stdin pipe");
    };
    let shown = Rc::new(RefCell::new(None));
    if let Some(stdout) = child.stdout.take() { _read_shown(stdout, shown.clone()); }
    *ui.sidecar.borrow_mut() = Some(Sidecar { child, stdin, last: None, shown, resend_after_job: false });
    follow(ui);
    let window = ui.window.clone();
    glib::timeout_add_local_once(Duration::from_millis(SIDECAR_REFOCUS_MS), move || window.present());
}

/// Keep `shown` in step with what the viewer reports on its stdout, until it exits.
fn _read_shown(stdout: std::process::ChildStdout, shown: Rc<RefCell<Option<PathBuf>>>) {
    let (tx, rx) = async_channel::unbounded::<Option<PathBuf>>();
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(stdout);
        while let Ok(Some(paths)) = read_update(&mut reader) {
            if tx.send_blocking(paths.into_iter().next()).is_err() { break; }
        }
    });
    glib::spawn_future_local(async move {
        while let Ok(path) = rx.recv().await {
            *shown.borrow_mut() = path;
        }
    });
}

/// The file the Sidecar shows, when it surely belongs to the last update sent
/// (the viewer may not have answered the newest update yet).
fn _shown_file(ui: &Ui) -> Option<PathBuf> {
    let mut slot = ui.sidecar.borrow_mut();
    let sidecar = slot.as_mut()?;
    if !sidecar.alive() { return None; }
    let shown = sidecar.shown.borrow().clone();
    pick_shown(shown.as_deref(), sidecar.last.as_deref()?)
}

/// The file to delete: the one the viewer reported when it is one of the
/// paths `sent` or inside one of them, else the only file sent, else none.
pub fn pick_shown(shown: Option<&Path>, sent: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = shown.filter(|p| sent.iter().any(|s| s == p || p.parent() == Some(s.as_path()))) {
        return Some(p.to_path_buf());
    }
    match sent {
        [one] if one.is_file() => Some(one.clone()),
        _ => None,
    }
}

/// Delete with a Sidecar open: only the file the viewer shows goes to the
/// trash (or is deleted for good when `permanent`), then the Lister and the
/// viewer move on to the next one. False when there is no
/// Sidecar or it shows nothing: the caller deletes the selection as usual.
pub fn delete_shown(ui: &Rc<Ui>, permanent: bool) -> bool {
    let Some(target) = _shown_file(ui) else { return false };
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (message, button, op) = if permanent {
        (format!("Delete \"{name}\" permanently?"), "Delete", Op::Delete)
    } else {
        (format!("Move \"{name}\" to the trash?"), "Trash", Op::Trash)
    };
    let this = ui.clone();
    opusdm_ui::requester(&ui.window, "Request", &message, None, &[(button, true), ("Cancel", false)], move |confirmed| {
        if !confirmed { return; }
        _step_past(&this, &target);
        crate::job_submit::submit_job(&this, op.clone(), vec![target.clone()], Some(this.cwd_path()), JobOptions::default());
    });
    true
}

/// Move the Lister off `target` before it is deleted, so the reload after
/// the job keeps the place: the next row (the previous one at the end) when
/// it is the only file shown, just its own row out of a bigger selection, and
/// for a file inside a shown folder a fresh update once the job is done.
fn _step_past(ui: &Rc<Ui>, target: &Path) {
    let row = (target.parent() == Some(ui.cwd_path().as_path())).then(|| _row_position(ui, target)).flatten();
    let Some(pos) = row else {
        if let Some(sidecar) = ui.sidecar.borrow_mut().as_mut() { sidecar.resend_after_job = true; }
        return;
    };
    if ui.view.selection.is_selected(pos) && ui.selected_paths().len() > 1 {
        ui.view.selection.unselect_item(pos);
        return;
    }
    let n = ui.view.selection.n_items();
    let next = if pos + 1 < n { pos + 1 } else if pos > 0 { pos - 1 } else { return };
    ui.jump_to_row(next);
}

/// Position of the row of `path` (a file of the current folder) in the list.
fn _row_position(ui: &Ui, path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_string_lossy();
    (0..ui.view.selection.n_items()).find(|&i| ui.row_at(i).is_some_and(|r| r.name == name))
}

/// A job finished: send the paths again when a delete asked for it.
pub fn job_done(ui: &Ui) {
    {
        let mut slot = ui.sidecar.borrow_mut();
        let Some(sidecar) = slot.as_mut().filter(|s| s.resend_after_job) else { return };
        sidecar.resend_after_job = false;
        sidecar.last = None;
    }
    follow(ui);
}

/// What the Sidecar shows: the selected paths, else the cursor row, keeping
/// only what is on the local disk (not entries inside archives or on servers).
pub fn sidecar_paths(selected: Vec<PathBuf>, cursor: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = if selected.is_empty() { cursor.into_iter().collect() } else { selected };
    paths.retain(|p| p.exists());
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selection_wins_over_the_cursor_and_missing_paths_are_dropped() {
        let td = tempfile::tempdir().unwrap();
        let (a, b) = (td.path().join("a.png"), td.path().join("b.png"));
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        let gone = td.path().join("gone.png");
        assert_eq!(sidecar_paths(vec![a.clone(), gone.clone()], Some(b.clone())), vec![a.clone()]);
        assert_eq!(sidecar_paths(Vec::new(), Some(b.clone())), vec![b]);
        assert_eq!(sidecar_paths(Vec::new(), Some(gone)), Vec::<PathBuf>::new());
        assert_eq!(sidecar_paths(Vec::new(), None), Vec::<PathBuf>::new());
    }

    #[test]
    fn delete_takes_the_shown_file_only_when_it_matches_what_was_sent() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("pics");
        std::fs::create_dir(&dir).unwrap();
        let (a, b, inner) = (td.path().join("a.png"), td.path().join("b.png"), dir.join("in.png"));
        for f in [&a, &b, &inner] { std::fs::write(f, "").unwrap(); }
        let both = vec![a.clone(), b.clone()];
        assert_eq!(pick_shown(Some(&b), &both), Some(b.clone()), "shown file in the gallery");
        assert_eq!(pick_shown(Some(&inner), std::slice::from_ref(&dir)), Some(inner.clone()), "inside a shown folder");
        assert_eq!(pick_shown(Some(&a), std::slice::from_ref(&b)), Some(b.clone()), "stale answer: the one file sent");
        assert_eq!(pick_shown(Some(&a), std::slice::from_ref(&dir)), None, "stale answer and a folder sent");
        assert_eq!(pick_shown(None, &both), None, "no answer and several files");
        assert_eq!(pick_shown(None, &[]), None);
    }
}
