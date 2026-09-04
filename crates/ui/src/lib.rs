//! Amiga ReqTools-style modal requesters: a titled window with the message
//! centered and the buttons along the bottom - the primary action bold on the
//! far left, the cancel action on the far right. The close gadget is disabled
//! (`deletable(false)`); the only way out is a real choice (Esc picks the
//! cancel button, Return picks the primary button).

use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

/// Wrap width shared by every label in a requester.
const BODY_MAX_WIDTH_CHARS: i32 = 50;

/// The body of a requester: either plain text or Pango markup.
enum Body<'a> {
    Text(&'a str),
    /// Rich text (bold titles, small print, links, ...), rendered centered.
    Markup(&'a str),
}

/// Show a modal requester over `parent`.
///
/// `buttons[0]` is the primary action (bold, far left, activated by Return),
/// the last entry is the cancel action (far right, also Esc), and anything in
/// between is spread across the middle. `on_choice` is called with the picked
/// index.
pub fn requester(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    message: &str,
    detail: Option<&str>,
    buttons: &[&str],
    on_choice: impl Fn(usize) + 'static,
) {
    build(
        parent.upcast_ref(),
        title,
        Body::Text(message),
        detail,
        None,
        buttons,
        _ignore_text(on_choice),
    );
}

/// Like [`requester`], but with a text entry as the body. `on_choice` gets the
/// picked button index and the entry text; Return in the entry picks the
/// primary button.
pub fn requester_entry(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    initial: &str,
    buttons: &[&str],
    on_choice: impl Fn(usize, String) + 'static,
) {
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    build(
        parent.upcast_ref(),
        title,
        Body::Text(""),
        None,
        Some(entry),
        buttons,
        Rc::new(on_choice),
    );
}

/// Like [`requester`], but the body is Pango markup, allowing rich text
/// (bold titles, small print, links, ...).
pub fn requester_rich(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    body_markup: &str,
    buttons: &[&str],
    on_choice: impl Fn(usize) + 'static,
) {
    build(
        parent.upcast_ref(),
        title,
        Body::Markup(body_markup),
        None,
        None,
        buttons,
        _ignore_text(on_choice),
    );
}

/// Adapt an index-only callback to the internal index + entry text signature.
fn _ignore_text(on_choice: impl Fn(usize) + 'static) -> Rc<dyn Fn(usize, String)> {
    Rc::new(move |idx, _text: String| on_choice(idx))
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
        // Decorated like the Amiga requesters, but not deletable: the WM
        // shows a title bar without a close gadget, so the only way out is
        // a real button choice.
        .decorated(true)
        .deletable(false)
        .resizable(false)
        .build();

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
        btn.set_size_request(90, -1);
        let dialog = dialog.clone();
        let on_choice = on_choice.clone();
        let entry = entry.clone();
        btn.connect_clicked(move |_| {
            let text = entry.as_ref().map(|e| e.text().to_string()).unwrap_or_default();
            on_choice(idx, text);
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
        let cancel = widgets[widgets.len() - 1].clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                cancel.emit_clicked();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        dialog.add_controller(keys);
    }

    dialog.set_child(Some(&vbox));
    dialog.present();
}
