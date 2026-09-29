//! Command Palette HUD dialog (Ctrl+P / Ctrl+Shift+P).

use crate::filter::{fuzzy_match, glob_match};
use crate::keys::CommandId;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{Op, Role, ToHub};
use std::borrow::Cow;
use std::rc::Rc;

#[derive(Clone)]
pub struct CommandItem {
    pub id: CommandId,
    pub title: Cow<'static, str>,
    pub category: Cow<'static, str>,
    pub shortcut: Option<&'static str>,
}

/// The command palette's entries. Entirely static, so this is a `const`
/// array instead of a `Vec` rebuilt (and heap-allocated) on every palette
/// open; `get_commands` just borrows it.
static COMMANDS: &[CommandItem] = &[
        CommandItem { id: CommandId::Filter, title: Cow::Borrowed("Filter Files in Current View"), category: Cow::Borrowed("View"), shortcut: Some("/") },
        CommandItem { id: CommandId::Preview, title: Cow::Borrowed("Quick Look File Preview"), category: Cow::Borrowed("View"), shortcut: Some("F3 / Space") },
        CommandItem { id: CommandId::SelectAll, title: Cow::Borrowed("Select All"), category: Cow::Borrowed("Selection"), shortcut: Some("Ctrl+A") },
        CommandItem { id: CommandId::DeselectAll, title: Cow::Borrowed("Deselect All"), category: Cow::Borrowed("Selection"), shortcut: Some("Esc") },
        CommandItem { id: CommandId::InvertSelection, title: Cow::Borrowed("Invert Selection"), category: Cow::Borrowed("Selection"), shortcut: Some("*") },
        CommandItem { id: CommandId::SelectPattern, title: Cow::Borrowed("Select by Pattern (*.rs, etc.)"), category: Cow::Borrowed("Selection"), shortcut: Some("+") },
        CommandItem { id: CommandId::DeselectPattern, title: Cow::Borrowed("Deselect by Pattern"), category: Cow::Borrowed("Selection"), shortcut: Some("-") },
        CommandItem { id: CommandId::ToggleHidden, title: Cow::Borrowed("Toggle Hidden Files"), category: Cow::Borrowed("View"), shortcut: Some("Ctrl+H") },
        CommandItem { id: CommandId::ListerFormat, title: Cow::Borrowed("Lister Format..."), category: Cow::Borrowed("View"), shortcut: None },
        CommandItem { id: CommandId::SetShowPattern, title: Cow::Borrowed("Set Show Pattern..."), category: Cow::Borrowed("View"), shortcut: None },
        CommandItem { id: CommandId::SetHidePattern, title: Cow::Borrowed("Set Hide Pattern..."), category: Cow::Borrowed("View"), shortcut: None },
        CommandItem { id: CommandId::Copy, title: Cow::Borrowed("Copy (Source -> Dest)"), category: Cow::Borrowed("File Ops"), shortcut: Some("F5") },
        CommandItem { id: CommandId::Move, title: Cow::Borrowed("Move (Source -> Dest)"), category: Cow::Borrowed("File Ops"), shortcut: Some("F6") },
        CommandItem { id: CommandId::Delete, title: Cow::Borrowed("Delete Selected Items"), category: Cow::Borrowed("File Ops"), shortcut: Some("F8 / Del") },
        CommandItem { id: CommandId::CopyAs, title: Cow::Borrowed("Copy As... (Source -> Dest, new names)"), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::MoveAs, title: Cow::Borrowed("Move As... (Source -> Dest, new names)"), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::Duplicate, title: Cow::Borrowed("Duplicate Selected Items"), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::CopyNewer, title: Cow::Borrowed("Copy (newer only, Source -> Dest)"), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::ChangePermissions, title: Cow::Borrowed("Change Permissions..."), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::SetTimestamp, title: Cow::Borrowed("Set Timestamp..."), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::MakeLink, title: Cow::Borrowed("Make Link... (hard or symbolic, Source -> Dest)"), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::JoinFiles, title: Cow::Borrowed("Join Files..."), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::SplitFile, title: Cow::Borrowed("Split File..."), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::CheckFit, title: Cow::Borrowed("Check Fit (Source -> Dest free space)..."), category: Cow::Borrowed("File Ops"), shortcut: None },
        CommandItem { id: CommandId::Reselect, title: Cow::Borrowed("Reselect Previous Selection"), category: Cow::Borrowed("Selection"), shortcut: Some("Ctrl+Shift+A") },
        CommandItem { id: CommandId::ClipCopy, title: Cow::Borrowed("Copy to Clipboard"), category: Cow::Borrowed("Clipboard"), shortcut: Some("Ctrl+C") },
        CommandItem { id: CommandId::ClipCut, title: Cow::Borrowed("Cut to Clipboard"), category: Cow::Borrowed("Clipboard"), shortcut: Some("Ctrl+X") },
        CommandItem { id: CommandId::ClipPaste, title: Cow::Borrowed("Paste from Clipboard"), category: Cow::Borrowed("Clipboard"), shortcut: Some("Ctrl+V") },
        CommandItem { id: CommandId::Mkdir, title: Cow::Borrowed("Make Directory..."), category: Cow::Borrowed("File Ops"), shortcut: Some("F7") },
        CommandItem { id: CommandId::NewFile, title: Cow::Borrowed("New File..."), category: Cow::Borrowed("File Ops"), shortcut: Some("Shift+F7") },
        CommandItem { id: CommandId::Rename, title: Cow::Borrowed("Rename..."), category: Cow::Borrowed("File Ops"), shortcut: Some("F2") },
        CommandItem { id: CommandId::BatchRename, title: Cow::Borrowed("Batch Rename Selected Files (Regex/Numbering)..."), category: Cow::Borrowed("File Ops"), shortcut: Some("F2 (multi)") },
        CommandItem { id: CommandId::Reload, title: Cow::Borrowed("Reload / Refresh Current Directory"), category: Cow::Borrowed("View"), shortcut: Some("Ctrl+R") },
        CommandItem { id: CommandId::ParentDir, title: Cow::Borrowed("Go to Parent Directory"), category: Cow::Borrowed("Navigation"), shortcut: Some("Backspace") },
        CommandItem { id: CommandId::HomeDir, title: Cow::Borrowed("Go to Home Directory (~)"), category: Cow::Borrowed("Navigation"), shortcut: Some("~") },
        CommandItem { id: CommandId::Terminal, title: Cow::Borrowed("Toggle Terminal Drawer"), category: Cow::Borrowed("Tools"), shortcut: Some("F4 / `") },
        CommandItem { id: CommandId::CalcSizes, title: Cow::Borrowed("Calculate Directory Sizes"), category: Cow::Borrowed("Tools"), shortcut: Some("Space (on dir)") },
        CommandItem { id: CommandId::CalcChecksums, title: Cow::Borrowed("Calculate Checksums (SHA-256 / MD5)..."), category: Cow::Borrowed("Tools"), shortcut: None },
        CommandItem { id: CommandId::CompareDirs, title: Cow::Borrowed("Compare / Sync Directories (Visual Diff)..."), category: Cow::Borrowed("Tools"), shortcut: None },
        CommandItem { id: CommandId::SetSource, title: Cow::Borrowed("Set Role: SOURCE"), category: Cow::Borrowed("Roles"), shortcut: None },
        CommandItem { id: CommandId::SetDest, title: Cow::Borrowed("Set Role: DEST"), category: Cow::Borrowed("Roles"), shortcut: None },
        CommandItem { id: CommandId::SetNone, title: Cow::Borrowed("Set Role: NONE"), category: Cow::Borrowed("Roles"), shortcut: None },
        CommandItem { id: CommandId::TogglePin, title: Cow::Borrowed("Toggle Role Pin / Lock"), category: Cow::Borrowed("Roles"), shortcut: None },
        CommandItem { id: CommandId::ManageFunctions, title: Cow::Borrowed("Manage Functions..."), category: Cow::Borrowed("Functions"), shortcut: None },
        CommandItem { id: CommandId::ManageHotkeys, title: Cow::Borrowed("Edit Hotkeys..."), category: Cow::Borrowed("Functions"), shortcut: None },
        CommandItem { id: CommandId::FindKey, title: Cow::Borrowed("Find Key..."), category: Cow::Borrowed("Functions"), shortcut: None },
        CommandItem { id: CommandId::ManageUserMenu, title: Cow::Borrowed("Edit User Menu..."), category: Cow::Borrowed("Functions"), shortcut: None },
        CommandItem { id: CommandId::ManageScripts, title: Cow::Borrowed("Edit Script Events..."), category: Cow::Borrowed("Functions"), shortcut: None },
];

/// Borrow the command palette's entries.
/// Every palette entry: the static commands, plus one dynamic "Function"
/// entry per saved function (re-read from `functions.json` on every call,
/// so a function created/renamed/deleted since the palette last opened is
/// always current).
pub fn get_commands() -> Vec<CommandItem> {
    let mut items: Vec<CommandItem> = COMMANDS.to_vec();
    for f in opusdm_ipc::function::load_functions() {
        items.push(CommandItem {
            id: CommandId::RunFunction(f.id),
            title: Cow::Owned(f.name),
            category: Cow::Borrowed("Function"),
            shortcut: None,
        });
    }
    items
}

/// The static built-in commands, without the dynamic per-function entries
/// (no disk read): what "Find Key" names a built-in key by.
pub fn builtin_commands() -> &'static [CommandItem] {
    COMMANDS
}

/// Execute a command by ID. The match is exhaustive (no wildcard arm): a
/// `CommandId` added without a handler here is a compile error, not a
/// silently dead-ended shortcut.
pub fn execute_command(ui: &Rc<Ui>, cmd_id: CommandId) {
    match cmd_id {
        CommandId::Filter => {
            ui.reveal_filter();
        }
        CommandId::Preview => {
            ui.toggle_preview();
        }
        CommandId::Terminal => {
            ui.toggle_terminal();
        }
        CommandId::SelectAll => {
            ui.view.selection.select_all();
        }
        CommandId::DeselectAll => {
            ui.remember_selection();
            ui.view.selection.unselect_all();
        }
        CommandId::InvertSelection => {
            let n = ui.view.selection.n_items();
            for i in 0..n {
                if ui.view.selection.is_selected(i) {
                    ui.view.selection.unselect_item(i);
                } else {
                    ui.view.selection.select_item(i, false);
                }
            }
        }
        CommandId::ListerFormat => crate::format_editor::open(ui),
        CommandId::SetShowPattern => crate::format_editor::set_pattern(ui, crate::format_editor::PatternKind::Show),
        CommandId::SetHidePattern => crate::format_editor::set_pattern(ui, crate::format_editor::PatternKind::Hide),
        CommandId::SelectPattern => {
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
        CommandId::DeselectPattern => {
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
        CommandId::ToggleHidden => {
            let active = !ui.view.show_hidden.get();
            ui.view.show_hidden.set(active);
            ui.reload();
        }
        CommandId::Copy => {
            if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
                crate::actions::flash(ui, &msg);
                return;
            }
            let srcs = ui.selected_paths();
            if srcs.is_empty() {
                crate::actions::flash(ui, "nothing selected");
                return;
            }
            crate::job_submit::submit_job(&ui, Op::Copy, srcs, None, opusdm_ipc::JobOptions::default());
        }
        CommandId::Move => {
            if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
                crate::actions::flash(ui, &msg);
                return;
            }
            let srcs = ui.selected_paths();
            if srcs.is_empty() {
                crate::actions::flash(ui, "nothing selected");
                return;
            }
            crate::job_submit::submit_job(&ui, Op::Move, srcs, None, opusdm_ipc::JobOptions::default());
        }
        CommandId::ChangePermissions => crate::attr_dialogs::open_permissions(ui),
        CommandId::SetTimestamp => crate::attr_dialogs::open_timestamp(ui),
        CommandId::MakeLink => crate::link_dialogs::open_link(ui),
        CommandId::JoinFiles => crate::link_dialogs::open_join(ui),
        CommandId::SplitFile => crate::link_dialogs::open_split(ui),
        CommandId::CheckFit => crate::checkfit_ui::start(ui),
        CommandId::Reselect => ui.reselect(),
        CommandId::CopyAs => crate::copy_as::open(ui, false),
        CommandId::MoveAs => crate::copy_as::open(ui, true),
        CommandId::Duplicate => {
            let srcs = ui.selected_paths();
            if srcs.is_empty() {
                crate::actions::flash(ui, "nothing selected");
                return;
            }
            let dest_dir = Some(ui.cwd_path());
            crate::job_submit::submit_job(ui, Op::Duplicate, srcs, dest_dir, opusdm_ipc::JobOptions::default());
        }
        CommandId::CopyNewer => {
            if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
                crate::actions::flash(ui, &msg);
                return;
            }
            let srcs = ui.selected_paths();
            if srcs.is_empty() {
                crate::actions::flash(ui, "nothing selected");
                return;
            }
            let options = opusdm_ipc::JobOptions { newer: true, ..Default::default() };
            crate::job_submit::submit_job(ui, Op::Copy, srcs, None, options);
        }
        CommandId::ClipCopy => {
            crate::clip::copy_or_cut(ui, false);
        }
        CommandId::ClipCut => {
            crate::clip::copy_or_cut(ui, true);
        }
        CommandId::ClipPaste => {
            crate::clip::paste(ui);
        }
        CommandId::Delete => {
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
        CommandId::Mkdir => {
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            crate::actions::prompt(&win, "New directory name", "", move |name| {
                let _ = ui_clone.hub.client.send(&ToHub::RunJob {
                    op: Op::MakeDir { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui_clone.cwd_path()),
                    options: opusdm_ipc::JobOptions::default(),
                });
            });
        }
        CommandId::NewFile => {
            let ui_clone = ui.clone();
            let win = ui.window.clone();
            crate::actions::prompt(&win, "New file name", "", move |name| {
                let _ = ui_clone.hub.client.send(&ToHub::RunJob {
                    op: Op::NewFile { name },
                    srcs: Vec::new(),
                    dest_dir: Some(ui_clone.cwd_path()),
                    options: opusdm_ipc::JobOptions::default(),
                });
            });
        }
        CommandId::Rename => {
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
        CommandId::BatchRename => {
            crate::batch_rename::open_batch_rename_dialog(ui);
        }
        CommandId::Reload => {
            ui.reload();
        }
        CommandId::ParentDir => {
            let parent = ui.cwd.borrow().parent().map(std::path::PathBuf::from);
            if let Some(p) = parent {
                ui.navigate_to(&p);
            }
        }
        CommandId::HomeDir => {
            if let Ok(home) = std::env::var("HOME") {
                ui.navigate_to(std::path::Path::new(&home));
            }
        }
        CommandId::CalcSizes => {
            crate::tools::calculate_dir_sizes_async(ui);
        }
        CommandId::CalcChecksums => {
            crate::tools::open_checksum_dialog(ui);
        }
        CommandId::CompareDirs => {
            crate::tools::open_compare_dialog(ui);
        }
        CommandId::SetSource => {
            let _ = ui.hub.client.send(&ToHub::SetRole(Role::Source));
        }
        CommandId::SetDest => {
            let _ = ui.hub.client.send(&ToHub::SetRole(Role::Dest));
        }
        CommandId::SetNone => {
            let _ = ui.hub.client.send(&ToHub::SetRole(Role::None));
        }
        CommandId::TogglePin => {
            let active = !ui.status_bar.role_lock_btn.is_active();
            ui.status_bar.role_lock_btn.set_active(active);
            let msg = if active { ToHub::LockRole } else { ToHub::UnlockRole };
            let _ = ui.hub.client.send(&msg);
        }
        CommandId::ManageFunctions => {
            crate::function_editor::open_functions_manager(ui);
        }
        CommandId::ManageHotkeys => {
            crate::hotkey_editor::open_hotkey_editor(ui);
        }
        CommandId::FindKey => {
            crate::hotkey_editor::open_find_key(ui);
        }
        CommandId::ManageUserMenu => {
            crate::menu_editor::open_menu_editor(ui);
        }
        CommandId::ManageScripts => {
            crate::scripts_editor::open_scripts_editor(ui);
        }
        CommandId::RunFunction(id) => {
            let Some(function) = opusdm_ipc::function::load_functions().into_iter().find(|f| f.id == id) else {
                crate::actions::flash(ui, "function not found (it may have been deleted)");
                return;
            };
            let _ = ui.hub.client.send(&ToHub::RunFunction { steps: function.steps });
        }
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

        let cat_label = gtk::Label::new(Some(cmd.category.as_ref()));
        cat_label.add_css_class("palette-cat");
        cat_label.set_size_request(80, -1);
        cat_label.set_xalign(0.0);

        let title_label = gtk::Label::new(Some(cmd.title.as_ref()));
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
                        execute_command(&ui_for_exec, cmd.id.clone());
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
