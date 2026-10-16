//! A list of the files a theme carries (fonts, sounds) with Add and Remove.
//! Adding or removing only queues an `AssetOp`; the files change on Save.

use crate::writer::AssetOp;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ui::{pick_file, FileFilterSpec, FilePick};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

/// The widget: a heading, the file names, and Add / Remove buttons.
pub struct AssetList {
    pub root: gtk::Box,
}

/// The file name shown in `row`.
fn row_name(row: &gtk::ListBoxRow) -> Option<String> {
    row.child().and_downcast::<gtk::Label>().map(|l| l.text().to_string())
}

/// Append a row showing `name`.
fn append_name(list: &gtk::ListBox, name: &str) {
    list.append(&gtk::Label::builder().label(name).xalign(0.0).margin_start(6).margin_top(3).margin_bottom(3).build());
}

/// Whether `list` already has a row showing `name`.
fn has_name(list: &gtk::ListBox, name: &str) -> bool {
    let mut row = list.first_child();
    while let Some(r) = row {
        if r.downcast_ref::<gtk::ListBoxRow>().and_then(row_name).as_deref() == Some(name) {
            return true;
        }
        row = r.next_sibling();
    }
    false
}

impl AssetList {
    /// Build the list under `title`, showing `names`. `suffixes` are the file
    /// extensions the Add chooser offers. `add_op` builds the operation for a
    /// chosen file; `remove` queues the removal of a name, told whether a file
    /// of that name was installed when the list was built.
    pub fn new(
        window: &gtk::ApplicationWindow,
        title: &str,
        names: Vec<String>,
        suffixes: &'static [&'static str],
        ops: Rc<RefCell<Vec<AssetOp>>>,
        add_op: fn(PathBuf) -> AssetOp,
        remove: fn(&mut Vec<AssetOp>, &str, bool),
    ) -> Self {
        let installed: HashSet<String> = names.iter().cloned().collect();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = opusdm_ui::section_heading(title);
        root.append(&heading);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        for name in &names {
            append_name(&list, name);
        }
        root.append(&gtk::ScrolledWindow::builder().child(&list).min_content_height(120).build());

        let add_btn = gtk::Button::with_label("Add...");
        let remove_btn = gtk::Button::with_label("Remove");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.append(&add_btn);
        buttons.append(&remove_btn);
        root.append(&buttons);

        {
            let window = window.clone();
            let list = list.clone();
            let ops = ops.clone();
            add_btn.connect_clicked(move |_| {
                let pick = FilePick {
                    title: "Add File",
                    filter: Some(FileFilterSpec { name: "Supported files", suffixes, ..Default::default() }),
                    ..Default::default()
                };
                let list = list.clone();
                let ops = ops.clone();
                pick_file(Some(window.upcast_ref()), &pick, move |path| {
                    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { return };
                    ops.borrow_mut().push(add_op(path));
                    if !has_name(&list, &name) {
                        append_name(&list, &name);
                    }
                });
            });
        }
        {
            let list = list.clone();
            remove_btn.connect_clicked(move |_| {
                let Some(row) = list.selected_row() else { return };
                let Some(name) = row_name(&row) else { return };
                remove(&mut ops.borrow_mut(), &name, installed.contains(&name));
                list.remove(&row);
            });
        }
        Self { root }
    }
}
