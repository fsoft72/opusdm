//! Icons tab: every icon the theme provides, with a button to replace it.

use crate::draft::queue_icon_replace;
use crate::writer::{AssetOp, ICON_NAMES};
use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::themes::theme_dir;
use std::cell::RefCell;
use std::rc::Rc;

/// Thumbnail edge in pixels.
const THUMB_SIZE: i32 = 32;

/// The Icons tab.
pub struct IconsTab {
    pub root: gtk::ScrolledWindow,
}

impl IconsTab {
    /// Build the tab for theme `slug`. Replacements are queued in `ops` and
    /// copied into the theme on Save.
    pub fn new(window: &gtk::ApplicationWindow, slug: &str, ops: &Rc<RefCell<Vec<AssetOp>>>) -> Self {
        let grid = gtk::Grid::builder().row_spacing(8).column_spacing(12).margin_top(12).margin_start(12).margin_end(12).margin_bottom(12).build();
        let icons = theme_dir(slug).join("icons");
        for (row, name) in ICON_NAMES.iter().enumerate() {
            let thumb = gtk::Picture::new();
            thumb.set_size_request(THUMB_SIZE, THUMB_SIZE);
            let current = icons.join(format!("{name}.png"));
            if current.is_file() {
                thumb.set_filename(Some(&current));
            }
            let label = gtk::Label::builder().label(*name).xalign(0.0).hexpand(true).build();
            let replace = gtk::Button::with_label("Replace...");
            grid.attach(&thumb, 0, row as i32, 1, 1);
            grid.attach(&label, 1, row as i32, 1, 1);
            grid.attach(&replace, 2, row as i32, 1, 1);

            let window = window.clone();
            let ops = ops.clone();
            replace.connect_clicked(move |_| {
                let filter = gtk::FileFilter::new();
                filter.add_suffix("png");
                filter.set_name(Some("PNG icons"));
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder().title("Replace Icon").filters(&filters).build();
                let thumb = thumb.clone();
                let ops = ops.clone();
                dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                    let Ok(file) = result else { return };
                    let Some(path) = file.path() else { return };
                    thumb.set_filename(Some(&path));
                    queue_icon_replace(&mut ops.borrow_mut(), name, path);
                });
            });
        }
        Self { root: gtk::ScrolledWindow::builder().child(&grid).build() }
    }
}
