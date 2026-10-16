//! Icons tab: every icon the theme provides, with a button to replace it.

use crate::draft::queue_icon_replace;
use crate::writer::{AssetOp, ICON_NAMES};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ui::{pick_file, FileFilterSpec, FilePick};
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
                let pick = FilePick {
                    title: "Replace Icon",
                    filter: Some(FileFilterSpec { name: "PNG icons", suffixes: &["png"], ..Default::default() }),
                    ..Default::default()
                };
                let thumb = thumb.clone();
                let ops = ops.clone();
                pick_file(Some(window.upcast_ref()), &pick, move |path| {
                    thumb.set_filename(Some(&path));
                    queue_icon_replace(&mut ops.borrow_mut(), name, path);
                });
            });
        }
        Self { root: gtk::ScrolledWindow::builder().child(&grid).build() }
    }
}
