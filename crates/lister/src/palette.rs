//! Command Palette HUD dialog (Ctrl+P / Ctrl+Shift+P).

use crate::filter::{fuzzy_match, glob_match};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{Op, Role, ToHub};
use std::rc::Rc;

#[derive(Clone)]
pub struct CommandItem {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub shortcut: Option<&'static str>,
}

pub fn get_commands() -> Vec<CommandItem> {
    vec![
        CommandItem { id: "filter", title: "Filter Files in Current View", category: "View", shortcut: Some("/") },
        CommandItem { id: "preview", title: "Quick Look File Preview", category: "View", shortcut: Some("F3 / Space") },
        CommandItem { id: "select_all", title: "Select All", category: "Selection", shortcut: Some("Ctrl+A") },
        CommandItem { id: "deselect_all", title: "Deselect All", category: "Selection", shortcut: Some("Esc") },
        CommandItem { id: "invert_selection", title: "Invert Selection", category: "Selection", shortcut: Some("*") },
        CommandItem { id: "select_pattern", title: "Select by Pattern (*.rs, etc.)", category: "Selection", shortcut: Some("+") },
        CommandItem { id: "deselect_pattern", title: "Deselect by Pattern", category: "Selection", shortcut: Some("-") },
        CommandItem { id: "toggle_hidden", title: "Toggle Hidden Files", category: "View", shortcut: Some("Ctrl+H") },
        CommandItem { id: "copy", title: "Copy (Source -> Dest)", category: "File Ops", shortcut: Some("c") },
        CommandItem { id: "move", title: "Move (Source -> Dest)", category: "File Ops", shortcut: Some("m") },
        CommandItem { id: "delete", title: "Delete Selected Items", category: "File Ops", shortcut: Some("d / Del") },
        CommandItem { id: "clip_copy", title: "Copy to Clipboard", category: "Clipboard", shortcut: Some("Ctrl+C") },
        CommandItem { id: "clip_cut", title: "Cut to Clipboard", category: "Clipboard", shortcut: Some("Ctrl+X") },
        CommandItem { id: "clip_paste", title: "Paste from Clipboard", category: "Clipboard", shortcut: Some("Ctrl+V") },
        CommandItem { id: "mkdir", title: "Make Directory...", category: "File Ops", shortcut: Some("F7") },
        CommandItem { id: "newfile", title: "New File...", category: "File Ops", shortcut: Some("Shift+F7") },
        CommandItem { id: "rename", title: "Rename...", category: "File Ops", shortcut: Some("F2") },
        CommandItem { id: "batch_rename", title: "Batch Rename Selected Files (Regex/Numbering)...", category: "File Ops", shortcut: Some("F2 (multi)") },
        CommandItem { id: "reload", title: "Reload / Refresh Current Directory", category: "View", shortcut: Some("Ctrl+R") },
        CommandItem { id: "parent_dir", title: "Go to Parent Directory", category: "Navigation", shortcut: Some("h / Backspace") },
        CommandItem { id: "home_dir", title: "Go to Home Directory (~)", category: "Navigation", shortcut: Some("~") },
        CommandItem { id: "terminal", title: "Toggle Terminal Drawer", category: "Tools", shortcut: Some("F4 / `") },
        CommandItem { id: "calc_sizes", title: "Calculate Directory Sizes", category: "Tools", shortcut: Some("Space (on dir)") },
        CommandItem { id: "calc_checksums", title: "Calculate Checksums (SHA-256 / MD5)...", category: "Tools", shortcut: None },
        CommandItem { id: "compare_dirs", title: "Compare / Sync Directories (Visual Diff)...", category: "Tools", shortcut: None },
        CommandItem { id: "set_source", title: "Set Role: SOURCE", category: "Roles", shortcut: None },
        CommandItem { id: "set_dest", title: "Set Role: DEST", category: "Roles", shortcut: None },
        CommandItem { id: "set_none", title: "Set Role: NONE", category: "Roles", shortcut: None },
        CommandItem { id: "toggle_pin", title: "Toggle Role Pin / Lock", category: "Roles", shortcut: None },
    ]
}

/// Execute a command by ID.
pub fn execute_command(ui: &Rc<Ui>, cmd_id: &str) {
    match cmd_id {
        "filter" => {
            ui.reveal_filter();
        }
        "preview" => {
            ui.toggle_preview();
        }
        "terminal" => {
            ui.toggle_terminal();
        }
        "select_all" => {
            ui.view.selection.select_all();
        }
        "deselect_all" => {
            ui.view.selection.unselect_all();
        }
        "invert_selection" => {
            let n = ui.view.selection.n_items();
            for i in 0..n {
                if ui.view.selection.is_selected(i) {
                    ui.view.selection.unselect_item(i);
                } else {
                    ui.view.selection.select_item(i, false);
                }
            }
        }
        "select_pattern" => {
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            crate::actions::prompt(&win, "Select files matching pattern (e.g. *.rs)", "*", move |pattern| {
                let n = ui_clone.view.selection.n_items();
                for i in 0..n {
                    if let Some(row) = ui_clone.row_at(i) {
                        if glob_match(&pattern, &row.name) {
                            ui_clone.view.selection.select_item(i, false);
                        }
                    }
                }
            });
        }
        "deselect_pattern" => {
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            crate::actions::prompt(&win, "Deselect files matching pattern (e.g. *.tmp)", "*", move |pattern| {
                let n = ui_clone.view.selection.n_items();
                for i in 0..n {
                    if let Some(row) = ui_clone.row_at(i) {
                        if glob_match(&pattern, &row.name) {
                            ui_clone.view.selection.unselect_item(i);
                        }
                    }
                }
            });
        }
        "toggle_hidden" => {
            let active = !ui.view.show_hidden.get();
            ui.view.show_hidden.set(active);
            ui.reload();
        }
        "copy" => {
            if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
                crate::actions::flash(ui, &msg);
                return;
            }
            let srcs = ui.selected_paths();
            if srcs.is_empty() {
                crate::actions::flash(ui, "nothing selected");
                return;
            }
            let _ = ui.hub.client.send(&ToHub::RunJob {
                op: Op::Copy,
                srcs,
                dest_dir: None,
            });
        }
        "move" => {
            if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
                crate::actions::flash(ui, &msg);
                return;
            }
            let srcs = ui.selected_paths();
            if srcs.is_empty() {
                crate::actions::flash(ui, "nothing selected");
                return;
            }
            let _ = ui.hub.client.send(&ToHub::RunJob {
                op: Op::Move,
                srcs,
                dest_dir: None,
            });
        }
        "clip_copy" => {
            crate::clip::copy_or_cut(ui, false);
        }
        "clip_cut" => {
            crate::clip::copy_or_cut(ui, true);
        }
        "clip_paste" => {
            crate::clip::paste(ui);
        }
        "delete" => {
            let paths = ui.selected_paths();
            if paths.is_empty() {
                crate::actions::flash(ui, "nothing selected");
                return;
            }
            crate::actions::confirm_delete(
                ui,
                &format!("Delete {} item(s) permanently?", paths.len()),
                paths,
            );
        }
        "mkdir" => {
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            crate::actions::prompt(&win, "New directory name", "", move |name| {
                let _ = ui_clone.hub.client.send(&ToHub::RunJob {
                    op: Op::MakeDir { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui_clone.cwd_path()),
                });
            });
        }
        "newfile" => {
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            crate::actions::prompt(&win, "New file name", "", move |name| {
                let _ = ui_clone.hub.client.send(&ToHub::RunJob {
                    op: Op::NewFile { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui_clone.cwd_path()),
                });
            });
        }
        "rename" => {
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
            let old_name = old
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            crate::actions::prompt(&win, "Rename to", &old_name, move |new_name| {
                let new_path = ui_clone.cwd_path().join(&new_name);
                match std::fs::rename(&old, &new_path) {
                    Ok(()) => ui_clone.reload(),
                    Err(e) => crate::actions::flash(&ui_clone, &format!("rename failed: {e}")),
                }
            });
        }
        "batch_rename" => {
            crate::batch_rename::open_batch_rename_dialog(ui);
        }
        "reload" => {
            ui.reload();
        }
        "parent_dir" => {
            let parent = ui.cwd.borrow().parent().map(std::path::PathBuf::from);
            if let Some(p) = parent {
                ui.navigate_to(&p);
            }
        }
        "home_dir" => {
            if let Ok(home) = std::env::var("HOME") {
                ui.navigate_to(std::path::Path::new(&home));
            }
        }
        "calc_sizes" => {
            crate::tools::calculate_dir_sizes_async(ui);
        }
        "calc_checksums" => {
            crate::tools::open_checksum_dialog(ui);
        }
        "compare_dirs" => {
            crate::tools::open_compare_dialog(ui);
        }
        "set_source" => {
            let _ = ui.hub.client.send(&ToHub::SetRole(Role::Source));
        }
        "set_dest" => {
            let _ = ui.hub.client.send(&ToHub::SetRole(Role::Dest));
        }
        "set_none" => {
            let _ = ui.hub.client.send(&ToHub::SetRole(Role::None));
        }
        "toggle_pin" => {
            let active = !ui.status_bar.role_lock_btn.is_active();
            ui.status_bar.role_lock_btn.set_active(active);
            let msg = if active { ToHub::LockRole } else { ToHub::UnlockRole };
            let _ = ui.hub.client.send(&msg);
        }
        _ => {}
    }
}

/// Open the Command Palette modal HUD.
pub fn open_command_palette(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "OpusDM Command Palette")
        .size(520, 380)
        .resizable(false);
    let root = shell.content();
    root.add_css_class("command-palette");

    let entry = gtk::SearchEntry::new();
    entry.set_placeholder_text(Some("Type a command... (Esc to cancel)"));
    entry.add_css_class("palette-search");
    root.append(&entry);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::Single);
    list_box.add_css_class("palette-list");

    let commands = get_commands();
    let mut rows: Vec<(gtk::ListBoxRow, CommandItem)> = Vec::new();

    for cmd in &commands {
        let row = gtk::ListBoxRow::new();
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);
        hbox.set_margin_top(6);
        hbox.set_margin_bottom(6);

        let cat_label = gtk::Label::new(Some(cmd.category));
        cat_label.add_css_class("palette-cat");
        cat_label.set_size_request(80, -1);
        cat_label.set_xalign(0.0);

        let title_label = gtk::Label::new(Some(cmd.title));
        title_label.set_xalign(0.0);
        title_label.set_hexpand(true);
        title_label.add_css_class("palette-title");

        hbox.append(&cat_label);
        hbox.append(&title_label);

        if let Some(sc) = cmd.shortcut {
            let sc_label = gtk::Label::new(Some(sc));
            sc_label.add_css_class("palette-shortcut");
            hbox.append(&sc_label);
        }

        row.set_child(Some(&hbox));
        list_box.append(&row);
        rows.push((row, cmd.clone()));
    }

    let scroller = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .vexpand(true)
        .hexpand(true)
        .build();
    root.append(&scroller);

    // Initial selection
    if let Some((first_row, _)) = rows.first() {
        list_box.select_row(Some(first_row));
    }

    // Filter list as search entry changes
    {
        let rows_ref = rows.clone();
        let list_box_clone = list_box.clone();
        entry.connect_search_changed(move |e| {
            let q = e.text().to_string();
            let mut first_visible: Option<&gtk::ListBoxRow> = None;
            for (row, cmd) in &rows_ref {
                let haystack = format!("{} {}", cmd.title, cmd.category);
                let visible = fuzzy_match(&q, &haystack);
                row.set_visible(visible);
                if visible && first_visible.is_none() {
                    first_visible = Some(row);
                }
            }
            if let Some(r) = first_visible {
                list_box_clone.select_row(Some(r));
            }
        });
    }

    // Execute selected row helper
    let execute_selected = {
        let ui_for_exec = ui.clone();
        let dlg = shell.window().clone();
        let list_box_clone = list_box.clone();
        let rows_for_exec = rows.clone();
        Rc::new(move || {
            if let Some(selected_row) = list_box_clone.selected_row() {
                for (r, cmd) in &rows_for_exec {
                    if *r == selected_row {
                        dlg.close();
                        execute_command(&ui_for_exec, cmd.id);
                        break;
                    }
                }
            }
        })
    };

    // Activate on row click
    {
        let exec = execute_selected.clone();
        list_box.connect_row_activated(move |_, _| {
            exec();
        });
    }

    // Key handling for Enter, Esc, Up, Down in SearchEntry
    {
        let key_ctrl = gtk::EventControllerKey::new();
        let dlg = shell.window().clone();
        let list_box_clone = list_box.clone();
        let rows_clone = rows.clone();
        let exec = execute_selected.clone();

        key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            match keyval {
                gdk::Key::Escape => {
                    dlg.close();
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    exec();
                    glib::Propagation::Stop
                }
                gdk::Key::Down => {
                    // Move to next visible row
                    let visible_rows: Vec<&gtk::ListBoxRow> = rows_clone
                        .iter()
                        .filter(|(r, _)| r.is_visible())
                        .map(|(r, _)| r)
                        .collect();
                    if let Some(sel) = list_box_clone.selected_row() {
                        if let Some(pos) = visible_rows.iter().position(|r| **r == sel) {
                            if pos + 1 < visible_rows.len() {
                                list_box_clone.select_row(Some(visible_rows[pos + 1]));
                            }
                        }
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    // Move to previous visible row
                    let visible_rows: Vec<&gtk::ListBoxRow> = rows_clone
                        .iter()
                        .filter(|(r, _)| r.is_visible())
                        .map(|(r, _)| r)
                        .collect();
                    if let Some(sel) = list_box_clone.selected_row() {
                        if let Some(pos) = visible_rows.iter().position(|r| **r == sel) {
                            if pos > 0 {
                                list_box_clone.select_row(Some(visible_rows[pos - 1]));
                            }
                        }
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        entry.add_controller(key_ctrl);
    }

    shell.present();
    entry.grab_focus();
}
