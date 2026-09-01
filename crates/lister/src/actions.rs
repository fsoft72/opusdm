//! The Lister button row and the dialogs it drives.

use crate::client::Roles;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{FileMeta, ListerId, Op, Outcome, Resolution, ToHub};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Check the preconditions for a Copy/Move from this Lister. The hub resolves
/// the actual destination directory (the DEST Lister's current path).
pub fn resolve_copy_target(roles: &Roles, me: ListerId) -> Result<(), String> {
    if roles.source != Some(me) {
        return Err("this Lister is not the SOURCE".into());
    }
    let real_dests: Vec<_> = roles.dests.iter().filter(|d| **d != me).collect();
    if real_dests.is_empty() {
        return Err("no DEST Lister set".into());
    }
    Ok(())
}

/// Build the button row and wire every control. Call once during window build.
pub fn attach(ui: &Rc<Ui>) {
    let row = &ui.button_row;

    let copy_btn = gtk::Button::with_label("Copy");
    let move_btn = gtk::Button::with_label("Move");
    let delete_btn = gtk::Button::with_label("Delete");
    let mkdir_btn = gtk::Button::with_label("MakeDir");
    let newfile_btn = gtk::Button::with_label("NewFile");
    let rename_btn = gtk::Button::with_label("Rename");
    let hidden_btn = gtk::ToggleButton::with_label("Hidden");
    hidden_btn.set_active(ui.show_hidden.get());

    row.append(&copy_btn);
    row.append(&move_btn);
    row.append(&delete_btn);
    row.append(&mkdir_btn);
    row.append(&newfile_btn);
    row.append(&rename_btn);
    row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    row.append(&hidden_btn);

    wire_transfer(ui, &copy_btn, Op::Copy);
    wire_transfer(ui, &move_btn, Op::Move);

    {
        let ui = ui.clone();
        hidden_btn.connect_toggled(move |btn| {
            ui.show_hidden.set(btn.is_active());
            ui.reload();
        });
    }

    {
        let ui = ui.clone();
        delete_btn.connect_clicked(move |_| {
            let paths = ui.selected_paths();
            if paths.is_empty() {
                flash(&ui, "nothing selected");
                return;
            }
            let dialog = gtk::AlertDialog::builder()
                .message(format!("Delete {} item(s) permanently?", paths.len()))
                .buttons(["Cancel", "Delete"])
                .cancel_button(0)
                .default_button(0)
                .build();
            let ui2 = ui.clone();
            dialog.choose(Some(&ui.window), gtk::gio::Cancellable::NONE, move |res| {
                if res == Ok(1) {
                    let _ = ui2.client.send(&ToHub::RunJob {
                        op: Op::Delete,
                        srcs: ui2.selected_paths(),
                        dest_dir: Some(ui2.cwd_path()),
                    });
                }
            });
        });
    }

    {
        let ui = ui.clone();
        mkdir_btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let win = ui.window.clone();
            prompt(&win, "New directory name", "", move |name| {
                let _ = ui.client.send(&ToHub::RunJob {
                    op: Op::MakeDir { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui.cwd_path()),
                });
            });
        });
    }

    {
        let ui = ui.clone();
        newfile_btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let win = ui.window.clone();
            prompt(&win, "New file name", "", move |name| {
                let _ = ui.client.send(&ToHub::RunJob {
                    op: Op::NewFile { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui.cwd_path()),
                });
            });
        });
    }

    {
        let ui = ui.clone();
        rename_btn.connect_clicked(move |_| {
            let sel = ui.selected_paths();
            if sel.len() != 1 {
                flash(&ui, "select exactly one item to rename");
                return;
            }
            let old = sel.into_iter().next().unwrap();
            let old_name = old
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ui = ui.clone();
            let win = ui.window.clone();
            prompt(&win, "Rename to", &old_name, move |new_name| {
                let new_path = ui.cwd_path().join(&new_name);
                match std::fs::rename(&old, &new_path) {
                    Ok(()) => ui.reload(),
                    Err(e) => flash(&ui, &format!("rename failed: {e}")),
                }
            });
        });
    }

    // Pin: locks this Lister's role against focus changes. Role itself is
    // set by clicking the stat-bar badge (see window::build_window), which
    // cycles SOURCE -> DEST -> NONE.
    {
        let ui = ui.clone();
        let btn = ui.role_lock_btn.clone();
        btn.connect_toggled(move |b| {
            if ui.role_guard.get() {
                return;
            }
            let msg = if b.is_active() { ToHub::LockRole } else { ToHub::UnlockRole };
            let _ = ui.client.send(&msg);
        });
    }

    // Reflect the initial role state (pin starts disabled with no role).
    sync_role_buttons(ui);
}

fn wire_transfer(ui: &Rc<Ui>, btn: &gtk::Button, op: Op) {
    let ui = ui.clone();
    btn.connect_clicked(move |_| {
        if let Err(msg) = resolve_copy_target(&ui.roles.borrow(), ui.id) {
            flash(&ui, &msg);
            return;
        }
        let srcs = ui.selected_paths();
        if srcs.is_empty() {
            flash(&ui, "nothing selected");
            return;
        }
        let _ = ui.client.send(&ToHub::RunJob {
            op: op.clone(),
            srcs,
            dest_dir: None, // hub fills this from the DEST Lister's path
        });
    });
}

/// Sync the pin control from the hub's role state without re-emitting. Pin
/// is only usable while this Lister holds a role (SOURCE or DEST).
pub fn sync_role_buttons(ui: &Rc<Ui>) {
    ui.role_guard.set(true);
    let roles = ui.roles.borrow();
    let has_role = roles.source == Some(ui.id) || roles.is_dest(ui.id);
    ui.role_lock_btn.set_sensitive(has_role);
    if !has_role && ui.role_lock_btn.is_active() {
        ui.role_lock_btn.set_active(false);
    }
    drop(roles);
    ui.role_guard.set(false);
}

/// Update the in-window job status line.
pub fn on_job_progress(ui: &Rc<Ui>, _job_id: u64, done: u64, total: u64, current: &Path) {
    let pct = if total > 0 { done * 100 / total } else { 0 };
    ui.job_line.set_text(&format!("{pct}%  {}", current.display()));
}

/// Finish a job: clear the line, show errors, reload the view.
pub fn on_job_done(ui: &Rc<Ui>, _job_id: u64, outcome: &Outcome, errors: &[(PathBuf, String)]) {
    match outcome {
        Outcome::Error { message } => flash(ui, &format!("job failed: {message}")),
        _ if !errors.is_empty() => {
            let detail = errors
                .iter()
                .take(20)
                .map(|(p, e)| format!("{}: {e}", p.display()))
                .collect::<Vec<_>>()
                .join("\n");
            gtk::AlertDialog::builder()
                .message(format!("Completed with {} error(s)", errors.len()))
                .detail(detail)
                .build()
                .show(Some(&ui.window));
            ui.job_line.set_text("");
        }
        _ => ui.job_line.set_text(""),
    }
    ui.reload();
}

/// Prompt for an overwrite conflict and reply to the hub.
pub fn on_conflict(ui: &Rc<Ui>, req_id: u64, path: &Path, src: FileMeta, dst: FileMeta) {
    let dialog = gtk::AlertDialog::builder()
        .message(format!("{} already exists", path.display()))
        .detail(format!(
            "source: {} bytes\ndestination: {} bytes",
            src.size, dst.size
        ))
        .buttons(["Skip", "Overwrite", "Skip All", "Overwrite All", "Cancel"])
        .cancel_button(4)
        .default_button(0)
        .build();
    let ui = ui.clone();
    let win = ui.window.clone();
    dialog.choose(Some(&win), gtk::gio::Cancellable::NONE, move |res| {
        let resolution = match res {
            Ok(0) => Resolution::Skip,
            Ok(1) => Resolution::Overwrite,
            Ok(2) => Resolution::SkipAll,
            Ok(3) => Resolution::OverwriteAll,
            _ => Resolution::Cancel,
        };
        let _ = ui.client.send(&ToHub::ConflictReply { req_id, resolution });
    });
}

/// Show `text` in the transient message line for a few seconds.
fn flash(ui: &Rc<Ui>, text: &str) {
    ui.job_line.set_text(text);
    let line = ui.job_line.clone();
    let expected = text.to_string();
    glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
        if line.text() == expected {
            line.set_text("");
        }
    });
}

/// A small modal name-entry dialog. Calls `on_ok` with the entered text.
fn prompt(
    parent: &gtk::ApplicationWindow,
    title: &str,
    initial: &str,
    on_ok: impl Fn(String) + 'static,
) {
    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .default_width(320)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(10);
    vbox.set_margin_bottom(10);
    vbox.set_margin_start(10);
    vbox.set_margin_end(10);

    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    vbox.append(&entry);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label("OK");
    buttons.append(&cancel);
    buttons.append(&ok);
    vbox.append(&buttons);

    dialog.set_child(Some(&vbox));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog_ref = dialog.clone();
        let entry_ref = entry.clone();
        let commit: Rc<dyn Fn()> = Rc::new(move || {
            let text = entry_ref.text().to_string();
            if !text.is_empty() {
                on_ok(text);
            }
            dialog_ref.close();
        });
        {
            let commit = commit.clone();
            ok.connect_clicked(move |_| commit());
        }
        {
            let commit = commit.clone();
            entry.connect_activate(move |_| commit());
        }
    }

    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Roles;
    use opusdm_ipc::ListerId;

    #[test]
    fn resolve_requires_me_source_and_a_dest() {
        let me = ListerId(1);
        let other = ListerId(2);
        assert!(resolve_copy_target(&Roles { source: Some(other), dests: vec![me], ..Default::default() }, me).is_err());
        assert!(resolve_copy_target(&Roles { source: Some(me), dests: vec![], ..Default::default() }, me).is_err());
        assert!(resolve_copy_target(&Roles { source: Some(me), dests: vec![me], ..Default::default() }, me).is_err());
        assert!(resolve_copy_target(&Roles { source: Some(me), dests: vec![other], ..Default::default() }, me).is_ok());
        assert!(resolve_copy_target(&Roles { source: Some(me), dests: vec![other, ListerId(3)], ..Default::default() }, me).is_ok());
    }
}
