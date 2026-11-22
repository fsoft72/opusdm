//! Amiga ReqTools-style modal requesters: a titled window with the message
//! centered and the buttons along the bottom - the primary action bold on the
//! far left, the cancel action on the far right. The requester always answers:
//! every exit path (button, Esc, or the window manager closing it) delivers
//! exactly one choice to the callback.

use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Wrap width shared by every label in a requester.
const BODY_MAX_WIDTH_CHARS: i32 = 50;

/// Minimum width of a requester button, in pixels.
const BUTTON_MIN_WIDTH: i32 = 90;

/// Most names a list requester shows before it ends with "... and N more".
pub const LIST_MAX_NAMES: usize = 10;

/// Width, in characters, past which a listed name is ellipsized in the middle.
const LIST_MAX_WIDTH_CHARS: i32 = 60;

/// Minimum width, in pixels, of the name box of a list requester: it makes
/// the delete requester about twice as wide as a plain one.
const LIST_MIN_WIDTH: i32 = 440;

/// CSS class of the box holding the names of a list requester.
const LIST_CLASS: &str = "opus-requester-list";

/// The look of the name box: a slightly different background than the
/// requester (a tint of the text colour, so it works on light and dark
/// themes) with some inner padding. The selector beats the "inner boxes are
/// transparent" rule a requester picture adds (`look_css.rs`).
const LIST_CSS: &str = "window.opus-requester box.opus-requester-list { background-color: alpha(currentColor, 0.08); border-radius: 6px; padding: 8px 12px; }";

thread_local! {
    /// Whether [`LIST_CSS`] is installed for the default display yet.
    static LIST_CSS_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// The body of a requester: either plain text or Pango markup.
enum Body<'a> {
    Text(&'a str),
    /// Rich text (bold titles, small print, links, ...), rendered centered.
    Markup(&'a str),
}

/// Show a modal requester over `parent`.
///
/// Each button is a `(label, value)` pair: `buttons[0]` is the primary action
/// (bold, far left, activated by Return), the last entry is the cancel action
/// (far right, also Esc and window close), and anything in between is spread
/// across the middle. `on_choice` receives the value of the picked button, so
/// call sites never deal in button indices.
pub fn requester<T: Clone + 'static>(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    message: &str,
    detail: Option<&str>,
    buttons: &[(&str, T)],
    on_choice: impl Fn(T) + 'static,
) {
    let (labels, values) = _split(buttons);
    build(
        parent.upcast_ref(),
        title,
        Body::Text(message),
        detail,
        None,
        &labels,
        _by_value(values, move |value, _text| on_choice(value)),
    );
}

/// Like [`requester`], but with a text entry as the body. `on_choice` gets the
/// value of the picked button and the entry text; Return in the entry picks
/// the primary button.
pub fn requester_entry<T: Clone + 'static>(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    initial: &str,
    buttons: &[(&str, T)],
    on_choice: impl Fn(T, String) + 'static,
) {
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    let (labels, values) = _split(buttons);
    build(
        parent.upcast_ref(),
        title,
        Body::Text(""),
        None,
        Some(entry),
        &labels,
        _by_value(values, on_choice),
    );
}

/// Like [`requester`], with `names` listed one per line in a monospace font
/// under the message (a deletion shows the files it removes). Past
/// [`LIST_MAX_NAMES`] the list ends with "... and N more".
pub fn requester_list<T: Clone + 'static>(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    message: &str,
    names: &[String],
    buttons: &[(&str, T)],
    on_choice: impl Fn(T) + 'static,
) {
    let (labels, values) = _split(buttons);
    build_with_header(
        parent.upcast_ref(),
        title,
        None,
        Body::Text(message),
        _name_list(names),
        None,
        None,
        &labels,
        _by_value(values, move |value, _text| on_choice(value)),
    );
}

/// The lines a list requester shows for `names`: at most [`LIST_MAX_NAMES`]
/// names, then "... and N more" for the rest.
pub fn list_lines(names: &[String]) -> Vec<String> {
    if names.len() <= LIST_MAX_NAMES { return names.to_vec(); }
    let mut lines: Vec<String> = names[..LIST_MAX_NAMES].to_vec();
    lines.push(format!("... and {} more", names.len() - LIST_MAX_NAMES));
    lines
}

/// The monospace, left aligned block of [`list_lines`] for `names`; `None`
/// when there is nothing to list.
fn _name_list(names: &[String]) -> Option<gtk::Widget> {
    if names.is_empty() { return None; }
    _install_list_css();
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    // The box spans the whole width inside the requester margins.
    list.set_halign(gtk::Align::Fill);
    list.set_size_request(LIST_MIN_WIDTH, -1);
    list.add_css_class(LIST_CLASS);
    for line in list_lines(names) {
        let label = gtk::Label::new(Some(&line));
        label.add_css_class("monospace");
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_max_width_chars(LIST_MAX_WIDTH_CHARS);
        list.append(&label);
    }
    Some(list.upcast())
}

/// Install [`LIST_CSS`] for the default display, once per process. Not every
/// OpusDM process loads the shared app CSS (the hub draws the Desktop
/// requesters), so the list requester brings its own rule.
fn _install_list_css() {
    if LIST_CSS_INSTALLED.with(|done| done.get()) { return; }
    let Some(display) = gtk::gdk::Display::default() else {
        opusdm_ipc::err_log!("ui", "no default display: the requester name list is drawn without its box");
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(LIST_CSS);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    LIST_CSS_INSTALLED.with(|done| done.set(true));
}

/// Like [`requester_entry`], but the entry gets the keyboard focus with the
/// characters `select.0..select.1` selected instead of the whole text.
pub fn requester_entry_selecting<T: Clone + 'static>(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    initial: &str,
    select: (i32, i32),
    buttons: &[(&str, T)],
    on_choice: impl Fn(T, String) + 'static,
) {
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    // Focusing selects everything, so the range is applied once the dialog
    // is on screen and the focus has settled.
    entry.connect_map(move |e| {
        let e = e.clone();
        gtk::glib::idle_add_local_once(move || {
            e.grab_focus();
            e.select_region(select.0, select.1);
        });
    });
    let (labels, values) = _split(buttons);
    build(
        parent.upcast_ref(),
        title,
        Body::Text(""),
        None,
        Some(entry),
        &labels,
        _by_value(values, on_choice),
    );
}

/// Like [`requester`], but the body is Pango markup, allowing rich text
/// (bold titles, small print, links, ...).
pub fn requester_rich<T: Clone + 'static>(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    body_markup: &str,
    buttons: &[(&str, T)],
    on_choice: impl Fn(T) + 'static,
) {
    let (labels, values) = _split(buttons);
    build(
        parent.upcast_ref(),
        title,
        Body::Markup(body_markup),
        None,
        None,
        &labels,
        _by_value(values, move |value, _text| on_choice(value)),
    );
}

/// Like [`requester_rich`], with `header` (a logo, say) above the body.
pub(crate) fn requester_rich_header<T: Clone + 'static>(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    header: Option<gtk::Widget>,
    body_markup: &str,
    buttons: &[(&str, T)],
    on_choice: impl Fn(T) + 'static,
) {
    let (labels, values) = _split(buttons);
    build_with_header(
        parent.upcast_ref(),
        title,
        header,
        Body::Markup(body_markup),
        None,
        None,
        None,
        &labels,
        _by_value(values, move |value, _text| on_choice(value)),
    );
}

/// Split the caller's `(label, value)` pairs into the two vectors `build`
/// and the choice adapter need.
fn _split<'a, T: Clone>(buttons: &[(&'a str, T)]) -> (Vec<&'a str>, Vec<T>) {
    buttons.iter().map(|(label, value)| (*label, value.clone())).unzip()
}

/// Adapt a value-based callback to the index-based signature `build` uses, so
/// that the dialog body stays monomorphic in the caller's value type.
fn _by_value<T: Clone + 'static>(
    values: Vec<T>,
    on_choice: impl Fn(T, String) + 'static,
) -> Rc<dyn Fn(usize, String)> {
    Rc::new(move |idx, text| on_choice(values[idx].clone(), text))
}

/// Build a wrapped label for one of the requester body lines.
fn _body_label(body: &Body) -> gtk::Label {
    let label = gtk::Label::new(None);
    match body {
        Body::Text(text) => label.set_text(text),
        Body::Markup(markup) => {
            label.set_markup(markup);
            label.set_justify(gtk::Justification::Center);
        }
    }
    label.set_wrap(true);
    label.set_max_width_chars(BODY_MAX_WIDTH_CHARS);
    label
}

fn build(
    parent: &gtk::Window,
    title: &str,
    body: Body,
    detail: Option<&str>,
    entry: Option<gtk::Entry>,
    buttons: &[&str],
    on_choice: Rc<dyn Fn(usize, String)>,
) {
    build_with_header(parent, title, None, body, None, detail, entry, buttons, on_choice);
}

/// [`build`] with an optional `header` widget (a logo, say) above the body
/// and an optional `list` widget (the names a deletion removes) under it.
#[allow(clippy::too_many_arguments)]
fn build_with_header(
    parent: &gtk::Window,
    title: &str,
    header: Option<gtk::Widget>,
    body: Body,
    list: Option<gtk::Widget>,
    detail: Option<&str>,
    entry: Option<gtk::Entry>,
    buttons: &[&str],
    on_choice: Rc<dyn Fn(usize, String)>,
) {
    assert!(!buttons.is_empty(), "a requester needs at least one button");

    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .decorated(true)
        .resizable(false)
        .build();
    dialog.add_css_class(crate::REQUESTER_CLASS);

    // The contract is that `on_choice` runs exactly once, whatever closes the
    // requester; `deliver` is the single place that enforces it, so every exit
    // path reports the same entry text.
    let answered = Rc::new(Cell::new(false));
    let deliver: Rc<dyn Fn(usize)> = {
        let answered = answered.clone();
        let entry = entry.clone();
        Rc::new(move |idx| {
            if answered.replace(true) {
                return;
            }
            let text = entry.as_ref().map(|e| e.text().to_string()).unwrap_or_default();
            on_choice(idx, text);
        })
    };

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);

    if let Some(header) = &header {
        vbox.append(header);
    }

    if !matches!(body, Body::Text("")) {
        vbox.append(&_body_label(&body));
    }

    if let Some(list) = &list {
        vbox.append(list);
    }

    if let Some(detail) = detail {
        let label = _body_label(&Body::Text(detail));
        label.add_css_class("dim-label");
        vbox.append(&label);
    }

    if let Some(entry) = &entry {
        entry.set_width_chars(30);
        vbox.append(entry);
    }

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let make_button = |label: &str, idx: usize| {
        // The primary (Return key) action gets bold text.
        let btn = if idx == 0 {
            let label_widget = gtk::Label::new(None);
            label_widget.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(label)));
            let btn = gtk::Button::new();
            btn.set_child(Some(&label_widget));
            btn
        } else {
            gtk::Button::with_label(label)
        };
        btn.set_size_request(BUTTON_MIN_WIDTH, -1);
        let dialog = dialog.clone();
        let deliver = deliver.clone();
        btn.connect_clicked(move |_| {
            deliver(idx);
            // destroy() rather than close(): it drops the widget tree and so
            // breaks the dialog -> button -> closure -> dialog reference cycle.
            dialog.destroy();
        });
        btn
    };

    let widgets: Vec<gtk::Button> = buttons
        .iter()
        .enumerate()
        .map(|(idx, label)| make_button(label, idx))
        .collect();
    let cancel = widgets[widgets.len() - 1].clone();

    dialog.set_default_widget(Some(&widgets[0]));
    btn_box.append(&widgets[0]);
    if widgets.len() == 1 {
        // A lone button sits in the middle, not pinned to the left.
        btn_box.set_halign(gtk::Align::Center);
        btn_box.set_hexpand(true);
    } else {
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        btn_box.append(&spacer);
        for btn in &widgets[1..] {
            btn_box.append(btn);
        }
    }
    vbox.append(&btn_box);

    // Esc clicks the cancel button, so both paths run the very same handler.
    {
        let keys = gtk::EventControllerKey::new();
        let cancel = cancel.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cancel.emit_clicked();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        dialog.add_controller(keys);
    }

    // Safety net: a window manager closing the requester must still answer.
    // Without it the hub would wait forever for, say, a ConflictReply.
    {
        let deliver = deliver.clone();
        let cancel_idx = buttons.len() - 1;
        dialog.connect_close_request(move |_| {
            deliver(cancel_idx);
            gtk::glib::Propagation::Proceed
        });
    }

    dialog.set_child(Some(&vbox));
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(count: usize) -> Vec<String> {
        (1..=count).map(|i| format!("file{i}.txt")).collect()
    }

    #[test]
    fn the_list_css_styles_the_list_class_inside_a_requester() {
        assert!(LIST_CSS.starts_with(&format!("window.{} box.{LIST_CLASS} ", crate::REQUESTER_CLASS)));
    }

    #[test]
    fn a_short_list_is_shown_whole() {
        assert_eq!(list_lines(&names(LIST_MAX_NAMES)), names(LIST_MAX_NAMES));
        assert!(list_lines(&[]).is_empty());
    }

    #[test]
    fn a_long_list_ends_with_the_count_of_the_rest() {
        let lines = list_lines(&names(LIST_MAX_NAMES + 3));
        assert_eq!(lines.len(), LIST_MAX_NAMES + 1);
        assert_eq!(lines[..LIST_MAX_NAMES], names(LIST_MAX_NAMES)[..]);
        assert_eq!(lines[LIST_MAX_NAMES], "... and 3 more");
    }
}
