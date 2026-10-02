//! File operations triggered from the desktop context menu: clipboard
//! cut/copy/paste, rename, delete, new folder/file, and opening a terminal
//! in the desktop directory.
//!
//! These mirror the equivalent Lister actions (`lister::clip`,
//! `lister::actions`), but issue `HubCmd::RunJob` directly rather than
//! `ToHub::RunJob` over IPC - the hub is already the job engine, and the
//! desktop window (not a per-Lister connection) supplies the destination
//! directory, so there is no SOURCE/DEST role to resolve first.

use crate::server::HubCmd;
use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::Op;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

const CUT_MARKER: &str = "#opusdm-clip:cut";
const COPY_MARKER: &str = "#opusdm-clip:copy";

/// Terminal binaries tried, in order, when `$TERMINAL` is unset or not on
/// `PATH`.
const FALLBACK_TERMINALS: [&str; 8] = [
    "x-terminal-emulator",
    "gnome-terminal",
    "konsole",
    "xfce4-terminal",
    "kitty",
    "alacritty",
    "lxterminal",
    "xterm",
];

/// Put `paths` on the system clipboard, tagged Cut or Copy, so Paste here or
/// in any Lister picks up the right operation.
pub fn copy_or_cut(paths: &[PathBuf], cut: bool) {
    let Some(display) = gdk::Display::default() else { return };
    if paths.is_empty() {
        return;
    }
    let marker = if cut { CUT_MARKER } else { COPY_MARKER };
    let payload = format!("{marker}\r\n{}", opusdm_iconview::dnd::paths_to_uri_list(paths));
    display.clipboard().set_text(&payload);
}

/// Paste whatever `copy_or_cut` (here or in a Lister) last put on the
/// clipboard into `dest_dir`, as a Copy or Move `RunJob` depending on the
/// marker line. A no-op if the clipboard holds no file paths.
pub fn paste(cmd_tx: &Sender<HubCmd>, dest_dir: PathBuf) {
    let Some(display) = gdk::Display::default() else { return };
    let cmd_tx = cmd_tx.clone();
    let cancellable: Option<&gio::Cancellable> = None;
    display.clipboard().read_text_async(cancellable, move |res| {
        let Ok(Some(text)) = res else { return };
        let srcs = opusdm_iconview::dnd::uris_to_paths(&text);
        if srcs.is_empty() {
            return;
        }
        let op = if text.starts_with(CUT_MARKER) { Op::Move } else { Op::Copy };
        let _ = cmd_tx.send(HubCmd::RunJob { op, srcs, dest_dir: dest_dir.clone() });
    });
}

/// Ask for a new name, then rename `old` in place. The desktop's own inotify
/// watch picks up the change and reloads the icon field; failures are logged.
pub fn rename(parent: &gtk::ApplicationWindow, old: PathBuf, old_name: &str) {
    let Some(dir) = old.parent().map(Path::to_path_buf) else { return };
    opusdm_ui::requester_entry(
        parent,
        "Rename to",
        old_name,
        &[("OK", true), ("Cancel", false)],
        move |accepted, text| {
            if !accepted || text.is_empty() {
                return;
            }
            let new_path = dir.join(&text);
            if let Err(e) = opusdm_fsops::rename_noreplace(&old, &new_path) {
                opusdm_ipc::err_log!("hub", "desktop rename failed: {e}");
            }
        },
    );
}

/// Ask for confirmation, then permanently delete `paths` (no trash).
pub fn confirm_delete(
    parent: &gtk::ApplicationWindow,
    cmd_tx: &Sender<HubCmd>,
    message: &str,
    srcs: Vec<PathBuf>,
    dest_dir: PathBuf,
) {
    let cmd_tx = cmd_tx.clone();
    opusdm_ui::requester(
        parent,
        "Request",
        message,
        None,
        &[("Delete", true), ("Cancel", false)],
        move |confirmed| {
            if !confirmed {
                return;
            }
            let _ = cmd_tx.send(HubCmd::RunJob {
                op: Op::Delete,
                srcs: srcs.clone(),
                dest_dir: dest_dir.clone(),
            });
        },
    );
}

/// Prompt for a name, then run whatever `RunJob` `make_op` builds from it
/// inside `dest_dir` - used for both "New Folder" (`Op::MakeDir`) and
/// "New File" (`Op::NewFile`).
pub fn prompt_new(
    parent: &gtk::ApplicationWindow,
    title: &str,
    cmd_tx: &Sender<HubCmd>,
    dest_dir: PathBuf,
    make_op: impl Fn(String) -> Op + 'static,
) {
    let cmd_tx = cmd_tx.clone();
    opusdm_ui::requester_entry(
        parent,
        title,
        "",
        &[("OK", true), ("Cancel", false)],
        move |accepted, text| {
            if !accepted || text.is_empty() {
                return;
            }
            let _ = cmd_tx.send(HubCmd::RunJob {
                op: make_op(text),
                srcs: Vec::new(),
                dest_dir: dest_dir.clone(),
            });
        },
    );
}

/// Launch a terminal emulator with `dir` as its working directory: `$TERMINAL`
/// first, then a handful of common terminal binaries, whichever is found
/// first. Errors (none found) are logged, not shown to the user.
pub fn open_terminal_in(dir: &Path) {
    let candidates = std::env::var("TERMINAL")
        .into_iter()
        .chain(FALLBACK_TERMINALS.iter().map(|s| s.to_string()));
    for bin in candidates {
        if std::process::Command::new(&bin).current_dir(dir).spawn().is_ok() {
            return;
        }
    }
    opusdm_ipc::err_log!("hub", "no terminal emulator found ($TERMINAL or a common fallback)");
}
