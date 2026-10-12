//! The shared state of the editor and the form widgets bound to it: each
//! widget writes its value into the profile being edited on every change.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_network::profile::Profile;
use opusdm_network_config::session::EditSession;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Gap between the widgets of a page.
pub const SPACING: i32 = 8;
/// Padding of a page.
pub const MARGIN: i32 = 12;
/// Width of the label column, in characters.
const LABEL_CHARS: i32 = 18;

/// What every widget of the editor shares.
pub struct EditorCtx {
    pub session: RefCell<EditSession>,
    /// The widgets that show a field, to mark the field's errors on them.
    fields: RefCell<Vec<(String, gtk::Widget)>>,
    /// Called after every change (the window updates its buttons).
    on_change: RefCell<Option<Box<dyn Fn()>>>,
    /// The VPN profiles (UUID, name), for "connect with VPN".
    pub vpns: Vec<(String, String)>,
    /// The window, the parent of the dialogs.
    pub window: gtk::Window,
}

impl EditorCtx {
    /// A context for `session`.
    pub fn new(session: EditSession, vpns: Vec<(String, String)>, window: gtk::Window) -> Rc<Self> {
        Rc::new(Self { session: RefCell::new(session), fields: RefCell::new(Vec::new()), on_change: RefCell::new(None), vpns, window })
    }

    /// Run `f` after every change.
    pub fn set_on_change(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    /// Change the profile being edited, then show the errors again.
    pub fn edit(&self, f: impl FnOnce(&mut Profile)) {
        f(&mut self.session.borrow_mut().current);
        self.refresh();
    }

    /// Read the profile being edited.
    pub fn read<T>(&self, f: impl FnOnce(&Profile) -> T) -> T {
        f(&self.session.borrow().current)
    }

    /// Mark `widget` as showing `field`.
    pub fn field(&self, field: &str, widget: &impl IsA<gtk::Widget>) {
        self.fields.borrow_mut().push((field.to_string(), widget.clone().upcast()));
    }

    /// Mark the fields with errors (red, with the reason as tooltip) and tell
    /// the window. Widgets that left the window are forgotten.
    pub fn refresh(&self) {
        let errors = self.session.borrow().errors();
        self.fields.borrow_mut().retain(|(_, w)| w.root().is_some());
        for (field, widget) in self.fields.borrow().iter() {
            let texts: Vec<&str> = errors.iter().filter(|(f, _)| f == field).map(|(_, t)| t.as_str()).collect();
            if texts.is_empty() {
                widget.remove_css_class("error");
                widget.set_tooltip_text(None);
            } else {
                widget.add_css_class("error");
                widget.set_tooltip_text(Some(&texts.join("\n")));
            }
        }
        if let Some(f) = self.on_change.borrow().as_ref() {
            f();
        }
    }
}

/// Two columns: labels and their widgets.
pub struct Form {
    grid: gtk::Grid,
    row: Cell<i32>,
}

impl Form {
    /// An empty form with the page padding.
    pub fn new() -> Self {
        let grid = gtk::Grid::new();
        grid.set_row_spacing(SPACING as u32);
        grid.set_column_spacing(SPACING as u32 * 2);
        grid.set_margin_top(MARGIN);
        grid.set_margin_bottom(MARGIN);
        grid.set_margin_start(MARGIN);
        grid.set_margin_end(MARGIN);
        Self { grid, row: Cell::new(0) }
    }

    /// The form widget.
    pub fn widget(&self) -> &gtk::Grid {
        &self.grid
    }

    /// A labelled row.
    pub fn row(&self, label: &str, widget: &impl IsA<gtk::Widget>) {
        let name = gtk::Label::new(Some(label));
        name.set_xalign(1.0);
        name.set_width_chars(LABEL_CHARS);
        name.set_valign(gtk::Align::Center);
        widget.set_hexpand(true);
        let row = self.row.get();
        self.grid.attach(&name, 0, row, 1, 1);
        self.grid.attach(widget, 1, row, 1, 1);
        self.row.set(row + 1);
    }

    /// A widget across both columns.
    pub fn wide(&self, widget: &impl IsA<gtk::Widget>) {
        let row = self.row.get();
        self.grid.attach(widget, 0, row, 2, 1);
        self.row.set(row + 1);
    }

    /// A widget in the value column, with no label (check buttons).
    pub fn indented(&self, widget: &impl IsA<gtk::Widget>) {
        let row = self.row.get();
        self.grid.attach(widget, 1, row, 1, 1);
        self.row.set(row + 1);
    }
}

/// A text entry for `field`; `set` stores the text in the profile.
pub fn entry(ctx: &Rc<EditorCtx>, field: &str, initial: &str, set: impl Fn(&mut Profile, String) + 'static) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_text(initial);
    ctx.field(field, &e);
    let ctx = ctx.clone();
    e.connect_changed(move |e| {
        let text = e.text().to_string();
        ctx.edit(|p| set(p, text));
    });
    e
}

/// A hidden entry with an eye to show it, for a secret.
pub fn secret_entry(ctx: &Rc<EditorCtx>, field: &str, initial: &str, set: impl Fn(&mut Profile, String) + 'static) -> gtk::PasswordEntry {
    let e = gtk::PasswordEntry::new();
    e.set_show_peek_icon(true);
    e.set_text(initial);
    ctx.field(field, &e);
    let ctx = ctx.clone();
    e.connect_changed(move |e| {
        let text = e.text().to_string();
        ctx.edit(|p| set(p, text));
    });
    e
}

/// A check button.
pub fn check(ctx: &Rc<EditorCtx>, label: &str, initial: bool, set: impl Fn(&mut Profile, bool) + 'static) -> gtk::CheckButton {
    let c = gtk::CheckButton::with_label(label);
    c.set_active(initial);
    let ctx = ctx.clone();
    c.connect_toggled(move |c| {
        let on = c.is_active();
        ctx.edit(|p| set(p, on));
    });
    c
}

/// A spin button for a whole number from `min` to `max`.
pub fn spin(ctx: &Rc<EditorCtx>, field: &str, initial: i64, min: i64, max: i64, set: impl Fn(&mut Profile, i64) + 'static) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min as f64, max as f64, 1.0);
    s.set_value(initial as f64);
    s.set_hexpand(false);
    s.set_halign(gtk::Align::Start);
    ctx.field(field, &s);
    let ctx = ctx.clone();
    s.connect_value_changed(move |s| {
        let v = s.value() as i64;
        ctx.edit(|p| set(p, v));
    });
    s
}

/// A drop-down over `labels`; `set` gets the index chosen.
pub fn choice(ctx: &Rc<EditorCtx>, field: &str, labels: &[&str], selected: usize, set: impl Fn(&mut Profile, usize) + 'static) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(labels);
    d.set_selected(selected as u32);
    d.set_hexpand(false);
    d.set_halign(gtk::Align::Start);
    ctx.field(field, &d);
    let ctx = ctx.clone();
    d.connect_selected_notify(move |d| {
        let i = d.selected() as usize;
        ctx.edit(|p| set(p, i));
    });
    d
}

/// A file name entry with a button that opens a file chooser.
pub fn file_entry(ctx: &Rc<EditorCtx>, field: &str, initial: &str, set: impl Fn(&mut Profile, String) + 'static) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, SPACING / 2);
    let e = entry(ctx, field, initial, set);
    e.set_hexpand(true);
    let browse = gtk::Button::with_label("...");
    browse.set_tooltip_text(Some("Choose a file"));
    row.append(&e);
    row.append(&browse);
    let window = ctx.window.clone();
    browse.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder().title("Choose a file").modal(true).build();
        let current = e.text().to_string();
        if !current.is_empty() {
            dialog.set_initial_file(Some(&gio::File::for_path(&current)));
        }
        let e = e.clone();
        let parent = window.clone();
        dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
            let file = match res {
                Ok(file) => file,
                Err(e) if e.matches(gtk::DialogError::Dismissed) => return,
                Err(e) => return opusdm_ui::requester(&parent, "File chooser", &e.to_string(), None, &[("OK", ())], |_| {}),
            };
            if let Some(path) = file.path() {
                e.set_text(&path.to_string_lossy());
            }
        });
    });
    row
}

/// A list written as `a, b, c`.
pub fn join_list(items: &[String]) -> String {
    items.join(", ")
}

/// The items of a list written as `a, b, c` (spaces also separate).
pub fn split_list(text: &str) -> Vec<String> {
    text.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// A vertical box with the page padding.
pub fn page_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    b.set_margin_top(MARGIN);
    b.set_margin_bottom(MARGIN);
    b.set_margin_start(MARGIN);
    b.set_margin_end(MARGIN);
    b
}

/// A dim, wrapped explanation line.
pub fn note(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_wrap(true);
    l.set_xalign(0.0);
    l.add_css_class("dim-label");
    l
}

/// Remove every child of `b`.
pub fn clear(b: &gtk::Box) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_split_on_commas_and_spaces() {
        assert_eq!(split_list(" 1.1.1.1, 9.9.9.9 ,,8.8.8.8 4.4.4.4"), vec!["1.1.1.1", "9.9.9.9", "8.8.8.8", "4.4.4.4"]);
        assert!(split_list("  ").is_empty());
        assert_eq!(join_list(&["a".into(), "b".into()]), "a, b");
    }
}
