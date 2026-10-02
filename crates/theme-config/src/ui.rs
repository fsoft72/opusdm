//! The ICON SET, LIGHT / DARK and BEHAVIOUR sections of the Settings window,
//! and the PRESET, COLOURS and APPEARANCE sections the Theme Editor's Colours
//! tab uses. Background editing reuses
//! `opusdm_desktop_config::ui::BackgroundSection` directly from `main.rs`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_desktop_config::ui::ColorRow;
use opusdm_ipc::{Appearance, Behavior, ColorSettings, IconSetId};

/// The THEME section: the picker of the installed themes.
pub struct IconSetSection {
    pub root: gtk::Box,
    dropdown: gtk::DropDown,
    /// Parallel to the dropdown's rows: `None` (index 0) then every
    /// installed custom theme, in the order shown.
    options: Vec<IconSetId>,
}

impl IconSetSection {
    pub fn new(current: &IconSetId) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("THEME"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let mut options: Vec<IconSetId> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        for theme in opusdm_ipc::themes::scan_themes() {
            options.push(IconSetId::Custom(theme.slug));
            labels.push(theme.label);
        }
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&label_refs);
        let selected = options.iter().position(|s| s == current).unwrap_or(0);
        dropdown.set_selected(selected as u32);
        root.append(&dropdown);

        Self { root, dropdown, options }
    }

    /// The currently selected icon set.
    pub fn value(&self) -> IconSetId {
        self.options[self.dropdown.selected() as usize].clone()
    }
}

/// One category's bg/fg colour pair, e.g. SOURCE. Each `ColorRow` is bare
/// (no leading name label) - the category name is the grid row's own label.
struct ColorSection {
    bg: ColorRow,
    fg: ColorRow,
}

fn color_section(window: &gtk::ApplicationWindow, bg_hex: &str, fg_hex: &str) -> ColorSection {
    ColorSection { bg: ColorRow::new(window, None, bg_hex), fg: ColorRow::new(window, None, fg_hex) }
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
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("COLOURS"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        outer.append(&heading);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_spacing(12);

        let column_header = |text: &str| {
            let l = gtk::Label::new(Some(text));
            l.set_xalign(0.0);
            l.add_css_class("dim-label");
            l
        };
        grid.attach(&column_header("Background"), 1, 0, 1, 1);
        grid.attach(&column_header("Text"), 2, 0, 1, 1);

        let source = color_section(window, &c.source_bg, &c.source_fg);
        let dest = color_section(window, &c.dest_bg, &c.dest_fg);
        let dir = color_section(window, &c.dir_bg, &c.dir_fg);
        let file = color_section(window, &c.file_bg, &c.file_fg);
        let selected = color_section(window, &c.selected_bg, &c.selected_fg);

        for (row, (title, section)) in [
            ("SOURCE", &source),
            ("DEST", &dest),
            ("DIRECTORY", &dir),
            ("FILE", &file),
            ("SELECTION", &selected),
        ]
        .into_iter()
        .enumerate()
        {
            let row = row as i32 + 1;
            let name = gtk::Label::new(Some(title));
            name.set_xalign(0.0);
            grid.attach(&name, 0, row, 1, 1);
            grid.attach(&section.bg.root, 1, row, 1, 1);
            grid.attach(&section.fg.root, 2, row, 1, 1);
        }

        outer.append(&grid);

        Self { root: outer, source, dest, dir, file, selected }
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

/// The APPEARANCE section: the non-theme look-and-feel toggles. Moved here
/// from the hub's now-removed Settings dialog - icon set, colours and
/// background already lived in this window, so the rest of the Lister's
/// look belongs here too.
pub struct AppearanceSection {
    pub root: gtk::Box,
    type_indicators: gtk::CheckButton,
    monospace_list: gtk::CheckButton,
    theme_window: gtk::CheckButton,
}

impl AppearanceSection {
    pub fn new(initial: &Appearance) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("APPEARANCE"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let type_indicators = gtk::CheckButton::with_label("Type indicators after names (/ * @ = |)");
        let monospace_list = gtk::CheckButton::with_label("Monospace file list");
        let theme_window = gtk::CheckButton::with_label("Apply theme colours to the whole window");
        type_indicators.set_active(initial.type_indicators);
        monospace_list.set_active(initial.monospace_list);
        theme_window.set_active(initial.theme_window);
        for c in [&type_indicators, &monospace_list, &theme_window] {
            root.append(c);
        }

        Self { root, type_indicators, monospace_list, theme_window }
    }

    /// The appearance the checkboxes describe. `theme_mode` is supplied by the
    /// caller: light / dark is a user preference, not edited here.
    pub fn read(&self, theme_mode: opusdm_ipc::ThemeMode) -> Appearance {
        Appearance {
            type_indicators: self.type_indicators.is_active(),
            monospace_list: self.monospace_list.is_active(),
            theme_window: self.theme_window.is_active(),
            theme_mode,
        }
    }
}

/// The dark/light GTK theme preference drop-down, shown in the Desktop tab
/// above the THEME section.
pub struct ThemeModeSection {
    pub root: gtk::Box,
    dropdown: gtk::DropDown,
}

impl ThemeModeSection {
    pub fn new(initial: opusdm_ipc::ThemeMode) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("LIGHT / DARK"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let dropdown = gtk::DropDown::from_strings(&["Light", "Dark"]);
        dropdown.set_selected(Self::index_of(initial));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some("Mode"));
        name.set_xalign(0.0);
        name.set_size_request(90, -1);
        row.append(&name);
        row.append(&dropdown);
        root.append(&row);

        Self { root, dropdown }
    }

    /// Call `f` with the new mode whenever the drop-down changes, so the
    /// settings window itself can preview it live.
    pub fn connect_mode_changed(&self, f: impl Fn(opusdm_ipc::ThemeMode) + 'static) {
        let dropdown = self.dropdown.clone();
        self.dropdown.connect_selected_notify(move |_| {
            let mode = match dropdown.selected() {
                0 => opusdm_ipc::ThemeMode::Light,
                _ => opusdm_ipc::ThemeMode::Dark,
            };
            f(mode);
        });
    }

    fn index_of(mode: opusdm_ipc::ThemeMode) -> u32 {
        match mode {
            opusdm_ipc::ThemeMode::Light => 0,
            opusdm_ipc::ThemeMode::Dark => 1,
        }
    }

    /// The currently selected mode.
    pub fn value(&self) -> opusdm_ipc::ThemeMode {
        match self.dropdown.selected() {
            0 => opusdm_ipc::ThemeMode::Light,
            _ => opusdm_ipc::ThemeMode::Dark,
        }
    }

    #[cfg(test)]
    fn set_mode_for_test(&self, mode: opusdm_ipc::ThemeMode) {
        self.dropdown.set_selected(Self::index_of(mode));
    }
}

/// The BEHAVIOUR section: the auto-refresh and recursive-filter toggles and
/// the status bar format.
/// `auto_refresh_debounce_ms` has no control here - same as the dialog this
/// replaced - so it round-trips untouched through [`Self::read`].
pub struct BehaviorSection {
    pub root: gtk::Box,
    auto_refresh: gtk::CheckButton,
    recursive_filter: gtk::CheckButton,
    status_format: gtk::Entry,
    cache_size: gtk::SpinButton,
    debounce_ms: u64,
}

impl BehaviorSection {
    pub fn new(initial: &Behavior) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some("BEHAVIOUR"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        root.append(&heading);

        let auto_refresh =
            gtk::CheckButton::with_label("Auto-refresh the list when the directory changes (inotify)");
        auto_refresh.set_active(initial.auto_refresh);
        root.append(&auto_refresh);

        let recursive_filter = gtk::CheckButton::with_label(
            "Ask for a file pattern before a recursive Copy, Move or Delete",
        );
        recursive_filter.set_active(initial.recursive_filter);
        root.append(&recursive_filter);

        let format_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        format_row.append(&gtk::Label::new(Some("Status bar format")));
        let status_format = gtk::Entry::new();
        status_format.set_hexpand(true);
        status_format.set_text(&initial.status_format);
        status_format.set_tooltip_text(Some(
            "%d %f %b: total dirs, files, bytes   %D %F %B: selected   %s: selected entries   %p: path   %%: a percent sign",
        ));
        format_row.append(&status_format);
        root.append(&format_row);

        let cache_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        cache_row.append(&gtk::Label::new(Some("Directory cache size (0 = off)")));
        let (lo, hi) = opusdm_ipc::CACHE_SIZE_RANGE;
        let cache_size = gtk::SpinButton::with_range(lo as f64, hi as f64, 1.0);
        cache_size.set_value(initial.cache_size as f64);
        cache_row.append(&cache_size);
        root.append(&cache_row);

        Self { root, auto_refresh, recursive_filter, status_format, cache_size, debounce_ms: initial.auto_refresh_debounce_ms }
    }

    pub fn read(&self) -> Behavior {
        Behavior {
            auto_refresh: self.auto_refresh.is_active(),
            auto_refresh_debounce_ms: self.debounce_ms,
            recursive_filter: self.recursive_filter.is_active(),
            status_format: self.status_format.text().to_string(),
            cache_size: self.cache_size.value_as_int().max(0) as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::ThemeMode;

    #[test]
    fn theme_mode_section_reports_seeded_then_edited_value() {
        if gtk::init().is_err() {
            return;
        }
        let s = ThemeModeSection::new(ThemeMode::Light);
        assert_eq!(s.value(), ThemeMode::Light);
        s.set_mode_for_test(ThemeMode::Dark);
        assert_eq!(s.value(), ThemeMode::Dark);
    }
}
