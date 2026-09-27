//! The three sections of the desktop-config window. Each is a builder that
//! owns its widgets and can read its values back into a settings struct.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::{Background, BackgroundMode, ImageFit, PanelPosition};
use std::path::{Path, PathBuf};

use crate::model::{hex_to_rgba, rgba_to_hex};

/// True when `p` points at a file that exists and can be used as a background.
pub fn image_path_is_usable(p: Option<&Path>) -> bool {
    p.map(|p| p.is_file()).unwrap_or(false)
}

/// A section heading label styled like the hub's settings dialog.
fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("heading");
    l
}

/// A "hex entry + colour swatch" control. Clicking the swatch opens a GTK
/// colour picker; the entry text is the source of truth (`#rrggbb`).
pub struct ColorRow {
    pub root: gtk::Box,
    entry: gtk::Entry,
}

impl ColorRow {
    /// Build the control seeded with `initial_hex`, using `window` as the
    /// colour dialog's parent. `label` is omitted (no leading name) when
    /// `None` - used where the caller supplies the row's identity some
    /// other way, e.g. a grid row label.
    pub fn new(window: &gtk::ApplicationWindow, label: Option<&str>, initial_hex: &str) -> Self {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = label.map(|text| {
            let name = gtk::Label::new(Some(text));
            name.set_xalign(0.0);
            name.set_size_request(60, -1);
            name
        });

        let entry = gtk::Entry::new();
        entry.set_text(initial_hex);
        entry.set_max_width_chars(9);

        let swatch = gtk::DrawingArea::new();
        swatch.set_content_width(40);
        swatch.set_content_height(24);
        {
            let entry = entry.clone();
            swatch.set_draw_func(move |_da, cr, w, h| {
                let c = hex_to_rgba(&entry.text());
                cr.set_source_rgb(c.red() as f64, c.green() as f64, c.blue() as f64);
                cr.rectangle(0.0, 0.0, w as f64, h as f64);
                cr.fill().ok();
            });
        }
        {
            let swatch = swatch.clone();
            entry.connect_changed(move |_| swatch.queue_draw());
        }
        {
            let entry = entry.clone();
            let window = window.clone();
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                let dialog = gtk::ColorDialog::new();
                let start = hex_to_rgba(&entry.text());
                let entry = entry.clone();
                dialog.choose_rgba(
                    Some(&window),
                    Some(&start),
                    gio::Cancellable::NONE,
                    move |res| {
                        if let Ok(c) = res {
                            entry.set_text(&rgba_to_hex(&c));
                        }
                    },
                );
            });
            swatch.add_controller(gesture);
        }

        if let Some(name) = &name {
            row.append(name);
        }
        row.append(&entry);
        row.append(&swatch);
        Self { root: row, entry }
    }

    /// The chosen colour as `#rrggbb`.
    pub fn hex(&self) -> String {
        rgba_to_hex(&hex_to_rgba(&self.entry.text()))
    }

    /// Overwrite the entry, which redraws the swatch too.
    pub fn set_hex(&self, hex: &str) {
        self.entry.set_text(hex);
    }
}

/// The BACKGROUND section: mode radios + a stack of per-mode controls.
pub struct BackgroundSection {
    pub root: gtk::Box,
    radio_solid: gtk::CheckButton,
    radio_gradient: gtk::CheckButton,
    radio_image: gtk::CheckButton,
    solid: ColorRow,
    grad_start: ColorRow,
    grad_end: ColorRow,
    grad_angle: gtk::SpinButton,
    image_entry: gtk::Entry,
    image_fit: gtk::DropDown,
    image_warning: gtk::Label,
}

impl BackgroundSection {
    /// Build the section, seeded from `initial`. `window` is the parent for
    /// the colour and file dialogs.
    pub fn new(window: &gtk::ApplicationWindow, initial: &Background) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("BACKGROUND"));

        let radio_solid = gtk::CheckButton::with_label("Solid colour");
        let radio_gradient = gtk::CheckButton::with_label("Gradient");
        let radio_image = gtk::CheckButton::with_label("Image");
        radio_gradient.set_group(Some(&radio_solid));
        radio_image.set_group(Some(&radio_solid));
        root.append(&radio_solid);
        root.append(&radio_gradient);
        root.append(&radio_image);

        let stack = gtk::Stack::new();
        stack.set_margin_top(6);

        // solid page
        let solid = ColorRow::new(window, Some("Colour"), &initial.solid);
        stack.add_named(&solid.root, Some("solid"));

        // gradient page
        let grad_start = ColorRow::new(window, Some("Start"), &initial.gradient_start);
        let grad_end = ColorRow::new(window, Some("End"), &initial.gradient_end);
        let grad_angle = gtk::SpinButton::with_range(0.0, 360.0, 1.0);
        grad_angle.set_value(initial.gradient_angle as f64);
        let grad_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        grad_page.append(&grad_start.root);
        grad_page.append(&grad_end.root);
        let angle_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let angle_label = gtk::Label::new(Some("Angle"));
        angle_label.set_xalign(0.0);
        angle_label.set_size_request(60, -1);
        angle_row.append(&angle_label);
        angle_row.append(&grad_angle);
        grad_page.append(&angle_row);
        stack.add_named(&grad_page, Some("gradient"));

        // image page
        let image_entry = gtk::Entry::new();
        image_entry.set_editable(false);
        image_entry.set_hexpand(true);
        if let Some(p) = initial.image_path.as_ref() {
            image_entry.set_text(&p.to_string_lossy());
        }
        let browse = gtk::Button::with_label("Browse...");
        let image_fit = gtk::DropDown::from_strings(&["Fill", "Fit", "Center", "Tile"]);
        image_fit.set_selected(match initial.image_fit {
            ImageFit::Fill => 0,
            ImageFit::Fit => 1,
            ImageFit::Center => 2,
            ImageFit::Tile => 3,
        });
        let image_warning = gtk::Label::new(Some("Image file not found"));
        image_warning.add_css_class("warning");
        image_warning.set_xalign(0.0);
        image_warning.set_visible(false);

        let image_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let pick_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        pick_row.append(&image_entry);
        pick_row.append(&browse);
        image_page.append(&pick_row);
        let fit_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        fit_row.append(&gtk::Label::new(Some("Fit")));
        fit_row.append(&image_fit);
        image_page.append(&fit_row);
        image_page.append(&image_warning);
        stack.add_named(&image_page, Some("image"));

        root.append(&stack);

        // radios drive the stack
        {
            let stack = stack.clone();
            radio_solid.connect_toggled(move |b| {
                if b.is_active() {
                    stack.set_visible_child_name("solid");
                }
            });
        }
        {
            let stack = stack.clone();
            radio_gradient.connect_toggled(move |b| {
                if b.is_active() {
                    stack.set_visible_child_name("gradient");
                }
            });
        }
        {
            let stack = stack.clone();
            radio_image.connect_toggled(move |b| {
                if b.is_active() {
                    stack.set_visible_child_name("image");
                }
            });
        }

        // browse dialog
        {
            let entry = image_entry.clone();
            let warning = image_warning.clone();
            let window = window.clone();
            browse.connect_clicked(move |_| {
                let dialog = gtk::FileDialog::builder()
                    .title("Choose a background image")
                    .build();
                let filter = gtk::FileFilter::new();
                filter.add_pixbuf_formats();
                filter.set_name(Some("Images"));
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                dialog.set_filters(Some(&filters));
                let entry = entry.clone();
                let warning = warning.clone();
                dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            entry.set_text(&path.to_string_lossy());
                            warning.set_visible(!image_path_is_usable(Some(&path)));
                        }
                    }
                });
            });
        }

        let section = Self {
            root,
            radio_solid,
            radio_gradient,
            radio_image,
            solid,
            grad_start,
            grad_end,
            grad_angle,
            image_entry,
            image_fit,
            image_warning,
        };
        section.select_mode(initial.mode);
        section
    }

    fn select_mode(&self, mode: BackgroundMode) {
        match mode {
            BackgroundMode::Solid => self.radio_solid.set_active(true),
            BackgroundMode::Gradient => self.radio_gradient.set_active(true),
            BackgroundMode::Image => self.radio_image.set_active(true),
        }
    }

    /// Show or hide the "image not found" warning.
    pub fn set_image_warning(&self, visible: bool) {
        self.image_warning.set_visible(visible);
    }

    /// Read every control back into a `Background`.
    pub fn read(&self) -> Background {
        let mode = if self.radio_image.is_active() {
            BackgroundMode::Image
        } else if self.radio_gradient.is_active() {
            BackgroundMode::Gradient
        } else {
            BackgroundMode::Solid
        };
        let text = self.image_entry.text().to_string();
        let image_path = (!text.is_empty()).then(|| PathBuf::from(text));
        let image_fit = match self.image_fit.selected() {
            1 => ImageFit::Fit,
            2 => ImageFit::Center,
            3 => ImageFit::Tile,
            _ => ImageFit::Fill,
        };
        Background {
            mode,
            solid: self.solid.hex(),
            gradient_start: self.grad_start.hex(),
            gradient_end: self.grad_end.hex(),
            gradient_angle: self.grad_angle.value() as u16,
            image_path,
            image_fit,
        }
    }
}

const ICON_PRESETS: [(&str, i32); 3] = [("S", 48), ("M", 64), ("L", 96)];
const SPACING_PRESETS: [(&str, i32); 3] = [("S", 56), ("M", 96), ("L", 140)];
const GAP_PRESETS: [(&str, i32); 3] = [("0", 0), ("4", 4), ("12", 12)];

/// The ICON GRID section: three spin buttons (icon size, grid spacing, grid
/// gap), each with preset buttons that fill the spin value. The pitch of the
/// desktop grid is `grid spacing + grid gap`, independent of the label widths.
pub struct GridSection {
    pub root: gtk::Box,
    icon_spin: gtk::SpinButton,
    spacing_spin: gtk::SpinButton,
    gap_spin: gtk::SpinButton,
}

impl GridSection {
    /// Build the section seeded from the current values.
    pub fn new(icon_size: i32, grid_spacing: i32, grid_gap: i32) -> Self {
        let (imin, imax) = opusdm_ipc::ICON_SIZE_RANGE;
        let (smin, smax) = opusdm_ipc::GRID_SPACING_RANGE;
        let (gmin, gmax) = opusdm_ipc::GRID_GAP_RANGE;

        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("ICON GRID"));

        let icon_spin = gtk::SpinButton::with_range(imin as f64, imax as f64, 1.0);
        icon_spin.set_value(icon_size.clamp(imin, imax) as f64);
        root.append(&preset_row("Icon size", &icon_spin, &ICON_PRESETS));

        let spacing_spin = gtk::SpinButton::with_range(smin as f64, smax as f64, 1.0);
        spacing_spin.set_value(grid_spacing.clamp(smin, smax) as f64);
        root.append(&preset_row("Grid spacing", &spacing_spin, &SPACING_PRESETS));

        let gap_spin = gtk::SpinButton::with_range(gmin as f64, gmax as f64, 1.0);
        gap_spin.set_value(grid_gap.clamp(gmin, gmax) as f64);
        root.append(&preset_row("Grid gap", &gap_spin, &GAP_PRESETS));

        Self { root, icon_spin, spacing_spin, gap_spin }
    }

    /// Current icon size in pixels.
    pub fn icon_size(&self) -> i32 {
        self.icon_spin.value() as i32
    }

    /// Current grid spacing (cell size) in pixels.
    pub fn grid_spacing(&self) -> i32 {
        self.spacing_spin.value() as i32
    }

    /// Current grid gap (space between cells) in pixels.
    pub fn grid_gap(&self) -> i32 {
        self.gap_spin.value() as i32
    }

    #[cfg(test)]
    fn set_icon_size_for_test(&self, v: i32) {
        self.icon_spin.set_value(v as f64);
    }
}

/// A row: a label, a spin button, and preset buttons that set the spin value.
fn preset_row(
    label: &str,
    spin: &gtk::SpinButton,
    presets: &'static [(&'static str, i32)],
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_size_request(90, -1);
    row.append(&name);
    row.append(spin);
    for (text, value) in presets {
        let b = gtk::Button::with_label(text);
        b.add_css_class("flat");
        let spin = spin.clone();
        let value = *value;
        b.connect_clicked(move |_| spin.set_value(value as f64));
        row.append(&b);
    }
    row
}

/// The DESKTOP FOLDER section: an override path with Browse and Reset.
/// An empty entry means "no override - use the XDG/~ resolution".
pub struct FolderSection {
    pub root: gtk::Box,
    entry: gtk::Entry,
}

impl FolderSection {
    /// Build the section. `override_dir` seeds the entry; `None` leaves it
    /// empty (the placeholder shows the default behaviour).
    pub fn new(override_dir: Option<&Path>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("DESKTOP FOLDER"));

        let entry = gtk::Entry::new();
        entry.set_editable(false);
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Default ($XDG_DESKTOP_DIR, then ~/Desktop)"));
        if let Some(p) = override_dir {
            entry.set_text(&p.to_string_lossy());
        }

        let browse = gtk::Button::with_label("Browse...");
        let reset = gtk::Button::with_label("Reset to default");
        {
            let entry = entry.clone();
            browse.connect_clicked(move |btn| {
                let dialog = gtk::FileDialog::builder()
                    .title("Choose the desktop folder")
                    .build();
                let entry = entry.clone();
                let win = btn.root().and_downcast::<gtk::Window>();
                dialog.select_folder(win.as_ref(), gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            entry.set_text(&path.to_string_lossy());
                        }
                    }
                });
            });
        }
        {
            let entry = entry.clone();
            reset.connect_clicked(move |_| entry.set_text(""));
        }

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&entry);
        row.append(&browse);
        row.append(&reset);
        root.append(&row);
        Self { root, entry }
    }

    /// The chosen override, or `None` when the entry is empty.
    pub fn override_dir(&self) -> Option<PathBuf> {
        let t = self.entry.text().to_string();
        (!t.is_empty()).then(|| PathBuf::from(t))
    }
}

/// The PANEL section: a drop-down choosing which screen edge the panel is
/// docked to.
pub struct PanelSection {
    pub root: gtk::Box,
    position: gtk::DropDown,
}

impl PanelSection {
    /// Build the section seeded with the current panel position.
    pub fn new(initial: PanelPosition) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("PANEL"));

        let position = gtk::DropDown::from_strings(&["Top", "Bottom"]);
        position.set_selected(match initial {
            PanelPosition::Top => 0,
            PanelPosition::Bottom => 1,
        });

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some("Position"));
        name.set_xalign(0.0);
        name.set_size_request(90, -1);
        row.append(&name);
        row.append(&position);
        root.append(&row);
        Self { root, position }
    }

    /// Call `f` with the new panel position whenever the drop-down changes,
    /// so a sibling section (the Dock tab) can react live.
    pub fn connect_position_changed(&self, f: impl Fn(PanelPosition) + 'static) {
        let position_dropdown = self.position.clone();
        self.position.connect_selected_notify(move |_| {
            f(Self::position_of(position_dropdown.selected()));
        });
    }

    /// Decode the drop-down's selected index into a position.
    fn position_of(selected: u32) -> PanelPosition {
        match selected {
            1 => PanelPosition::Bottom,
            _ => PanelPosition::Top,
        }
    }

    /// The chosen panel position.
    pub fn position(&self) -> PanelPosition {
        match self.position.selected() {
            1 => PanelPosition::Bottom,
            _ => PanelPosition::Top,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_path_usable_only_for_an_existing_file() {
        assert!(!image_path_is_usable(None));
        assert!(!image_path_is_usable(Some(Path::new("/no/such/file.png"))));
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(image_path_is_usable(Some(f.path())));
    }

    #[test]
    fn grid_section_reports_seeded_then_edited_values() {
        if gtk::init().is_err() {
            return;
        }
        let s = GridSection::new(64, 96, 4);
        assert_eq!(s.icon_size(), 64);
        assert_eq!(s.grid_spacing(), 96);
        assert_eq!(s.grid_gap(), 4);
        s.set_icon_size_for_test(120);
        assert_eq!(s.icon_size(), 120);
    }

    #[test]
    fn folder_section_reports_none_when_empty_and_some_when_set() {
        if gtk::init().is_err() {
            return;
        }
        let s = FolderSection::new(None);
        assert_eq!(s.override_dir(), None);
        let s = FolderSection::new(Some(Path::new("/home/u/Desk")));
        assert_eq!(s.override_dir(), Some(PathBuf::from("/home/u/Desk")));
    }
}
