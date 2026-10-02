//! Bodies of the built-in commands that need more than a one line call. The
//! command palette's `execute_command` is the exhaustive dispatch table; the
//! functions here are grouped by the palette category they belong to.

use crate::filter::glob_match;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{JobOptions, Op, Role, ToHub};
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// Flip the selection of every row.
pub fn invert_selection(ui: &Rc<Ui>) {
    let n = ui.view.selection.n_items();
    for i in 0..n {
        if ui.view.selection.is_selected(i) {
            ui.view.selection.unselect_item(i);
        } else {
            ui.view.selection.select_item(i, false);
        }
    }
}

/// Ask for a glob pattern, then select (`select == true`) or deselect every
/// row whose name matches it.
pub fn select_by_pattern(ui: &Rc<Ui>, select: bool) {
    let (title, initial) = if select {
        ("Select files matching pattern (e.g. *.rs)", "*")
    } else {
        ("Deselect files matching pattern (e.g. *.tmp)", "*")
    };
    let ui_clone = ui.clone();
    let win = ui.window.clone();
    crate::actions::prompt(&win, title, initial, move |pattern| {
        for i in 0..ui_clone.view.selection.n_items() {
            let Some(row) = ui_clone.row_at(i) else { continue };
            if !glob_match(&pattern, &row.name) {
                continue;
            }
            if select {
                ui_clone.view.selection.select_item(i, false);
            } else {
                ui_clone.view.selection.unselect_item(i);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Toggle hidden files through the toolbar button when there is one, so its
/// state and icon follow.
pub fn toggle_hidden(ui: &Rc<Ui>) {
    let active = !ui.view.show_hidden.get();
    // The toggle's handler sets the flag, swaps the icon and reloads.
    let button = ui.hidden_btn.borrow().clone();
    match button {
        Some(btn) => btn.set_active(active),
        None => {
            ui.view.show_hidden.set(active);
            ui.reload();
        }
    }
}

// ---------------------------------------------------------------------------
// File operations
// ---------------------------------------------------------------------------

/// Copy or Move the selection from SOURCE to the DEST Listers' folders.
pub fn transfer_to_dest(ui: &Rc<Ui>, op: Op, options: JobOptions) {
    if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
        crate::actions::flash(ui, &msg);
        return;
    }
    let srcs = ui.selected_paths();
    if srcs.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    crate::job_submit::submit_job(ui, op, srcs, None, options);
}

/// Copy each selected entry next to itself.
pub fn duplicate(ui: &Rc<Ui>) {
    let srcs = ui.selected_paths();
    if srcs.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    let dest_dir = Some(ui.cwd_path());
    crate::job_submit::submit_job(ui, Op::Duplicate, srcs, dest_dir, JobOptions::default());
}

/// Delete the selection after asking for confirmation.
pub fn delete_selection(ui: &Rc<Ui>) {
    let paths = ui.selected_paths();
    if paths.is_empty() {
        crate::actions::flash(ui, "nothing selected");
        return;
    }
    crate::actions::confirm_delete(ui, &format!("Delete {} item(s) permanently?", paths.len()), paths);
}

/// Ask for a name, then create a folder (`file == false`) or an empty file in
/// the current folder.
pub fn create_here(ui: &Rc<Ui>, file: bool) {
    let title = if file { "New file name" } else { "New directory name" };
    let ui_clone = ui.clone();
    let win = ui.window.clone();
    crate::actions::prompt(&win, title, "", move |name| {
        let op = if file { Op::NewFile { name } } else { Op::MakeDir { name } };
        let _ = ui_clone.hub.client.send(&ToHub::RunJob {
            op,
            srcs: Vec::new(),
            dest_dir: Some(ui_clone.cwd_path()),
            options: JobOptions::default(),
        });
    });
}

/// Rename the selected item (a dialog), or open the batch renamer for several.
pub fn rename_selection(ui: &Rc<Ui>) {
    let sel = ui.selected_paths();
    if sel.is_empty() {
        crate::actions::flash(ui, "nothing selected to rename");
        return;
    }
    if sel.len() > 1 {
        crate::batch_rename::open_batch_rename_dialog(ui);
        return;
    }
    let old = sel.into_iter().next().unwrap();
    let old_name = old.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ui_clone = ui.clone();
    let win = ui.window.clone();
    crate::actions::prompt_rename(&win, &old_name, old.is_dir(), move |new_name| {
        crate::actions::rename_and_reload(&ui_clone, &old, &new_name)
    });
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

/// Go to the parent folder (archive and remote aware).
pub fn go_parent(ui: &Rc<Ui>) {
    let parent = crate::navpath::parent(&ui.cwd.borrow());
    if let Some(p) = parent {
        ui.navigate_to(&p);
    }
}

/// Go to `$HOME`.
pub fn go_home(ui: &Rc<Ui>) {
    if let Ok(home) = std::env::var("HOME") {
        ui.navigate_to(std::path::Path::new(&home));
    }
}

/// Go to the root of the current tree.
pub fn go_root(ui: &Rc<Ui>) {
    let root = crate::navpath::root(&ui.cwd.borrow());
    ui.navigate_to(&root);
}

/// Open the parent folder in a new Lister.
pub fn open_parent_in_new(ui: &Rc<Ui>) {
    match crate::navpath::parent(&ui.cwd.borrow()) {
        Some(parent) => {
            let _ = ui.hub.client.send(&ToHub::SpawnLister { start_path: Some(parent) });
        }
        None => crate::actions::flash(ui, "already at the top"),
    }
}

/// Open the root of the current tree in a new Lister.
pub fn open_root_in_new(ui: &Rc<Ui>) {
    let root = crate::navpath::root(&ui.cwd.borrow());
    let _ = ui.hub.client.send(&ToHub::SpawnLister { start_path: Some(root) });
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// Ask the hub for `role`. Closing the palette hands the focus back to the
/// Lister window, and the hub makes a window that gains focus the SOURCE, which
/// would undo a DEST or NONE request sent just before. So while the window is
/// not active yet the request waits until it is, and then goes out after the
/// focus notification.
pub fn request_role(ui: &Rc<Ui>, role: Role) {
    if ui.window.is_active() {
        let _ = ui.hub.client.send(&ToHub::SetRole(role));
        return;
    }
    let ui = ui.clone();
    let sent = std::rc::Rc::new(std::cell::Cell::new(false));
    ui.window.clone().connect_is_active_notify(move |w| {
        if !w.is_active() || sent.replace(true) {
            return;
        }
        let ui = ui.clone();
        glib::idle_add_local_once(move || {
            let _ = ui.hub.client.send(&ToHub::SetRole(role));
        });
    });
}

/// Flip the role pin / lock.
pub fn toggle_pin(ui: &Rc<Ui>) {
    let active = !ui.status_bar.role_lock_btn.is_active();
    ui.status_bar.role_lock_btn.set_active(active);
    let msg = if active { ToHub::LockRole } else { ToHub::UnlockRole };
    let _ = ui.hub.client.send(&msg);
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// Open the File Types editor on a new type derived from the first selected file.
pub fn create_filetype(ui: &Rc<Ui>) {
    let sample = ui.selected_names().into_iter().next().map(|name| {
        let path = ui.cwd_path().join(&name);
        (name, path)
    });
    match sample {
        Some(sample) => crate::filetypes_ui::open_editor(ui, Some(sample)),
        None => crate::actions::flash(ui, "select a file to derive the file type from"),
    }
}

/// Run the saved Function `id` through the hub.
pub fn run_function(ui: &Rc<Ui>, id: opusdm_ipc::function::FunctionId) {
    let Some(function) = opusdm_ipc::function::load_functions().into_iter().find(|f| f.id == id) else {
        crate::actions::flash(ui, "function not found (it may have been deleted)");
        return;
    };
    ui.start_run(ToHub::RunFunction { steps: function.steps });
}
