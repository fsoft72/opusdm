//! Context menu (right-click / secondary click) on Lister rows.

use crate::model::Row;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use opusdm_ipc::Op;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The popover being filled: each item closes it, then runs its action.
struct Menu {
    ui: Rc<Ui>,
    popover: gtk::Popover,
    items: gtk::Box,
}

impl Menu {
    /// An empty menu popping up at (`x`, `y`) of `anchor`.
    fn new(ui: &Rc<Ui>, anchor: &impl IsA<gtk::Widget>, x: f64, y: f64) -> Self {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        popover.set_has_arrow(true);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
        items.set_margin_start(4);
        items.set_margin_end(4);
        items.set_margin_top(4);
        items.set_margin_bottom(4);
        items.add_css_class("context-menu");
        Self { ui: ui.clone(), popover, items }
    }

    /// Add a flat, left-aligned button that closes the menu and runs `action`.
    fn item(&self, title: &str, action: impl Fn(&Rc<Ui>) + 'static) {
        let btn = gtk::Button::with_label(title);
        btn.add_css_class("flat");
        btn.set_halign(gtk::Align::Fill);
        if let Some(child) = btn.child().and_downcast::<gtk::Label>() {
            child.set_xalign(0.0);
        }
        let ui = self.ui.clone();
        let popover = self.popover.clone();
        btn.connect_clicked(move |_| {
            popover.popdown();
            action(&ui);
        });
        self.items.append(&btn);
    }

    /// Add a horizontal separator.
    fn separator(&self) {
        self.items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }

    /// Show the menu.
    fn popup(self) {
        self.popover.set_child(Some(&self.items));
        self.popover.popup();
    }
}

/// Copy or move `path` to the DEST Lister, when there is one.
fn transfer(ui: &Rc<Ui>, op: Op, path: &Path) {
    if let Err(msg) = crate::actions::resolve_copy_target(&ui.hub.roles.borrow(), ui.hub.id) {
        crate::actions::flash(ui, &msg);
        return;
    }
    crate::job_submit::submit_job(ui, op, vec![path.to_path_buf()], None, opusdm_ipc::JobOptions::default());
}

/// Set `path` as the desktop background. The path goes in as a literal
/// template, so a name holding a `{...}` placeholder is refused instead of
/// being expanded.
fn set_background(ui: &Rc<Ui>, path: &Path) {
    let text = path.to_string_lossy().into_owned();
    if text.contains('{') {
        crate::actions::flash(ui, "cannot set a background from a path containing '{'");
        return;
    }
    let mut args = std::collections::BTreeMap::new();
    args.insert("path".to_string(), text);
    let steps = vec![opusdm_ipc::function::FnStep::Internal {
        cmd: opusdm_ipc::function::InternalCmd::SetBackground,
        args: opusdm_ipc::function::InternalArgs(args),
    }];
    ui.start_run(opusdm_ipc::ToHub::RunFunction { steps });
}

/// Leave the selection (when it holds `path`, else `path` alone) out on the
/// desktop. A path that cannot be left out (inside an archive, say) is
/// refused with a message.
fn leave_out(ui: &Rc<Ui>, path: &Path) {
    let paths = crate::crypt_dialog::targets_for(ui.selected_paths(), path);
    if let Err(msg) = opusdm_ipc::leftout::check_leavable(&paths) {
        crate::actions::flash(ui, &msg);
        return;
    }
    if let Err(e) = ui.hub.client.send(&opusdm_ipc::ToHub::LeaveOut { paths }) {
        crate::actions::flash(ui, &format!("lost the connection to the hub: {e}"));
    }
}

/// Show the context menu for a row.
pub fn show_context_menu(
    ui: &Rc<Ui>,
    anchor: &(impl IsA<gtk::Widget> + Clone),
    row: &Row,
    target_path: PathBuf,
    x: f64,
    y: f64,
) {
    let menu = Menu::new(ui, anchor, x, y);
    let target = Rc::new(target_path);
    // The selection when it holds the clicked row, else the row alone.
    let targets = {
        let target = target.clone();
        move |ui: &Rc<Ui>| crate::crypt_dialog::targets_for(ui.selected_paths(), &target)
    };

    // Quick actions: Copy, Rename, Move, Delete.
    let t = target.clone();
    menu.item("Copy (Source -> Dest)", move |ui| transfer(ui, Op::Copy, &t));
    let (t, old_name) = (target.clone(), row.name.clone());
    menu.item("Rename...", move |ui| {
        let (ui, old) = (ui.clone(), t.to_path_buf());
        crate::actions::prompt(&ui.window.clone(), "Rename to", &old_name, move |new_name| {
            crate::actions::rename_and_reload(&ui, &old, &new_name)
        });
    });
    let t = target.clone();
    menu.item("Move (Source -> Dest)", move |ui| transfer(ui, Op::Move, &t));
    let (t, file_name) = (target.clone(), row.name.clone());
    menu.item("Delete...", move |ui| {
        crate::actions::confirm_delete(ui, &format!("Delete \"{file_name}\" permanently?"), vec![t.to_path_buf()]);
    });

    // File type "Open With" entries: one per function of the type the file matches.
    if let Some(file_type) = crate::filetypes_ui::type_for(&target, row) {
        let entries = crate::filetypes_ui::menu_entries(&file_type, &ui.hub.functions.borrow());
        if !entries.is_empty() {
            menu.separator();
        }
        for (function, label) in entries {
            let name = row.name.clone();
            menu.item(&format!("Open With: {label}"), move |ui| crate::filetypes_ui::run_on_row(ui, &name, &function));
        }
    }

    let tg = targets.clone();
    menu.item("Compress...", move |ui| crate::compress_dialog::open_compress(ui, tg(ui)));
    // Offered on a supported archive file (not on one already inside an archive).
    if !row.is_dir && !crate::extract_dialog::extractable(vec![target.to_path_buf()]).is_empty() {
        let tg = targets.clone();
        menu.item("Extract archive", move |ui| crate::extract_dialog::open_extract(ui, tg(ui)));
    }
    let tg = targets.clone();
    menu.item("Encrypt...", move |ui| crate::crypt_dialog::open_crypt(ui, true, tg(ui)));
    if !row.is_dir && row.name.to_ascii_lowercase().ends_with(opusdm_fsops::crypt::ENC_SUFFIX) {
        let tg = targets.clone();
        menu.item("Decrypt...", move |ui| crate::crypt_dialog::open_crypt(ui, false, tg(ui)));
    }

    menu.separator();
    menu.item("Quick Look (Preview)", |ui| ui.toggle_preview());
    if !row.is_dir && opusdm_ipc::is_background_media(&target) {
        let t = target.clone();
        menu.item("Set as Desktop Background", move |ui| set_background(ui, &t));
    }
    let t = target.clone();
    menu.item("Leave Out on Desktop", move |ui| leave_out(ui, &t));
    if row.is_dir {
        menu.item("Checksums & Dimensioni cartelle", |ui| crate::tools::calculate_dir_sizes_async(ui, false));
    }
    menu.popup();
}
