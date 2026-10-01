//! Fonts tab: the font family and size for the file list and the UI, and the
//! font files shipped in the theme.

use super::asset_list::AssetList;
use super::optional::OptionalSection;
use crate::draft::{list_names, queue_remove_font};
use crate::writer::{AssetOp, ThemeDraft};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::look::{FontSpec, Fonts};
use opusdm_ipc::themes::theme_dir;
use std::cell::RefCell;
use std::rc::Rc;

/// Largest size the spin buttons offer, in points.
const MAX_SIZE: f64 = 72.0;

/// One family entry plus size spin button.
struct FontRow {
    family: gtk::Entry,
    size: gtk::SpinButton,
}

impl FontRow {
    /// Attach a labelled row to `grid` at `row`, seeded from `spec`.
    fn new(grid: &gtk::Grid, row: i32, caption: &str, spec: &FontSpec) -> Self {
        let label = gtk::Label::builder().label(caption).xalign(0.0).build();
        let family = gtk::Entry::builder().text(&spec.family).placeholder_text("system default").hexpand(true).build();
        let size = gtk::SpinButton::with_range(0.0, MAX_SIZE, 1.0);
        size.set_value(f64::from(spec.size));
        grid.attach(&label, 0, row, 1, 1);
        grid.attach(&family, 1, row, 1, 1);
        grid.attach(&size, 2, row, 1, 1);
        Self { family, size }
    }

    /// The font the row describes now.
    fn read(&self) -> FontSpec {
        FontSpec { family: self.family.text().trim().to_string(), size: self.size.value_as_int().max(0) as u32 }
    }
}

/// The Fonts tab.
pub struct FontsTab {
    pub root: gtk::ScrolledWindow,
    opt: OptionalSection,
    list: FontRow,
    ui: FontRow,
    initial: Option<Fonts>,
}

impl FontsTab {
    /// Build the tab for theme `slug`, seeded from `draft`. Added or removed
    /// font files are queued in `ops`.
    pub fn new(window: &gtk::ApplicationWindow, slug: &str, draft: &ThemeDraft, ops: &Rc<RefCell<Vec<AssetOp>>>) -> Self {
        let fonts = draft.fonts.clone().unwrap_or_default();
        let grid = gtk::Grid::builder().row_spacing(8).column_spacing(12).build();
        let list = FontRow::new(&grid, 0, "File list", &fonts.list);
        let ui = FontRow::new(&grid, 1, "Interface", &fonts.ui);
        let opt = OptionalSection::new("FONTS", &grid, draft.fonts.is_some());

        let files = AssetList::new(
            window,
            "FONT FILES (.ttf, .otf), registered for OpusDM only",
            list_names(&theme_dir(slug).join("fonts")),
            &["ttf", "otf"],
            ops.clone(),
            AssetOp::AddFont,
            queue_remove_font,
        );

        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.set_margin_top(12);
        page.set_margin_bottom(12);
        page.set_margin_start(12);
        page.set_margin_end(12);
        page.append(&opt.root);
        page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        page.append(&files.root);
        let root = gtk::ScrolledWindow::builder().child(&page).build();

        let mut tab = Self { root, opt, list, ui, initial: None };
        tab.initial = tab.read();
        tab
    }

    /// The fonts the widgets describe now; `None` when the theme sets none.
    fn read(&self) -> Option<Fonts> {
        self.opt.is_set().then(|| Fonts { list: self.list.read(), ui: self.ui.read() })
    }

    /// Write the tab's values into `draft` when they changed.
    pub fn collect_into(&self, draft: &mut ThemeDraft) {
        let now = self.read();
        if now != self.initial {
            draft.fonts = now;
        }
    }
}
