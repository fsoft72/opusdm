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
    let icon_size = 18;

    let copy_btn = crate::icons::icon_button(crate::icons::SVG_COPY, "Copy (Source -> Dest)", icon_size);
    let move_btn = crate::icons::icon_button(crate::icons::SVG_MOVE, "Move (Source -> Dest)", icon_size);
    let delete_btn = crate::icons::icon_button(crate::icons::SVG_DELETE, "Delete", icon_size);
    let mkdir_btn = crate::icons::icon_button(crate::icons::SVG_MKDIR, "Make Directory", icon_size);
    let newfile_btn = crate::icons::icon_button(crate::icons::SVG_NEWFILE, "New File", icon_size);
    let rename_btn = crate::icons::icon_button(crate::icons::SVG_RENAME, "Rename", icon_size);
    
    let is_hidden_active = ui.show_hidden.get();
    let hidden_btn = crate::icons::icon_toggle_button(
        if is_hidden_active { crate::icons::SVG_EYE } else { crate::icons::SVG_EYE_OFF },
        "Toggle Hidden Files",
        icon_size,
    );
    hidden_btn.set_active(is_hidden_active);

    let preview_btn = gtk::Button::with_label("👁");
    preview_btn.set_tooltip_text(Some("Quick Look Preview (F3 / Space)"));
    preview_btn.add_css_class("opus-btn");

    let term_btn = gtk::Button::with_label(">_");
    term_btn.set_tooltip_text(Some("Terminal Drawer (F4 / `)"));
    term_btn.add_css_class("opus-btn");

    row.append(&copy_btn);
    row.append(&move_btn);
    row.append(&delete_btn);
    row.append(&mkdir_btn);
    row.append(&newfile_btn);
    row.append(&rename_btn);
    row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    row.append(&hidden_btn);
    row.append(&preview_btn);
    row.append(&term_btn);

    wire_transfer(ui, &copy_btn, Op::Copy);
    wire_transfer(ui, &move_btn, Op::Move);

    {
        let ui = ui.clone();
        preview_btn.connect_clicked(move |_| {
            ui.toggle_preview();
        });
    }

    {
        let ui = ui.clone();
        term_btn.connect_clicked(move |_| {
            ui.toggle_terminal();
        });
    }

    {
        let ui = ui.clone();
        hidden_btn.connect_toggled(move |btn| {
            let active = btn.is_active();
            ui.show_hidden.set(active);
            btn.set_child(Some(&crate::icons::svg_image(
                if active { crate::icons::SVG_EYE } else { crate::icons::SVG_EYE_OFF },
                icon_size,
            )));
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
            let ui2 = ui.clone();
            opusdm_ui::requester(
                &ui.window,
                "Request",
                &format!("Delete {} item(s) permanently?", paths.len()),
                None,
                &["Delete", "Cancel"],
                move |choice| {
                    if choice == 0 {
                        let _ = ui2.client.send(&ToHub::RunJob {
                            op: Op::Delete,
                            srcs: ui2.selected_paths(),
                            dest_dir: Some(ui2.cwd_path()),
                        });
                    }
                },
            );
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
            if sel.is_empty() {
                flash(&ui, "nothing selected to rename");
                return;
            }
            if sel.len() > 1 {
                crate::batch_rename::open_batch_rename_dialog(&ui);
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

        let cwd = ui.cwd.borrow().clone();
        if let Some((arch, inner)) = crate::archive::parse_archive_path(&cwd) {
            let names = ui.selected_names();
            if names.is_empty() {
                flash(&ui, "nothing selected");
                return;
            }
            let temp_extract = std::env::temp_dir().join(format!("opusdm-extract-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&temp_extract);
            let full_items: Vec<String> = names
                .into_iter()
                .map(|n| {
                    if inner.is_empty() {
                        n
                    } else {
                        format!("{}/{}", inner.trim_end_matches('/'), n)
                    }
                })
                .collect();
            match crate::archive::extract_items_to_dir(&arch, &full_items, &temp_extract) {
                Ok(extracted) => {
                    let _ = ui.client.send(&ToHub::RunJob {
                        op: op.clone(),
                        srcs: extracted,
                        dest_dir: None,
                    });
                }
                Err(e) => flash(&ui, &format!("extraction failed: {e}")),
            }
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
            show_error_dialog(ui, errors);
            ui.job_line.set_text("");
        }
        _ => ui.job_line.set_text(""),
    }
    ui.reload();
}

fn show_error_dialog(ui: &Rc<Ui>, errors: &[(PathBuf, String)]) {
    let dialog = gtk::Window::builder()
        .title(format!("Completed with {} error(s)", errors.len()))
        .transient_for(&ui.window)
        .modal(true)
        .default_width(550)
        .default_height(350)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);

    let header = gtk::Label::new(Some(&format!(
        "The operation completed with {} error(s):",
        errors.len()
    )));
    header.set_xalign(0.0);
    header.add_css_class("title-4");
    vbox.append(&header);

    let full_text = errors
        .iter()
        .map(|(p, e)| format!("{}: {e}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");

    let text_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    text_view.buffer().set_text(&full_text);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&text_view)
        .vexpand(true)
        .hexpand(true)
        .build();
    vbox.append(&scroller);

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk::Align::End);

    let copy_btn = gtk::Button::with_label("Copy Errors");
    let full_text_clone = full_text.clone();
    copy_btn.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&full_text_clone);
        }
    });
    btn_box.append(&copy_btn);

    let close_btn = gtk::Button::with_label("Close");
    let dlg = dialog.clone();
    close_btn.connect_clicked(move |_| {
        dlg.close();
    });
    btn_box.append(&close_btn);

    vbox.append(&btn_box);
    dialog.set_child(Some(&vbox));
    dialog.present();
}

/// Prompt for an overwrite conflict and reply to the hub.
pub fn on_conflict(ui: &Rc<Ui>, req_id: u64, path: &Path, src: FileMeta, dst: FileMeta) {
    let ui = ui.clone();
    let win = ui.window.clone();
    opusdm_ui::requester(
        &win,
        "Request",
        &format!("{} already exists", path.display()),
        Some(&format!(
            "source: {} bytes\ndestination: {} bytes",
            src.size, dst.size
        )),
        &["Overwrite", "Skip", "Overwrite All", "Skip All", "Cancel"],
        move |choice| {
            let resolution = match choice {
                0 => Resolution::Overwrite,
                1 => Resolution::Skip,
                2 => Resolution::OverwriteAll,
                3 => Resolution::SkipAll,
                _ => Resolution::Cancel,
            };
            let _ = ui.client.send(&ToHub::ConflictReply { req_id, resolution });
        },
    );
}

/// Show `text` in the transient message line for a few seconds.
pub fn flash(ui: &Rc<Ui>, text: &str) {
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
pub fn prompt(
    parent: &gtk::ApplicationWindow,
    title: &str,
    initial: &str,
    on_ok: impl Fn(String) + 'static,
) {
    opusdm_ui::requester_entry(parent, title, "", initial, &["OK", "Cancel"], move |idx, text| {
        if idx == 0 && !text.is_empty() {
            on_ok(text);
        }
    });
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
