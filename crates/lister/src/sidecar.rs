//! Sidecar: an `opusdm-viewer --sidecar` that follows this Lister. Every
//! change of the selection (or of the cursor row when nothing is selected)
//! writes the new paths to the viewer's standard input. Closing the pipe, by
//! toggling it off or by closing the Lister, closes the viewer.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::sidecar::{encode, SIDECAR_FLAG};
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

/// Launch the viewer with a pipe on its stdin and send it the selection.
fn start(ui: &Rc<Ui>) {
    let exe = std::env::current_exe().ok();
    let mut cmd = crate::image_viewer::viewer_command(exe.as_deref().and_then(Path::parent), &[]);
    cmd.arg(SIDECAR_FLAG).stdin(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return crate::actions::flash(ui, &format!("cannot start {}: {e}", crate::image_viewer::VIEWER_BIN)),
    };
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return crate::actions::flash(ui, "cannot talk to the Sidecar: no stdin pipe");
    };
    *ui.sidecar.borrow_mut() = Some(Sidecar { child, stdin, last: None });
    follow(ui);
    let window = ui.window.clone();
    glib::timeout_add_local_once(Duration::from_millis(SIDECAR_REFOCUS_MS), move || window.present());
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
}
