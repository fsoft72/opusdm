//! Amiga ReqTools-style modal requesters: a titled window with the message
//! centered and the buttons along the bottom - the primary action bold on the
//! far left, the cancel action on the far right. The close gadget is disabled
//! (`deletable(false)`); the only way out is a real choice (Esc picks the
//! cancel button, Return picks the primary button).

use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

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
    let on_choice = Rc::new(move |idx, _text: String| on_choice(idx));
    build(parent, title, message, detail, None, buttons, on_choice, None);
}

/// Like [`requester`], but with a text entry under the message. `on_choice`
/// gets the picked button index and the entry text; Return in the entry
/// picks the primary button.
pub fn requester_entry(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    message: &str,
    initial: &str,
    buttons: &[&str],
    on_choice: impl Fn(usize, String) + 'static,
) {
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    build(
        parent,
        title,
        message,
        None,
        Some(entry),
        buttons,
        Rc::new(on_choice),
        None,
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
    let on_choice = Rc::new(move |idx, _text: String| on_choice(idx));
    build_rich(parent, title, body_markup, buttons, on_choice);
}

fn build_rich(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    body_markup: &str,
    buttons: &[&str],
    on_choice: Rc<dyn Fn(usize, String)>,
) {
    let body = gtk::Label::new(None);
    body.set_markup(body_markup);
    body.set_wrap(true);
    body.set_max_width_chars(50);
    body.set_justify(gtk::Justification::Center);
    build(parent, title, "", None, None, buttons, on_choice, Some(body));
}

fn build(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    message: &str,
    detail: Option<&str>,
    entry: Option<gtk::Entry>,
    buttons: &[&str],
    on_choice: Rc<dyn Fn(usize, String)>,
    rich_body: Option<gtk::Label>,
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
    dialog.add_css_class("requester");

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);

    if let Some(body) = rich_body {
        vbox.append(&body);
    }

    if !message.is_empty() {
        let msg = gtk::Label::new(Some(message));
        msg.set_wrap(true);
        msg.set_max_width_chars(50);
        vbox.append(&msg);
    }

    if let Some(detail) = detail {
        let d = gtk::Label::new(Some(detail));
        d.set_wrap(true);
        d.set_max_width_chars(50);
        d.add_css_class("dim-label");
        vbox.append(&d);
    }

    if let Some(entry) = &entry {
        entry.set_width_chars(30);
        vbox.append(entry);
    }

    let entry_text = {
        let entry = entry.clone();
        move || entry.as_ref().map(|e| e.text().to_string()).unwrap_or_default()
    };

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    // A lone button sits in the middle, not pinned to the left.
    let single = buttons.len() == 1;
    if single {
        btn_box.set_halign(gtk::Align::Center);
        btn_box.set_hexpand(true);
    }

    let make_button = |label: &str, idx: usize| {
        // The primary (Return key) action gets bold text.
        let label_widget = gtk::Label::new(None);
        if idx == 0 {
            label_widget.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(label)));
        } else {
            label_widget.set_text(label);
        }
        let btn = gtk::Button::new();
        btn.set_child(Some(&label_widget));
        btn.set_size_request(90, -1);
        let dialog = dialog.clone();
        let on_choice = on_choice.clone();
        let entry_text = entry_text.clone();
        btn.connect_clicked(move |_| {
            on_choice(idx, entry_text());
            dialog.close();
        });
        btn
    };

    let primary = make_button(buttons[0], 0);
    dialog.set_default_widget(Some(&primary));
    btn_box.append(&primary);
    if !single {
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        btn_box.append(&spacer);
    }
    for (i, label) in buttons[1..].iter().enumerate() {
        btn_box.append(&make_button(label, i + 1));
    }
    vbox.append(&btn_box);

    // Esc is the cancel button.
    {
        let keys = gtk::EventControllerKey::new();
        let dlg = dialog.clone();
        let on_choice = on_choice.clone();
        let cancel_idx = buttons.len() - 1;
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                on_choice(cancel_idx, String::new());
                dlg.close();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        dialog.add_controller(keys);
    }

    dialog.set_child(Some(&vbox));
    dialog.present();
}
