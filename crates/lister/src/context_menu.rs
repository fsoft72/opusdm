//! Context menu (right-click / secondary click) on Lister rows.

use crate::model::Row;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use opusdm_ipc::Op;
use std::path::PathBuf;
use std::rc::Rc;

/// Show the context menu for a row.
pub fn show_context_menu(
    ui: &Rc<Ui>,
    anchor: &(impl IsA<gtk::Widget> + Clone),
    row: &Row,
    target_path: PathBuf,
    x: f64,
    y: f64,
) {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    popover.set_has_arrow(true);
    let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
    popover.set_pointing_to(Some(&rect));

    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.add_css_class("context-menu");

    // Helper to create flat menu action buttons
    fn menu_button(title: &str) -> gtk::Button {
        let btn = gtk::Button::with_label(title);
        btn.add_css_class("flat");
        btn.set_halign(gtk::Align::Fill);
        if let Some(child) = btn.child().and_downcast::<gtk::Label>() {
            child.set_xalign(0.0);
        }
        btn
    }

    // 1. Azioni rapide: Copy, Rename, Move, Delete
    // Copy
    {
        let ui = ui.clone();
        let pop = popover.clone();
        let target_path = target_path.clone();
        let btn = menu_button("Copy (Source -> Dest)");
        btn.connect_clicked(move |_| {
            pop.popdown();
            if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
                crate::actions::flash(&ui, &msg);
                return;
            }
            crate::job_submit::submit_job(&ui, Op::Copy, vec![target_path.clone()], None, opusdm_ipc::JobOptions::default());
        });
        menu_box.append(&btn);
    }

    // Rename
    {
        let ui = ui.clone();
        let pop = popover.clone();
        let target_path = target_path.clone();
        let old_name = row.name.clone();
        let btn = menu_button("Rename...");
        btn.connect_clicked(move |_| {
            pop.popdown();
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            let old = target_path.clone();
            crate::actions::prompt(&win, "Rename to", &old_name, move |new_name| {
                let new_path = ui_clone.cwd_path().join(&new_name);
                match std::fs::rename(&old, &new_path) {
                    Ok(()) => ui_clone.reload(),
                    Err(e) => crate::actions::flash(&ui_clone, &format!("rename failed: {e}")),
                }
            });
        });
        menu_box.append(&btn);
    }

    // Move
    {
        let ui = ui.clone();
        let pop = popover.clone();
        let target_path = target_path.clone();
        let btn = menu_button("Move (Source -> Dest)");
        btn.connect_clicked(move |_| {
            pop.popdown();
            if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
                crate::actions::flash(&ui, &msg);
                return;
            }
            crate::job_submit::submit_job(&ui, Op::Move, vec![target_path.clone()], None, opusdm_ipc::JobOptions::default());
        });
        menu_box.append(&btn);
    }

    // Delete
    {
        let ui = ui.clone();
        let pop = popover.clone();
        let target_path = target_path.clone();
        let file_name = row.name.clone();
        let btn = menu_button("Delete...");
        btn.connect_clicked(move |_| {
            pop.popdown();
            crate::actions::confirm_delete(
                &ui,
                &format!("Delete \"{}\" permanently?", file_name),
                vec![target_path.clone()],
            );
        });
        menu_box.append(&btn);
    }

    // 2. Separatore
    menu_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // 3. Quick Look
    {
        let ui = ui.clone();
        let pop = popover.clone();
        let btn = menu_button("Quick Look (Preview)");
        btn.connect_clicked(move |_| {
            pop.popdown();
            ui.toggle_preview();
        });
        menu_box.append(&btn);
    }

    // Set as Desktop Background (image files only). The path goes in as a
    // literal template, so a name holding a `{...}` placeholder is refused
    // instead of being expanded.
    if !row.is_dir && opusdm_ipc::is_background_image(&target_path) {
        let ui = ui.clone();
        let pop = popover.clone();
        let target_path = target_path.clone();
        let btn = menu_button("Set as Desktop Background");
        btn.connect_clicked(move |_| {
            pop.popdown();
            let text = target_path.to_string_lossy().into_owned();
            if text.contains('{') {
                crate::actions::flash(&ui, "cannot set a background from a path containing '{'");
                return;
            }
            let mut args = std::collections::BTreeMap::new();
            args.insert("path".to_string(), text);
            let steps = vec![opusdm_ipc::function::FnStep::Internal {
                cmd: opusdm_ipc::function::InternalCmd::SetBackground,
                args: opusdm_ipc::function::InternalArgs(args),
            }];
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::RunFunction { steps });
        });
        menu_box.append(&btn);
    }

    // 4. Checksums & Dimensioni cartelle (solo per directories)
    if row.is_dir {
        let ui = ui.clone();
        let pop = popover.clone();
        let btn = menu_button("Checksums & Dimensioni cartelle");
        btn.connect_clicked(move |_| {
            pop.popdown();
            crate::tools::calculate_dir_sizes_async(&ui);
        });
        menu_box.append(&btn);
    }

    popover.set_child(Some(&menu_box));
    popover.popup();
}
