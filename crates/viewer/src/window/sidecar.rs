//! Sidecar mode: the window follows the paths a Lister writes on stdin. Each
//! update replaces the gallery; the window keeps its size, only the stage and
//! the strip change. The file on screen goes back to the Lister on stdout (same
//! format, one path or none), so its Delete can take exactly that file.

use super::State;
use crate::gallery::Gallery;
use crate::kind::kind_of;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

/// Title of a Sidecar with nothing on screen.
const SIDECAR_TITLE: &str = "Sidecar";
/// What an empty Sidecar says before the first update.
pub(super) const SIDECAR_WAITING: &str = "Waiting for the Lister";
/// What it says when the Lister selected nothing it can show.
const SIDECAR_NOTHING: &str = "Nothing to show";

/// Read the updates on a thread and apply the newest on the main thread; the
/// window closes when the Lister goes away.
pub(super) fn follow_stdin(state: &Rc<State>) {
    let (tx, rx) = async_channel::unbounded::<Option<Vec<PathBuf>>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            let update = match opusdm_ipc::sidecar::read_update(&mut stdin) {
                Ok(update) => update,
                Err(e) => {
                    opusdm_ipc::err_log!("viewer", "cannot read the Sidecar updates: {e}");
                    None
                }
            };
            let done = update.is_none();
            if tx.send_blocking(update).is_err() || done { break; }
        }
    });
    let s = state.clone();
    glib::spawn_future_local(async move {
        while let Ok(mut update) = rx.recv().await {
            // Arrow keys in the Lister send bursts: only the newest one counts.
            while let Ok(newer) = rx.try_recv() { update = newer; }
            let Some(paths) = update else { break };
            s._replace_gallery(&paths);
        }
        s.window.close();
    });
}

/// The folders and the files of a known kind among `targets`: walking past an
/// archive or a disk image in the Lister must not decode it as a picture.
fn showable(targets: &[PathBuf]) -> Vec<PathBuf> {
    targets.iter().filter(|t| t.is_dir() || kind_of(t).is_some()).cloned().collect()
}

impl State {
    /// Show `targets` (files and folders) in place of the gallery of before.
    fn _replace_gallery(self: &Rc<Self>, targets: &[PathBuf]) {
        let mut gallery = Gallery::from_targets(&showable(targets)).unwrap_or_else(|_| Gallery::empty());
        gallery.keep_place(&self.gallery.borrow());
        self.strip.set_items(gallery.items());
        *self.gallery.borrow_mut() = gallery;
        self._sync_strip();
        if self.gallery.borrow().is_empty() { return self._show_nothing(SIDECAR_NOTHING); }
        self.show_current();
    }

    /// Clear the stage and say `message`, with no file on screen.
    pub(super) fn _show_nothing(&self, message: &str) {
        self.generation.set(self.generation.get() + 1);
        self._cancel_timer();
        self.video.stop();
        self.window.set_title(Some(SIDECAR_TITLE));
        self._pdf_file_changed();
        self._show_error(message);
        self.info.set_text(message);
        self._report_shown();
    }

    /// Tell the Lister which file is on screen. A Lister gone away is noticed
    /// on stdin, so a write error is ignored here.
    pub(super) fn _report_shown(&self) {
        if !self.sidecar { return; }
        let shown: Vec<PathBuf> = self.gallery.borrow().current().map(|p| p.to_path_buf()).into_iter().collect();
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(&opusdm_ipc::sidecar::encode(&shown)).and_then(|_| out.flush());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_folders_and_known_files_are_shown() {
        let td = tempfile::tempdir().unwrap();
        let targets: Vec<PathBuf> = ["a.png", "b.zip", "c.md"].iter().map(|n| td.path().join(n)).chain([td.path().to_path_buf()]).collect();
        assert_eq!(showable(&targets), vec![td.path().join("a.png"), td.path().join("c.md"), td.path().to_path_buf()]);
    }
}
