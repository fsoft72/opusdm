//! A list of the files a theme carries (fonts, sounds) with Add and Remove.
//! Adding or removing only queues an `AssetOp`; the files change on Save.

use crate::writer::AssetOp;
use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use std::cell::RefCell;
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
    /// chosen file; `remove` queues (or cancels) the removal of a name.
    pub fn new(
        window: &gtk::ApplicationWindow,
        title: &str,
        names: Vec<String>,
        suffixes: &'static [&'static str],
        ops: Rc<RefCell<Vec<AssetOp>>>,
        add_op: fn(PathBuf) -> AssetOp,
        remove: fn(&mut Vec<AssetOp>, &str),
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some(title));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
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
                let filter = gtk::FileFilter::new();
                for suffix in suffixes {
                    filter.add_suffix(suffix);
                }
                filter.set_name(Some("Supported files"));
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder().title("Add File").filters(&filters).build();
                let list = list.clone();
                let ops = ops.clone();
                dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                    let Ok(file) = result else { return };
                    let Some(path) = file.path() else { return };
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
                remove(&mut ops.borrow_mut(), &name);
                list.remove(&row);
            });
        }
        Self { root }
    }
}
