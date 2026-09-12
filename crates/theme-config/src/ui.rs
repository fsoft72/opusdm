//! The ICON SET and COLOURS sections. Background editing reuses
//! `opusdm_desktop_config::ui::BackgroundSection` directly from `main.rs`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_desktop_config::ui::ColorRow;
use opusdm_ipc::IconSetId;

/// The ICON SET section.
pub struct IconSetSection {
    pub root: gtk::Box,
    dropdown: gtk::DropDown,
}

impl IconSetSection {
    pub fn new(current: IconSetId) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("ICON SET"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let labels: Vec<&str> = IconSetId::ALL.iter().map(|s| s.label()).collect();
        let dropdown = gtk::DropDown::from_strings(&labels);
        let selected = IconSetId::ALL.iter().position(|s| *s == current).unwrap_or(0);
        dropdown.set_selected(selected as u32);
        root.append(&dropdown);

        Self { root, dropdown }
    }

    /// The currently selected icon set.
    pub fn value(&self) -> IconSetId {
        IconSetId::ALL[self.dropdown.selected() as usize]
    }
}

/// One "label" + bg/fg colour row pair, e.g. SOURCE.
struct ColorSection {
    root: gtk::Box,
    bg: ColorRow,
    fg: ColorRow,
}

fn color_section(window: &gtk::ApplicationWindow, title: &str, bg_hex: &str, fg_hex: &str) -> ColorSection {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    root.append(&heading);
    let bg = ColorRow::new(window, "Background:", bg_hex);
    let fg = ColorRow::new(window, "Text:", fg_hex);
    root.append(&bg.root);
    root.append(&fg.root);
    ColorSection { root, bg, fg }
}

/// All ten colour rows, grouped into their five sections.
pub struct ColorsSection {
    pub root: gtk::Box,
    source: ColorSection,
    dest: ColorSection,
    dir: ColorSection,
    file: ColorSection,
    selected: ColorSection,
}

impl ColorsSection {
    pub fn new(window: &gtk::ApplicationWindow, c: &opusdm_ipc::ColorSettings) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("COLOURS"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let source = color_section(window, "SOURCE", &c.source_bg, &c.source_fg);
        let dest = color_section(window, "DEST", &c.dest_bg, &c.dest_fg);
        let dir = color_section(window, "DIRECTORY", &c.dir_bg, &c.dir_fg);
        let file = color_section(window, "FILE", &c.file_bg, &c.file_fg);
        let selected = color_section(window, "SELECTION", &c.selected_bg, &c.selected_fg);

        for s in [&source, &dest, &dir, &file, &selected] {
            root.append(&s.root);
        }

        Self { root, source, dest, dir, file, selected }
    }

    pub fn read(&self) -> opusdm_ipc::ColorSettings {
        opusdm_ipc::ColorSettings {
            source_bg: self.source.bg.hex(),
            source_fg: self.source.fg.hex(),
            dest_bg: self.dest.bg.hex(),
            dest_fg: self.dest.fg.hex(),
            dir_bg: self.dir.bg.hex(),
            dir_fg: self.dir.fg.hex(),
            file_bg: self.file.bg.hex(),
            file_fg: self.file.fg.hex(),
            selected_bg: self.selected.bg.hex(),
            selected_fg: self.selected.fg.hex(),
        }
    }
}
