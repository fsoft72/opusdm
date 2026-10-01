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
    assert!(!buttons.is_empty(), "a requester needs at least one button");

    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .decorated(true)
        .resizable(false)
        .build();

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

    if !matches!(body, Body::Text("")) {
        vbox.append(&_body_label(&body));
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
