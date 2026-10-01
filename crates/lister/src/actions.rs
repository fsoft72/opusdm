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

    let copy_btn = crate::icons::icon_button(crate::icons::SVG_COPY, "Copy (Source -> Dest). Right click: Copy As...", icon_size);
    let move_btn = crate::icons::icon_button(crate::icons::SVG_MOVE, "Move (Source -> Dest). Right click: Move As...", icon_size);
    let delete_btn = crate::icons::icon_button(crate::icons::SVG_DELETE, "Delete", icon_size);
    let mkdir_btn = crate::icons::icon_button(crate::icons::SVG_MKDIR, "Make Directory", icon_size);
    let newfile_btn = crate::icons::icon_button(crate::icons::SVG_NEWFILE, "New File", icon_size);
    let rename_btn = crate::icons::icon_button(crate::icons::SVG_RENAME, "Rename", icon_size);
    
    let is_hidden_active = ui.view.show_hidden.get();
    let hidden_btn = crate::icons::icon_toggle_button(
        hidden_icon(is_hidden_active),
        "Toggle Hidden Files",
        icon_size,
    );
    hidden_btn.set_active(is_hidden_active);
    *ui.hidden_btn.borrow_mut() = Some(hidden_btn.clone());

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
    row.append(&ui.user_menu_btn);

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
            ui.view.show_hidden.set(active);
            btn.set_child(Some(&crate::icons::svg_image(hidden_icon(active), icon_size)));
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
            confirm_delete(
                &ui,
                &format!("Delete {} item(s) permanently?", paths.len()),
                paths,
            );
        });
    }

    {
        let ui = ui.clone();
        mkdir_btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let win = ui.window.clone();
            prompt(&win, "New directory name", "", move |name| {
                let _ = ui.hub.client.send(&ToHub::RunJob {
                    op: Op::MakeDir { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui.cwd_path()),
                    options: opusdm_ipc::JobOptions::default(),
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
                let _ = ui.hub.client.send(&ToHub::RunJob {
                    op: Op::NewFile { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui.cwd_path()),
                    options: opusdm_ipc::JobOptions::default(),
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
            prompt_rename(&win, &old_name, old.is_dir(), move |new_name| {
                let new_path = ui.cwd_path().join(&new_name);
                match std::fs::rename(&old, &new_path) {
                    Ok(()) => ui.reload(),
                    Err(e) => flash(&ui, &format!("rename failed: {e}")),
                }
            });
        });
    }

    // Pin: locks this Lister's role against focus changes. Role itself is
    // set by clicking the stat-bar badge (see build::build_window), which
    // cycles SOURCE -> DEST -> NONE.
    {
        let ui = ui.clone();
        let btn = ui.status_bar.role_lock_btn.clone();
        btn.connect_toggled(move |b| {
            if ui.status_bar.role_guard.get() {
                return;
            }
            let msg = if b.is_active() { ToHub::LockRole } else { ToHub::UnlockRole };
            let _ = ui.hub.client.send(&msg);
        });
    }

    // Reflect the initial role state (pin starts disabled with no role).
    sync_role_buttons(ui);
}

/// The "As..." command a right click on the Copy / Move button runs.
fn as_command(op: &Op) -> Option<crate::keys::CommandId> {
    match op {
        Op::Copy => Some(crate::keys::CommandId::CopyAs),
        Op::Move => Some(crate::keys::CommandId::MoveAs),
        _ => None,
    }
}

fn wire_transfer(ui: &Rc<Ui>, btn: &gtk::Button, op: Op) {
    // Right click: the "As..." variant (naming dialog) of the same operation.
    if let Some(cmd) = as_command(&op) {
        let ui = ui.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.connect_pressed(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            crate::palette::execute_command(&ui, cmd.clone());
        });
        btn.add_controller(gesture);
    }
    let ui = ui.clone();
    btn.connect_clicked(move |_| {
        if let Err(msg) = resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
            flash(&ui, &msg);
            return;
        }

        let cwd = ui.cwd.borrow().clone();
        // A remote folder: the hub hands the job back (see `remote_xfer`).
        if crate::remote::url_of(&cwd).is_some() {
            let srcs = ui.selected_paths();
            if srcs.is_empty() {
                flash(&ui, "nothing selected");
                return;
            }
            crate::job_submit::submit_job(&ui, op.clone(), srcs, None, opusdm_ipc::JobOptions::default());
            return;
        }
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
                    crate::job_submit::submit_job(&ui, op.clone(), extracted, None, opusdm_ipc::JobOptions::default());
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
        crate::job_submit::submit_job(&ui, op.clone(), srcs, None, opusdm_ipc::JobOptions::default());
    });
}

/// Sync the pin control from the hub's role state without re-emitting. Pin
/// is only usable while this Lister holds a role (SOURCE or DEST).
pub fn sync_role_buttons(ui: &Rc<Ui>) {
    ui.status_bar.role_guard.set(true);
    let roles = ui.hub.roles.borrow();
    let has_role = roles.source == Some(ui.hub.id) || roles.is_dest(ui.hub.id);
    ui.status_bar.role_lock_btn.set_sensitive(has_role);
    if !has_role && ui.status_bar.role_lock_btn.is_active() {
        ui.status_bar.role_lock_btn.set_active(false);
    }
    drop(roles);
    ui.status_bar.role_guard.set(false);
}

/// Finish a job: clear the line, show errors, reload the view.
pub fn on_job_done(ui: &Rc<Ui>, _job_id: u64, outcome: &Outcome, errors: &[(PathBuf, String)]) {
    match outcome {
        Outcome::Error { message } => flash(ui, &format!("job failed: {message}")),
        _ if !errors.is_empty() => {
            show_error_dialog(ui, errors);
            ui.status_bar.job_line.set_text("");
        }
        _ => ui.status_bar.job_line.set_text(""),
    }
    ui.reload();
}

fn show_error_dialog(ui: &Rc<Ui>, errors: &[(PathBuf, String)]) {
    let shell = opusdm_ui::DialogShell::new(
        &ui.window,
        &format!("Completed with {} error(s)", errors.len()),
    )
    .size(550, 350);
    let vbox = shell.content();

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

    let copy_btn = shell.button("Copy Errors");
    copy_btn.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&full_text);
        }
    });
    shell.close_button("Close");

    shell.present();
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
        &[
            ("Overwrite", Resolution::Overwrite),
            ("Skip", Resolution::Skip),
            ("Overwrite All", Resolution::OverwriteAll),
            ("Skip All", Resolution::SkipAll),
            ("Cancel", Resolution::Cancel),
        ],
        move |resolution| {
            let _ = ui.hub.client.send(&ToHub::ConflictReply { req_id, resolution });
        },
    );
}

/// Ask the user to confirm a deletion, then hand `srcs` to the hub.
///
/// `message` is the question shown in the requester; it differs between the
/// toolbar (a count of selected entries) and the context menu (one name).
pub fn confirm_delete(ui: &Rc<Ui>, message: &str, srcs: Vec<PathBuf>) {
    let win = ui.window.clone();
    let ui = ui.clone();
    opusdm_ui::requester(
        &win,
        "Request",
        message,
        None,
        &[("Delete", true), ("Cancel", false)],
        move |confirmed| {
            if !confirmed {
                return;
            }
            crate::job_submit::submit_job(&ui, Op::Delete, srcs.clone(), Some(ui.cwd_path()), opusdm_ipc::JobOptions::default());
        },
    );
}

/// Show `text` in the transient message line for a few seconds.
pub fn flash(ui: &Rc<Ui>, text: &str) {
    ui.status_bar.job_line.set_text(text);
    let line = ui.status_bar.job_line.clone();
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
    opusdm_ui::requester_entry(
        parent,
        title,
        initial,
        &[("OK", true), ("Cancel", false)],
        move |accepted, text| {
            if accepted && !text.is_empty() {
                on_ok(text);
            }
        },
    );
}

/// The toolbar icon for the show-hidden toggle: open eye while dotfiles show.
pub fn hidden_icon(active: bool) -> &'static [u8] {
    if active { crate::icons::SVG_EYE } else { crate::icons::SVG_EYE_OFF }
}

/// Character range of `name` a rename dialog preselects: the name without its
/// extension. A directory, a name without a dot and a leading-dot name such as
/// `.bashrc` are selected whole.
pub fn rename_selection(name: &str, is_dir: bool) -> (i32, i32) {
    let total = name.chars().count() as i32;
    if is_dir {
        return (0, total);
    }
    match name.rfind('.') {
        Some(byte) if byte > 0 => (0, name[..byte].chars().count() as i32),
        _ => (0, total),
    }
}

/// The rename dialog: like [`prompt`], with only the name part selected.
pub fn prompt_rename(
    parent: &gtk::ApplicationWindow,
    initial: &str,
    is_dir: bool,
    on_ok: impl Fn(String) + 'static,
) {
    opusdm_ui::requester_entry_selecting(
        parent,
        "Rename to",
        initial,
        rename_selection(initial, is_dir),
        &[("OK", true), ("Cancel", false)],
        move |accepted, text| {
            if accepted && !text.is_empty() {
                on_ok(text);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Roles;

    #[test]
    fn right_click_on_copy_and_move_runs_the_as_variants() {
        use crate::keys::CommandId;
        assert_eq!(as_command(&Op::Copy), Some(CommandId::CopyAs));
        assert_eq!(as_command(&Op::Move), Some(CommandId::MoveAs));
        assert_eq!(as_command(&Op::Delete), None);
    }

    #[test]
    fn the_hidden_toggle_icon_follows_its_state() {
        assert_eq!(hidden_icon(true), crate::icons::SVG_EYE);
        assert_eq!(hidden_icon(false), crate::icons::SVG_EYE_OFF);
    }

    #[test]
    fn rename_preselects_the_name_without_its_extension() {
        assert_eq!(rename_selection("report.final.txt", false), (0, 12));
        assert_eq!(rename_selection("a.tar", false), (0, 1));
        assert_eq!(rename_selection("Makefile", false), (0, 8));
        assert_eq!(rename_selection(".bashrc", false), (0, 7));
        assert_eq!(rename_selection("è.rs", false), (0, 1), "counts characters, not bytes");
        assert_eq!(rename_selection("photos.2024", true), (0, 11), "a directory is selected whole");
    }

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
