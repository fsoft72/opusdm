//! The ICON SET and COLOURS sections. Background editing reuses
//! `opusdm_desktop_config::ui::BackgroundSection` directly from `main.rs`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_desktop_config::ui::ColorRow;
use opusdm_ipc::{ColorSettings, IconSetId};

/// The ICON SET section.
pub struct IconSetSection {
    pub root: gtk::Box,
    dropdown: gtk::DropDown,
}

impl IconSetSection {
    pub fn new(current: &IconSetId) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("ICON SET"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let labels: Vec<String> = IconSetId::ALL.iter().map(|s| s.label()).collect();
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&label_refs);
        let selected = IconSetId::ALL.iter().position(|s| s == current).unwrap_or(0);
        dropdown.set_selected(selected as u32);
        root.append(&dropdown);

        Self { root, dropdown }
    }

    /// The currently selected icon set.
    pub fn value(&self) -> IconSetId {
        IconSetId::ALL[self.dropdown.selected() as usize].clone()
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

/// A named colour combination the preset dropdown can fill the ten entries
/// with in one click. Purely a fill-in convenience: nothing about the choice
/// is persisted, only the resulting `ColorSettings` values are.
fn presets() -> Vec<(&'static str, ColorSettings)> {
    vec![
        ("Default (light)", ColorSettings::default()),
        (
            "Amiga Workbench 1.3",
            ColorSettings {
                source_bg: "#FF8800".into(),
                source_fg: "#000000".into(),
                dest_bg: "#FFFFFF".into(),
                dest_fg: "#0055AA".into(),
                dir_bg: "#0055AA".into(),
                dir_fg: "#FFFFFF".into(),
                file_bg: "#0055AA".into(),
                file_fg: "#FF8800".into(),
                selected_bg: "#FF8800".into(),
                selected_fg: "#000000".into(),
            },
        ),
        (
            "Directory Opus 4/5 Classic",
            ColorSettings {
                source_bg: "#00A0C0".into(),
                source_fg: "#000000".into(),
                dest_bg: "#D8D8D8".into(),
                dest_fg: "#000080".into(),
                dir_bg: "#B4B4B4".into(),
                dir_fg: "#000080".into(),
                file_bg: "#B4B4B4".into(),
                file_fg: "#000000".into(),
                selected_bg: "#000080".into(),
                selected_fg: "#FFFFFF".into(),
            },
        ),
        (
            "Cyberpunk / Tokyo Night",
            ColorSettings {
                source_bg: "#7DCFFF".into(),
                source_fg: "#1A1B26".into(),
                dest_bg: "#F7768E".into(),
                dest_fg: "#1A1B26".into(),
                dir_bg: "#1A1B26".into(),
                dir_fg: "#7DCFFF".into(),
                file_bg: "#1A1B26".into(),
                file_fg: "#C0CAF5".into(),
                selected_bg: "#BB9AF7".into(),
                selected_fg: "#1A1B26".into(),
            },
        ),
        (
            "Monochrome Phosphor (green)",
            ColorSettings {
                source_bg: "#33FF33".into(),
                source_fg: "#0A1A0A".into(),
                dest_bg: "#1F6B1F".into(),
                dest_fg: "#CCFFCC".into(),
                dir_bg: "#0A1A0A".into(),
                dir_fg: "#7CFF7C".into(),
                file_bg: "#0A1A0A".into(),
                file_fg: "#33FF33".into(),
                selected_bg: "#33FF33".into(),
                selected_fg: "#0A1A0A".into(),
            },
        ),
        (
            "Monochrome Phosphor (amber)",
            ColorSettings {
                source_bg: "#FFB000".into(),
                source_fg: "#1A1000".into(),
                dest_bg: "#7A5400".into(),
                dest_fg: "#FFE0A0".into(),
                dir_bg: "#1A1000".into(),
                dir_fg: "#FFD070".into(),
                file_bg: "#1A1000".into(),
                file_fg: "#FFB000".into(),
                selected_bg: "#FFB000".into(),
                selected_fg: "#1A1000".into(),
            },
        ),
    ]
}

/// All ten colour rows, grouped into their five sections, plus the preset
/// dropdown that fills them in one click.
pub struct ColorsSection {
    pub root: gtk::Box,
    source: ColorSection,
    dest: ColorSection,
    dir: ColorSection,
    file: ColorSection,
    selected: ColorSection,
}

impl ColorsSection {
    pub fn new(window: &gtk::ApplicationWindow, c: &ColorSettings) -> Self {
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

    pub fn read(&self) -> ColorSettings {
        ColorSettings {
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

    /// Overwrite every entry, which redraws the swatches too.
    pub fn write(&self, c: &ColorSettings) {
        self.source.bg.set_hex(&c.source_bg);
        self.source.fg.set_hex(&c.source_fg);
        self.dest.bg.set_hex(&c.dest_bg);
        self.dest.fg.set_hex(&c.dest_fg);
        self.dir.bg.set_hex(&c.dir_bg);
        self.dir.fg.set_hex(&c.dir_fg);
        self.file.bg.set_hex(&c.file_bg);
        self.file.fg.set_hex(&c.file_fg);
        self.selected.bg.set_hex(&c.selected_bg);
        self.selected.fg.set_hex(&c.selected_fg);
    }
}

/// Id stored nowhere: purely so the dropdown can show which preset (if any)
/// the current colours still match. Index 0 is always "Custom".
pub struct PresetSection {
    pub root: gtk::Box,
    dropdown: gtk::DropDown,
    values: Vec<ColorSettings>,
}

impl PresetSection {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some("Preset:"));
        label.set_xalign(0.0);
        label.set_size_request(100, -1);

        let table = presets();
        let mut labels = vec!["Custom".to_string()];
        labels.extend(table.iter().map(|(name, _)| name.to_string()));
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&label_refs);
        dropdown.set_selected(0);
        dropdown.set_hexpand(true);

        root.append(&label);
        root.append(&dropdown);

        let values = table.into_iter().map(|(_, c)| c).collect();
        Self { root, dropdown, values }
    }

    /// Selecting a preset fills every colour entry; index 0 ("Custom") does
    /// nothing on its own.
    pub fn connect_selected(&self, colors: std::rc::Rc<ColorsSection>) {
        let values = self.values.clone();
        self.dropdown.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let Some(c) = idx.checked_sub(1).and_then(|i| values.get(i)) else {
                return;
            };
            colors.write(c);
        });
    }
}
