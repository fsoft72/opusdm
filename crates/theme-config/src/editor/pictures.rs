//! Pictures tab: the desktop background and the Lister / requester pictures.

use super::optional::OptionalSection;
use crate::writer::ThemeDraft;
use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_desktop_config::ui::BackgroundSection;
use opusdm_ipc::desktop::{Background, ImageFit, BACKGROUND_IMAGE_EXTS};
use opusdm_ipc::look::SurfaceImage;
use std::path::PathBuf;
use std::rc::Rc;

/// The fits a surface picture offers, in drop-down order.
const FITS: [(ImageFit, &str); 4] =
    [(ImageFit::Fill, "Fill (cover)"), (ImageFit::Fit, "Fit (contain)"), (ImageFit::Center, "Center"), (ImageFit::Tile, "Tile")];

/// Thumbnail size in pixels.
const THUMB_WIDTH: i32 = 200;
const THUMB_HEIGHT: i32 = 120;

/// Picture chooser for one surface: path, Choose button, fit and thumbnail.
struct SurfacePicker {
    root: gtk::Box,
    path: gtk::Entry,
    fit: gtk::DropDown,
}

impl SurfacePicker {
    /// Build the picker seeded from `initial`; `window` parents the file chooser.
    fn new(window: &gtk::ApplicationWindow, initial: &SurfaceImage) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let path = gtk::Entry::builder().hexpand(true).editable(false).placeholder_text("no picture").build();
        if let Some(p) = &initial.image_path {
            path.set_text(&p.to_string_lossy());
        }
        let choose = gtk::Button::with_label("Choose...");
        let clear = gtk::Button::with_label("Clear");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&path);
        row.append(&choose);
        row.append(&clear);

        let labels: Vec<&str> = FITS.iter().map(|(_, l)| *l).collect();
        let fit = gtk::DropDown::from_strings(&labels);
        fit.set_selected(FITS.iter().position(|(f, _)| *f == initial.fit).unwrap_or(0) as u32);

        let thumb = gtk::Picture::new();
        thumb.set_size_request(THUMB_WIDTH, THUMB_HEIGHT);
        thumb.set_content_fit(gtk::ContentFit::Contain);
        thumb.set_halign(gtk::Align::Start);
        if let Some(p) = &initial.image_path {
            thumb.set_filename(Some(p));
        }

        root.append(&row);
        root.append(&fit);
        root.append(&thumb);

        {
            let path = path.clone();
            let thumb = thumb.clone();
            let window = window.clone();
            choose.connect_clicked(move |_| {
                let filter = gtk::FileFilter::new();
                for ext in BACKGROUND_IMAGE_EXTS {
                    filter.add_suffix(ext);
                }
                filter.set_name(Some("Pictures"));
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder().title("Choose Picture").filters(&filters).build();
                let path = path.clone();
                let thumb = thumb.clone();
                dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                    let Ok(file) = result else { return };
                    let Some(chosen) = file.path() else { return };
                    path.set_text(&chosen.to_string_lossy());
                    thumb.set_filename(Some(&chosen));
                });
            });
        }
        {
            let path = path.clone();
            clear.connect_clicked(move |_| {
                path.set_text("");
                thumb.set_filename(None::<&std::path::Path>);
            });
        }
        Self { root, path, fit }
    }

    /// The picture and fit the widgets currently describe.
    fn read(&self) -> SurfaceImage {
        let text = self.path.text().to_string();
        SurfaceImage {
            image_path: (!text.is_empty()).then(|| PathBuf::from(text)),
            fit: FITS[self.fit.selected() as usize].0,
        }
    }
}

/// The values of the tab as written into the draft.
type Values = (Option<Background>, Option<SurfaceImage>, Option<SurfaceImage>);

/// The Pictures tab.
pub struct PicturesTab {
    pub root: gtk::ScrolledWindow,
    desktop: Rc<BackgroundSection>,
    desktop_opt: OptionalSection,
    lister: SurfacePicker,
    lister_opt: OptionalSection,
    requester: SurfacePicker,
    requester_opt: OptionalSection,
    initial: Values,
}

impl PicturesTab {
    /// Build the tab seeded from `draft`.
    pub fn new(window: &gtk::ApplicationWindow, draft: &ThemeDraft) -> Self {
        let desktop = Rc::new(BackgroundSection::new(window, &draft.background.clone().unwrap_or_default()));
        let desktop_opt = OptionalSection::new("DESKTOP BACKGROUND", &desktop.root, draft.background.is_some());
        let lister = SurfacePicker::new(window, &draft.lister.clone().unwrap_or_default());
        let lister_opt = OptionalSection::new("LISTER FILE LIST PICTURE", &lister.root, draft.lister.is_some());
        let requester = SurfacePicker::new(window, &draft.requester.clone().unwrap_or_default());
        let requester_opt = OptionalSection::new("REQUESTER PICTURE", &requester.root, draft.requester.is_some());

        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.set_margin_top(12);
        page.set_margin_bottom(12);
        page.set_margin_start(12);
        page.set_margin_end(12);
        for section in [&desktop_opt.root, &lister_opt.root, &requester_opt.root] {
            page.append(section);
            page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let root = gtk::ScrolledWindow::builder().child(&page).build();

        let mut tab = Self { root, desktop, desktop_opt, lister, lister_opt, requester, requester_opt, initial: (None, None, None) };
        tab.initial = tab.read();
        tab
    }

    /// The values of the sections that are set.
    fn read(&self) -> Values {
        (
            self.desktop_opt.is_set().then(|| self.desktop.read()),
            self.lister_opt.is_set().then(|| self.lister.read()),
            self.requester_opt.is_set().then(|| self.requester.read()),
        )
    }

    /// Write the tab's values into `draft` when they changed.
    pub fn collect_into(&self, draft: &mut ThemeDraft) {
        let now = self.read();
        if now.0 != self.initial.0 {
            draft.background = now.0;
        }
        if now.1 != self.initial.1 {
            draft.lister = now.1;
        }
        if now.2 != self.initial.2 {
            draft.requester = now.2;
        }
    }
}
