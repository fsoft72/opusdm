//! "Change Permissions..." (shared with the Desktop, see
//! `opusdm_fileactions::permissions`) and "Set Timestamp..." dialogs for the
//! selection.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::timefmt::{format_local, parse_local, TIME_FORMAT_HINT};
use opusdm_ipc::{JobOptions, Op};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Open the permissions dialog for the current selection.
pub fn open_permissions(ui: &Rc<Ui>) {
    open_permissions_for(ui, ui.selected_paths());
}

/// Open the permissions dialog for `srcs` (the context menu passes the
/// clicked row or the selection that holds it).
pub fn open_permissions_for(ui: &Rc<Ui>, srcs: Vec<std::path::PathBuf>) {
    opusdm_fileactions::permissions::open_permissions(&crate::file_host::ListerHost::rc(ui), srcs);
}

/// Now, as Unix seconds.
fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Open the timestamp dialog for the current selection.
pub fn open_timestamp(ui: &Rc<Ui>) {
    let srcs = ui.selected_paths();
    let Some(first) = srcs.first() else {
        crate::actions::flash(ui, "nothing selected");
        return;
    };
    let initial = std::fs::metadata(first)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(now_secs);

    let shell = opusdm_ui::DialogShell::new(&ui.window, "Set Timestamp").size(380, 200);
    shell.content().append(&gtk::Label::new(Some(&format!("Date and time ({TIME_FORMAT_HINT})"))));
    let entry = gtk::Entry::new();
    entry.set_text(&format_local(initial));
    entry.set_activates_default(true);
    shell.content().append(&entry);
    let now_btn = gtk::Button::with_label("Now");
    now_btn.set_halign(gtk::Align::Start);
    shell.content().append(&now_btn);
    let contents = gtk::CheckButton::with_label("Apply to contents of directories");
    shell.content().append(&contents);
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.add_css_class("error");
    shell.content().append(&error);

    {
        let entry = entry.clone();
        now_btn.connect_clicked(move |_| entry.set_text(&format_local(now_secs())));
    }
    let ok_btn = shell.button("Apply");
    shell.close_button("Cancel");
    let window = shell.window().clone();
    let ui = ui.clone();
    ok_btn.connect_clicked(move |_| match parse_local(entry.text().as_str()) {
        Ok(unix_secs) => {
            let op = Op::Touch { unix_secs, recurse: contents.is_active() };
            crate::job_submit::submit_job(&ui, op, srcs.clone(), Some(ui.cwd_path()), JobOptions::default());
            window.close();
        }
        Err(msg) => error.set_text(&msg),
    });
    shell.present();
}
