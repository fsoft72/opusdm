//! Hub tab: the paint of the hub bar (flat colour, gradient or picture), the
//! picture of the start menu button and the font of the start menu.

use super::fonts::FontRow;
use super::optional::OptionalSection;
use super::pictures::SurfacePicker;
use crate::writer::{ThemeDraft, HUB_ICON_EXTS};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_desktop_config::ui::ColorRow;
use opusdm_ipc::look::{FontSpec, HubBar, HubBarKind, HubLook};
use opusdm_ipc::GRADIENT_ANGLE_MAX;
use opusdm_ui::{pick_file, FileFilterSpec, FilePick};
use std::path::PathBuf;
use std::rc::Rc;

/// The kinds of bar paint, in dropdown order.
const KINDS: [(HubBarKind, &str); 3] =
    [(HubBarKind::Solid, "Colour"), (HubBarKind::Gradient, "Gradient"), (HubBarKind::Image, "Picture")];

/// Largest start menu font size, in points.
const MAX_SIZE: f64 = 72.0;

/// Size of the start menu icon preview, in pixels.
const ICON_PREVIEW: i32 = 32;

/// Editor of the hub bar paint.
struct BarEditor {
    root: gtk::Box,
    kind: gtk::DropDown,
    color: Rc<ColorRow>,
    start: Rc<ColorRow>,
    end: Rc<ColorRow>,
    angle: gtk::SpinButton,
    image: SurfacePicker,
}

impl BarEditor {
    /// Build the editor seeded from `initial`; `window` parents the dialogs.
    fn new(window: &gtk::ApplicationWindow, initial: &HubBar) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let labels: Vec<&str> = KINDS.iter().map(|(_, l)| *l).collect();
        let kind = gtk::DropDown::from_strings(&labels);
        kind.set_selected(KINDS.iter().position(|(k, _)| *k == initial.kind).unwrap_or(0) as u32);
        kind.set_halign(gtk::Align::Start);

        let color = Rc::new(ColorRow::new(window, Some("Colour"), &initial.color));
        let start = Rc::new(ColorRow::new(window, Some("Start"), &initial.start));
        let end = Rc::new(ColorRow::new(window, Some("End"), &initial.end));
        let angle = gtk::SpinButton::with_range(0.0, f64::from(GRADIENT_ANGLE_MAX), 1.0);
        angle.set_value(f64::from(initial.angle));
        let angle_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        angle_row.append(&gtk::Label::new(Some("Angle")));
        angle_row.append(&angle);
        let gradient = gtk::Box::new(gtk::Orientation::Vertical, 4);
        gradient.append(&start.root);
        gradient.append(&end.root);
        gradient.append(&angle_row);
        let image = SurfacePicker::new(window, &initial.image);

        root.append(&kind);
        root.append(&color.root);
        root.append(&gradient);
        root.append(&image.root);

        // Only the controls of the chosen kind are shown.
        let show = {
            let (solid, gradient, picture) = (color.root.clone(), gradient.clone(), image.root.clone());
            move |selected: u32| {
                let kind = KINDS.get(selected as usize).map_or(HubBarKind::Solid, |(k, _)| *k);
                solid.set_visible(kind == HubBarKind::Solid);
                gradient.set_visible(kind == HubBarKind::Gradient);
                picture.set_visible(kind == HubBarKind::Image);
            }
        };
        show(kind.selected());
        kind.connect_selected_notify(move |d| show(d.selected()));
        Self { root, kind, color, start, end, angle, image }
    }

    /// The bar the widgets describe now.
    fn read(&self) -> HubBar {
        HubBar {
            kind: KINDS.get(self.kind.selected() as usize).map_or(HubBarKind::Solid, |(k, _)| *k),
            color: self.color.hex(),
            start: self.start.hex(),
            end: self.end.hex(),
            angle: self.angle.value() as u16,
            image: self.image.read(),
        }
        .sanitized()
    }
}

/// Chooser of the start menu icon: path, Choose / Clear buttons, preview.
struct IconPicker {
    root: gtk::Box,
    path: gtk::Entry,
}

impl IconPicker {
    /// Build the chooser seeded from `initial`; `window` parents the file dialog.
    fn new(window: &gtk::ApplicationWindow, initial: Option<&PathBuf>) -> Self {
        let path = gtk::Entry::builder().hexpand(true).editable(false).placeholder_text("built-in icon").build();
        let preview = gtk::Picture::new();
        preview.set_size_request(ICON_PREVIEW, ICON_PREVIEW);
        preview.set_content_fit(gtk::ContentFit::Contain);
        if let Some(p) = initial {
            path.set_text(&p.to_string_lossy());
            preview.set_filename(Some(p));
        }
        let choose = gtk::Button::with_label("Choose...");
        let clear = gtk::Button::with_label("Clear");
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.append(&preview);
        root.append(&path);
        root.append(&choose);
        root.append(&clear);
        {
            let (path, preview, window) = (path.clone(), preview.clone(), window.clone());
            choose.connect_clicked(move |_| {
                let pick = FilePick {
                    title: "Choose Start Menu Icon",
                    filter: Some(FileFilterSpec { name: "Icons", suffixes: &HUB_ICON_EXTS, ..Default::default() }),
                    ..Default::default()
                };
                let (path, preview) = (path.clone(), preview.clone());
                pick_file(Some(window.upcast_ref()), &pick, move |chosen| {
                    path.set_text(&chosen.to_string_lossy());
                    preview.set_filename(Some(&chosen));
                });
            });
        }
        {
            let path = path.clone();
            clear.connect_clicked(move |_| {
                path.set_text("");
                preview.set_filename(None::<&std::path::Path>);
            });
        }
        Self { root, path }
    }

    /// The icon file chosen, if any.
    fn read(&self) -> Option<PathBuf> {
        let text = self.path.text().to_string();
        (!text.is_empty()).then(|| PathBuf::from(text))
    }
}

/// The Hub tab.
pub struct HubTab {
    pub root: gtk::ScrolledWindow,
    bar: BarEditor,
    bar_opt: OptionalSection,
    icon: IconPicker,
    icon_opt: OptionalSection,
    font: FontRow,
    font_opt: OptionalSection,
    initial: HubLook,
}

impl HubTab {
    /// Build the tab seeded from `draft`; `window` parents the dialogs.
    pub fn new(window: &gtk::ApplicationWindow, draft: &ThemeDraft) -> Self {
        let hub = &draft.hub;
        let bar = BarEditor::new(window, &hub.bar.clone().unwrap_or_default());
        let bar_opt = OptionalSection::new("HUB BAR", &bar.root, hub.bar.is_some());
        let icon = IconPicker::new(window, hub.menu_icon.as_ref());
        let icon_opt = OptionalSection::new("START MENU ICON (png, svg)", &icon.root, hub.menu_icon.is_some());
        let grid = gtk::Grid::builder().row_spacing(8).column_spacing(12).build();
        let font = FontRow::new(&grid, 0, "Menu", &hub.menu_font.clone().unwrap_or_default());
        let font_opt = OptionalSection::new("START MENU FONT", &grid, hub.menu_font.is_some());

        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.set_margin_top(12);
        page.set_margin_bottom(12);
        page.set_margin_start(12);
        page.set_margin_end(12);
        for section in [&bar_opt.root, &icon_opt.root, &font_opt.root] {
            page.append(section);
            page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let root = gtk::ScrolledWindow::builder().child(&page).build();

        let mut tab = Self { root, bar, bar_opt, icon, icon_opt, font, font_opt, initial: HubLook::default() };
        tab.initial = tab.read();
        tab
    }

    /// The hub look of the sections that are set.
    fn read(&self) -> HubLook {
        HubLook {
            bar: self.bar_opt.is_set().then(|| self.bar.read()),
            menu_icon: self.icon_opt.is_set().then(|| self.icon.read()).flatten(),
            menu_font: self.font_opt.is_set().then(|| self.font.read()).map(|f: FontSpec| FontSpec {
                size: f.size.min(MAX_SIZE as u32),
                ..f
            }),
        }
    }

    /// Write the tab's values into `draft` when they changed.
    pub fn collect_into(&self, draft: &mut ThemeDraft) {
        let now = self.read();
        if now != self.initial {
            draft.hub = now;
        }
    }
}
