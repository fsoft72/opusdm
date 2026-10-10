//! The Theme tab of the Preferences dialog: pick a theme, edit its colours
//! (previewed live on the open terminals) and export the result as a new
//! user theme in `~/.config/opusdm/terminal-themes/`.

use crate::theme::{self, parse_hex, to_hex, Rgb, TerminalTheme, PALETTE_SIZE};
use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Gap between the widgets of the tab.
const SPACING: i32 = 8;

/// Names of the 8 ANSI colours, shown as tooltips of the palette buttons.
const ANSI_NAMES: [&str; 8] = ["Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White"];

/// A colour a theme may leave to VTE: a "Default" check box plus a button
/// that is only used while the check box is off.
struct OptionalColour {
    default: gtk::CheckButton,
    button: gtk::ColorDialogButton,
}

impl OptionalColour {
    /// A "Default" check box and a colour button named `name`.
    fn new(name: &str) -> Self {
        let default = gtk::CheckButton::with_label("Default");
        default.set_tooltip_text(Some("Let the terminal choose this colour"));
        Self { default, button: colour_button(name) }
    }

    /// Show `value`, or tick "Default" (keeping `fallback` in the button).
    fn set(&self, value: Option<&str>, fallback: &str) {
        self.default.set_active(value.is_none());
        self.button.set_rgba(&rgba(value.unwrap_or(fallback)));
        self.button.set_sensitive(value.is_some());
    }

    /// The colour, `None` while "Default" is ticked.
    fn get(&self) -> Option<String> {
        (!self.default.is_active()).then(|| hex(&self.button))
    }
}

/// The widgets of the Theme tab and the state of the edit.
pub struct ThemeEditor {
    /// The tab, to put in the notebook.
    pub widget: gtk::Widget,
    selector: gtk::DropDown,
    /// Every theme the selector lists, in its order.
    themes: RefCell<Vec<TerminalTheme>>,
    foreground: gtk::ColorDialogButton,
    background: gtk::ColorDialogButton,
    cursor: OptionalColour,
    cursor_foreground: OptionalColour,
    selection: OptionalColour,
    selection_foreground: OptionalColour,
    palette: Vec<gtk::ColorDialogButton>,
    /// Whether the colours differ from the selected theme.
    dirty: Cell<bool>,
    /// Set while the widgets are filled from a theme, to mute the handlers.
    loading: Cell<bool>,
    /// Called after every change of the selection or of a colour.
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
}

/// A colour button without alpha.
fn colour_button(name: &str) -> gtk::ColorDialogButton {
    let dialog = gtk::ColorDialog::builder().with_alpha(false).title(name).build();
    let button = gtk::ColorDialogButton::new(Some(dialog));
    button.set_tooltip_text(Some(name));
    button
}

/// The GDK colour of `hex` (black when it does not parse).
fn rgba(hex: &str) -> gdk::RGBA {
    let c = parse_hex(hex).unwrap_or(Rgb { red: 0.0, green: 0.0, blue: 0.0 });
    gdk::RGBA::new(c.red, c.green, c.blue, 1.0)
}

/// The `#rrggbb` text of the colour of `button`.
fn hex(button: &gtk::ColorDialogButton) -> String {
    let c = button.rgba();
    to_hex(Rgb { red: c.red(), green: c.green(), blue: c.blue() })
}

/// A label for a grid cell, aligned to the left.
fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l
}

impl ThemeEditor {
    /// Build the tab listing `themes`, showing the one named `selected`.
    pub fn new(themes: Vec<TerminalTheme>, selected: &str) -> Rc<Self> {
        let page = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        page.set_margin_start(SPACING);
        page.set_margin_end(SPACING);
        page.set_margin_top(SPACING);
        page.set_margin_bottom(SPACING);

        let selector = gtk::DropDown::from_strings(&[]);
        selector.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        top.append(&label("Theme"));
        top.append(&selector);
        page.append(&top);

        let grid = gtk::Grid::new();
        grid.set_column_spacing(SPACING as u32);
        grid.set_row_spacing(SPACING as u32);
        grid.set_margin_top(SPACING);
        let foreground = colour_button("Text");
        let background = colour_button("Background");
        grid.attach(&label("Text"), 0, 0, 1, 1);
        grid.attach(&foreground, 1, 0, 1, 1);
        grid.attach(&label("Background"), 0, 1, 1, 1);
        grid.attach(&background, 1, 1, 1, 1);
        let cursor = OptionalColour::new("Cursor");
        let cursor_foreground = OptionalColour::new("Text under the cursor");
        let selection = OptionalColour::new("Selection");
        let selection_foreground = OptionalColour::new("Selected text");
        let optionals = [
            ("Cursor", &cursor),
            ("Text under the cursor", &cursor_foreground),
            ("Selection", &selection),
            ("Selected text", &selection_foreground),
        ];
        for (i, (name, colour)) in optionals.into_iter().enumerate() {
            let row = i as i32 + 2;
            grid.attach(&label(name), 0, row, 1, 1);
            grid.attach(&colour.button, 1, row, 1, 1);
            grid.attach(&colour.default, 2, row, 1, 1);
        }
        page.append(&grid);

        let palette_label = label("Palette: normal colours, then the bright ones");
        palette_label.set_margin_top(SPACING);
        page.append(&palette_label);
        let palette_grid = gtk::Grid::new();
        palette_grid.set_column_spacing(4);
        palette_grid.set_row_spacing(4);
        let palette: Vec<gtk::ColorDialogButton> = (0..PALETTE_SIZE)
            .map(|i| {
                let bright = if i >= 8 { "Bright " } else { "" };
                let button = colour_button(&format!("{bright}{}", ANSI_NAMES[i % 8]));
                palette_grid.attach(&button, (i % 8) as i32, (i / 8) as i32, 1, 1);
                button
            })
            .collect();
        page.append(&palette_grid);

        let editor = Rc::new(Self {
            widget: page.upcast(),
            selector,
            themes: RefCell::new(Vec::new()),
            foreground,
            background,
            cursor,
            cursor_foreground,
            selection,
            selection_foreground,
            palette,
            dirty: Cell::new(false),
            loading: Cell::new(false),
            on_change: RefCell::new(None),
        });
        editor.set_themes(themes, selected);
        editor.connect_widgets();
        editor
    }

    /// The four colours a theme may leave to VTE.
    fn optionals(&self) -> [&OptionalColour; 4] {
        [&self.cursor, &self.cursor_foreground, &self.selection, &self.selection_foreground]
    }

    /// Call `f` after every change of the selection or of a colour.
    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Rc::new(f));
    }

    /// Run the change callback, if any.
    fn notify(&self) {
        let callback = self.on_change.borrow().clone();
        if let Some(f) = callback { f(); }
    }

    /// Wire the selector (load the picked theme) and every colour widget
    /// (mark the edit dirty).
    fn connect_widgets(self: &Rc<Self>) {
        {
            let weak = Rc::downgrade(self);
            self.selector.connect_selected_notify(move |_| {
                let Some(editor) = weak.upgrade() else { return };
                if editor.loading.get() { return; }
                editor.load_selected();
                editor.notify();
            });
        }
        let edited = {
            let weak = Rc::downgrade(self);
            Rc::new(move || {
                let Some(editor) = weak.upgrade() else { return };
                if editor.loading.get() { return; }
                for colour in editor.optionals() {
                    colour.button.set_sensitive(!colour.default.is_active());
                }
                editor.dirty.set(true);
                editor.notify();
            })
        };
        for button in [&self.foreground, &self.background].into_iter().chain(self.palette.iter()) {
            let e = edited.clone();
            button.connect_rgba_notify(move |_| e());
        }
        for colour in self.optionals() {
            let e = edited.clone();
            colour.button.connect_rgba_notify(move |_| e());
            let e = edited.clone();
            colour.default.connect_toggled(move |_| e());
        }
    }

    /// List `themes` in the selector and show the one named `selected`.
    pub fn set_themes(&self, themes: Vec<TerminalTheme>, selected: &str) {
        self.loading.set(true);
        let names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
        self.selector.set_model(Some(&gtk::StringList::new(&names)));
        let index = names.iter().position(|n| *n == selected).unwrap_or(0);
        self.selector.set_selected(index as u32);
        *self.themes.borrow_mut() = themes;
        self.loading.set(false);
        self.load_selected();
    }

    /// Fill the colour widgets from the selected theme; the edit is clean.
    fn load_selected(&self) {
        let theme = self.themes.borrow().get(self.selector.selected() as usize).cloned();
        let Some(theme) = theme else { return };
        self.loading.set(true);
        self.foreground.set_rgba(&rgba(&theme.foreground));
        self.background.set_rgba(&rgba(&theme.background));
        self.cursor.set(theme.cursor.as_deref(), &theme.foreground);
        self.cursor_foreground.set(theme.cursor_foreground.as_deref(), &theme.background);
        self.selection.set(theme.selection.as_deref(), &theme.palette[8]);
        self.selection_foreground.set(theme.selection_foreground.as_deref(), &theme.foreground);
        for (button, colour) in self.palette.iter().zip(&theme.palette) {
            button.set_rgba(&rgba(colour));
        }
        self.loading.set(false);
        self.dirty.set(false);
    }

    /// Put the colours of the selected theme back, dropping the edits.
    pub fn reset(&self) {
        self.load_selected();
        self.notify();
    }

    /// Name of the selected theme.
    pub fn selected_name(&self) -> String {
        let themes = self.themes.borrow();
        themes.get(self.selector.selected() as usize).map(|t| t.name.clone()).unwrap_or_default()
    }

    /// Whether the colours were edited since a theme was picked or exported.
    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    /// The theme the widgets show, called `name`.
    pub fn edited_theme(&self, name: &str) -> TerminalTheme {
        TerminalTheme {
            name: name.to_string(),
            foreground: hex(&self.foreground),
            background: hex(&self.background),
            cursor: self.cursor.get(),
            cursor_foreground: self.cursor_foreground.get(),
            selection: self.selection.get(),
            selection_foreground: self.selection_foreground.get(),
            palette: self.palette.iter().map(hex).collect(),
        }
    }

    /// Write the edited colours as the user theme `name`, then list it and
    /// select it. Returns the file written.
    pub fn export(&self, name: &str) -> Result<std::path::PathBuf, String> {
        let name = name.trim();
        if name.is_empty() { return Err("The theme needs a name.".into()); }
        let path = theme::save_user_theme(&theme::user_themes_dir(), &self.edited_theme(name))?;
        self.set_themes(theme::all_themes(), name);
        Ok(path)
    }
}
