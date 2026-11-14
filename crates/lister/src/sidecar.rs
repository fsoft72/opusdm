//! Sidecar: an `opusdm-viewer --sidecar` that follows this Lister. Every
//! change of the selection (or of the cursor row when nothing is selected)
//! writes the new paths to the viewer's standard input. Closing the pipe, by
//! toggling it off or by closing the Lister, closes the viewer. The viewer
//! writes back the file on screen, which Delete takes instead of the selection.
//! Inside an archive the files are first extracted to temporary copies on a
//! worker thread, and the viewer gets the copies.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::sidecar::{encode, read_update, SIDECAR_FLAG};
use opusdm_ipc::{JobOptions, Op};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::rc::Rc;
use std::time::Duration;

/// Milliseconds before the Lister takes the keyboard back from a new Sidecar,
/// so the arrows keep walking the list.
const SIDECAR_REFOCUS_MS: u64 = 400;

/// At most this many files of an archive are extracted for one update.
const MAX_ARCHIVE_PREVIEWS: usize = 20;

/// Archive entries bigger than this are not extracted just to be previewed.
const MAX_ARCHIVE_PREVIEW_BYTES: u64 = 256 * 1024 * 1024;

thread_local! {
    /// Id of the last archive preview request, across every Sidecar of this
    /// process: only the answer to the newest request is shown.
    static LAST_ARCHIVE_REQUEST: Cell<u64> = const { Cell::new(0) };
}

/// One archive entry as listed: archive, inner path, size and date. The
/// size and the date are part of the key so a changed archive is extracted
/// again.
type EntryKey = (PathBuf, String, u64, i64);

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
    /// Temporary copies of the archive entries already extracted for it.
    previews: HashMap<EntryKey, PathBuf>,
    /// The archive preview request whose answer is awaited (0: none).
    pending: u64,
    /// True while an archive preview worker runs: one at a time, so walking
    /// a big archive does not start a worker per row.
    extracting: bool,
    /// The selection changed while the worker ran: follow it again after.
    queued: bool,
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
        let Sidecar { child, stdin, .. } = self;
        drop(stdin);
        crate::background::reap(child, "the Sidecar viewer");
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
/// user closed is forgotten. Inside an archive see [`_follow_archive`].
pub fn follow(ui: &Rc<Ui>) {
    if ui.sidecar.borrow().is_none() { return; }
    let cwd = ui.cwd_path();
    if let Some((arch, inner)) = crate::archive::parse_archive_path(&cwd) {
        return _follow_archive(ui, &arch, &inner);
    }
    let cursor = ui.row_at(ui.view.cursor.get()).map(|r| cwd.join(&r.name));
    _send(ui, sidecar_paths(ui.selected_paths(), cursor, &cwd));
}

/// Write `paths` to the open Sidecar; a viewer that is gone is forgotten.
fn _send(ui: &Ui, paths: Vec<PathBuf>) {
    let mut slot = ui.sidecar.borrow_mut();
    let Some(sidecar) = slot.as_mut() else { return };
    sidecar.pending = 0;
    if let Err(e) = sidecar.send(paths) {
        crate::dlog!("sidecar gone: {e}");
        if let Some(gone) = slot.take() { gone.close(); }
    }
}

/// The Lister shows the folder `inner` of the archive `arch`: send the
/// temporary copies of the selected files (else of the cursor file). Copies
/// extracted before are reused; the missing ones are extracted on a worker
/// thread and sent when they are ready, unless a newer update came first.
fn _follow_archive(ui: &Rc<Ui>, arch: &Path, inner: &str) {
    let mut rows: Vec<_> = ui.selected_positions().into_iter().filter_map(|i| ui.row_at(i)).collect();
    if rows.is_empty() { rows.extend(ui.row_at(ui.view.cursor.get())); }
    let keys = archive_preview_keys(arch, inner, rows.iter().map(|r| (r.name.as_str(), r.is_dir, r.size, r.modified_unix)));
    let missing: Vec<EntryKey> = {
        let slot = ui.sidecar.borrow();
        let Some(sidecar) = slot.as_ref() else { return };
        keys.iter().filter(|k| !sidecar.previews.contains_key(*k)).cloned().collect()
    };
    if missing.is_empty() { return _send_previews(ui, &keys); }
    let request = LAST_ARCHIVE_REQUEST.with(|last| { last.set(last.get() + 1); last.get() });
    {
        let mut slot = ui.sidecar.borrow_mut();
        let Some(sidecar) = slot.as_mut() else { return };
        if sidecar.extracting {
            sidecar.queued = true;
            return;
        }
        sidecar.extracting = true;
        sidecar.pending = request;
    }
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let done: Vec<(EntryKey, io::Result<PathBuf>)> =
            missing.into_iter().map(|k| { let copy = crate::archive::extract_single_file_temp(&k.0, &k.1); (k, copy) }).collect();
        let _ = tx.send_blocking(done);
    });
    // OWNER: the future holds a strong `Rc<Ui>` only until this one answer
    // arrives (the worker always sends it), like the double click extraction.
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let Ok(done) = rx.recv().await else {
            opusdm_ipc::err_log!("opusdm-lister", "Sidecar archive extraction ended without a result");
            return;
        };
        _previews_ready(&ui, request, &keys, done);
    });
}

/// The worker of request `request` extracted `done`: keep the copies and,
/// when the request is still the one awaited, send the copies of `keys`.
/// A selection change that came while the worker ran is followed now.
fn _previews_ready(ui: &Rc<Ui>, request: u64, keys: &[EntryKey], done: Vec<(EntryKey, io::Result<PathBuf>)>) {
    let (current, queued) = {
        let mut slot = ui.sidecar.borrow_mut();
        let Some(sidecar) = slot.as_mut() else {
            crate::dlog!("Sidecar closed before archive preview {request} was ready");
            return;
        };
        sidecar.extracting = false;
        for (key, copy) in done {
            match copy {
                Ok(path) => { sidecar.previews.insert(key, path); }
                Err(e) => opusdm_ipc::err_log!("opusdm-lister", "cannot extract {} from {} for the Sidecar: {e}", key.1, key.0.display()),
            }
        }
        (sidecar.pending == request, std::mem::take(&mut sidecar.queued))
    };
    if queued { return follow(ui); }
    if !current {
        crate::dlog!("archive preview {request} is out of date, not sent");
        return;
    }
    _send_previews(ui, keys);
}

/// Send the temporary copies of `keys` that exist.
fn _send_previews(ui: &Ui, keys: &[EntryKey]) {
    let paths: Vec<PathBuf> = {
        let slot = ui.sidecar.borrow();
        let Some(sidecar) = slot.as_ref() else { return };
        keys.iter().filter_map(|k| sidecar.previews.get(k).cloned()).collect()
    };
    _send(ui, paths);
}

/// The entries to preview among `rows` (name, is a folder, size, date) of the
/// folder `inner` of `arch`: files only (a folder of an archive has no copy
/// to show), none bigger than [`MAX_ARCHIVE_PREVIEW_BYTES`], at most
/// [`MAX_ARCHIVE_PREVIEWS`].
pub fn archive_preview_keys<'a>(arch: &Path, inner: &str, rows: impl Iterator<Item = (&'a str, bool, u64, i64)>) -> Vec<EntryKey> {
    let mut keys = Vec::new();
    for (name, is_dir, size, modified) in rows {
        if keys.len() >= MAX_ARCHIVE_PREVIEWS {
            crate::dlog!("Sidecar: only the first {MAX_ARCHIVE_PREVIEWS} files of the archive selection are previewed");
            break;
        }
        if is_dir { continue; }
        if size > MAX_ARCHIVE_PREVIEW_BYTES {
            crate::dlog!("Sidecar: {name} is too big ({size} bytes) to be extracted for a preview");
            continue;
        }
        let entry = if inner.is_empty() { name.to_string() } else { format!("{inner}/{name}") };
        keys.push((arch.to_path_buf(), entry, size, modified));
    }
    keys
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
    *ui.sidecar.borrow_mut() = Some(Sidecar { child, stdin, last: None, shown, resend_after_job: false, previews: HashMap::new(), pending: 0, extracting: false, queued: false });
    follow(ui);
    let window = ui.window.clone();
    glib::timeout_add_local_once(Duration::from_millis(SIDECAR_REFOCUS_MS), move || window.present());
}

/// Keep `shown` in step with what the viewer reports on its stdout, until it exits.
fn _read_shown(stdout: std::process::ChildStdout, shown: Rc<RefCell<Option<PathBuf>>>) {
    crate::background::run_streaming(
        &crate::background::CancelToken::new(),
        move |out| {
            let mut reader = io::BufReader::new(stdout);
            while let Ok(Some(paths)) = read_update(&mut reader) {
                if !out.emit(paths.into_iter().next()) { break; }
            }
        },
        move |path| *shown.borrow_mut() = path,
        || {},
    );
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
    // Inside an archive the viewer shows a temporary copy: never delete it.
    if crate::archive::parse_archive_path(&ui.cwd_path()).is_some() { return false; }
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
pub fn job_done(ui: &Rc<Ui>) {
    {
        let mut slot = ui.sidecar.borrow_mut();
        let Some(sidecar) = slot.as_mut().filter(|s| s.resend_after_job) else { return };
        sidecar.resend_after_job = false;
        sidecar.last = None;
    }
    follow(ui);
}

/// What the Sidecar shows: the selected paths, else the cursor row, all in
/// the folder `cwd`. Only a local folder sends anything (not a server; the
/// inside of an archive goes through [`_follow_archive`] instead). The rows come from the listing, so they are not
/// checked on disk: this runs on every selection change on the GTK thread.
pub fn sidecar_paths(selected: Vec<PathBuf>, cursor: Option<PathBuf>, cwd: &Path) -> Vec<PathBuf> {
    let local = crate::archive::parse_archive_path(cwd).is_none() && crate::remote::url_of(cwd).is_none();
    if !local { return Vec::new(); }
    if selected.is_empty() { cursor.into_iter().collect() } else { selected }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selection_wins_over_the_cursor() {
        let (a, b) = (PathBuf::from("/d/a.png"), PathBuf::from("/d/b.png"));
        let local = Path::new("/d");
        assert_eq!(sidecar_paths(vec![a.clone()], Some(b.clone()), local), vec![a]);
        assert_eq!(sidecar_paths(Vec::new(), Some(b.clone()), local), vec![b]);
        assert_eq!(sidecar_paths(Vec::new(), None, local), Vec::<PathBuf>::new());
    }

    #[test]
    fn nothing_is_sent_from_archives_or_servers_and_nothing_is_stat_ed() {
        // The paths do not exist: a local folder sends them as listed.
        let gone = PathBuf::from("/definitely/not/here.png");
        assert_eq!(sidecar_paths(Vec::new(), Some(gone.clone()), Path::new("/definitely/not")), vec![gone]);
        let in_archive = Path::new("/tmp/x.zip#inner");
        assert!(sidecar_paths(vec![in_archive.join("a.png")], None, in_archive).is_empty());
        let remote = crate::remote::path_of(&opusdm_ftp::RemoteUrl::parse("sftp://host/pub").unwrap());
        assert!(sidecar_paths(vec![remote.join("a.png")], None, &remote).is_empty());
    }

    #[test]
    fn archive_previews_skip_folders_and_big_files_and_are_capped() {
        let arch = Path::new("/d/x.zip");
        let rows = [("dir", true, 0, 1), ("a.png", false, 10, 2), ("huge.mkv", false, MAX_ARCHIVE_PREVIEW_BYTES + 1, 3)];
        let keys = archive_preview_keys(arch, "sub", rows.into_iter());
        assert_eq!(keys, vec![(arch.to_path_buf(), "sub/a.png".to_string(), 10, 2)]);
        assert_eq!(archive_preview_keys(arch, "", [("a.png", false, 10, 2)].into_iter())[0].1, "a.png");
        let names: Vec<String> = (0..MAX_ARCHIVE_PREVIEWS + 5).map(|i| format!("f{i}.png")).collect();
        let many = archive_preview_keys(arch, "", names.iter().map(|n| (n.as_str(), false, 1, 0)));
        assert_eq!(many.len(), MAX_ARCHIVE_PREVIEWS);
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
