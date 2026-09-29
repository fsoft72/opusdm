//! Ctrl+C / Ctrl+X / Ctrl+V clipboard for file paths, backed by the system
//! text clipboard so Copy/Cut in one Lister and Paste in another - or in a
//! different app entirely - just works without any hub-mediated IPC.
//!
//! The payload is a `text/uri-list` (see `opusdm_iconview::dnd`) with a
//! marker line in front recording whether it was a Copy or a Cut, since
//! `uris_to_paths` already ignores lines starting with `#`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::Op;
use std::rc::Rc;

const CUT_MARKER: &str = "#opusdm-clip:cut";
const COPY_MARKER: &str = "#opusdm-clip:copy";

/// Put the current selection on the system clipboard.
///
/// `cut` records whether the paste should move (Cut) or duplicate (Copy)
/// the files - the source files are left in place either way; a Cut only
/// takes effect once something is actually pasted.
pub fn copy_or_cut(ui: &Rc<Ui>, cut: bool) {
    let Some(display) = gdk::Display::default() else { return };
    let paths = ui.selected_paths();
    if paths.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    let marker = if cut { CUT_MARKER } else { COPY_MARKER };
    let payload = format!("{marker}\r\n{}", opusdm_iconview::dnd::paths_to_uri_list(&paths));
    display.clipboard().set_text(&payload);
    crate::dlog!("clipboard: {} {} item(s)", if cut { "cut" } else { "copy" }, paths.len());
}

/// Paste whatever `copy_or_cut` last put on the clipboard into the current
/// directory, as a Copy or Move `RunJob` depending on the marker line.
pub fn paste(ui: &Rc<Ui>) {
    let Some(display) = gdk::Display::default() else { return };
    let ui = ui.clone();
    let cancellable: Option<&gio::Cancellable> = None;
    display.clipboard().read_text_async(cancellable, move |res| {
        let Ok(Some(text)) = res else {
            crate::dlog!("clipboard paste: no text on clipboard");
            return;
        };
        let srcs = opusdm_iconview::dnd::uris_to_paths(&text);
        if srcs.is_empty() {
            crate::actions::flash(&ui, "clipboard has no files");
            return;
        }
        let op = if text.starts_with(CUT_MARKER) { Op::Move } else { Op::Copy };
        let dest_dir = Some(ui.cwd_path());
        crate::dlog!("clipboard paste: op={op:?} srcs={srcs:?} dest={dest_dir:?}");
        crate::job_submit::submit_job(&ui, op, srcs, dest_dir, opusdm_ipc::JobOptions::default());
    });
}
