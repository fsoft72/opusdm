//! Small dialogs: the kind of a new connection and the password of a VPN.

use crate::form::{note, SPACING};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_network::profile::ProfileKind;

/// The kinds a new connection can be, with a line about each.
const NEW_KINDS: [(ProfileKind, &str); 4] = [
    (ProfileKind::Ethernet, "A wired connection"),
    (ProfileKind::Wifi, "A wireless network, also a hidden one"),
    (ProfileKind::WireGuard, "A WireGuard VPN tunnel"),
    (ProfileKind::OpenVpn, "An OpenVPN VPN"),
];

/// Ask which kind of connection to create; `on_choice` gets it.
pub fn choose_kind(parent: &gtk::Window, on_choice: impl Fn(ProfileKind) + 'static) {
    let shell = opusdm_ui::DialogShell::new(parent, "New connection");
    let on_choice = std::rc::Rc::new(on_choice);
    shell.content().append(&note("Choose the kind of connection. A VPN can also be imported from a file with Import."));
    let window = shell.window().clone();
    for (kind, text) in NEW_KINDS {
        let button = gtk::Button::new();
        let line = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(Some(kind.label()));
        title.set_xalign(0.0);
        title.add_css_class("heading");
        let detail = gtk::Label::new(Some(text));
        detail.set_xalign(0.0);
        detail.add_css_class("dim-label");
        line.append(&title);
        line.append(&detail);
        button.set_child(Some(&line));
        let (window, on_choice) = (window.clone(), on_choice.clone());
        button.connect_clicked(move |_| {
            window.close();
            on_choice(kind);
        });
        shell.content().append(&button);
    }
    shell.close_button("Cancel");
    shell.present();
}

/// Ask for the password of the VPN `name`; `on_submit` gets it. The entry is
/// emptied as soon as the password is handed over.
pub fn ask_password(parent: &gtk::Window, name: &str, on_submit: impl Fn(String) + 'static) {
    let shell = opusdm_ui::DialogShell::new(parent, "VPN password");
    let label = gtk::Label::new(Some(&format!("Password for {name}")));
    label.set_xalign(0.0);
    let entry = gtk::PasswordEntry::new();
    entry.set_show_peek_icon(true);
    entry.set_width_chars(28);
    entry.set_activates_default(true);
    let content = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    content.append(&label);
    content.append(&entry);
    shell.content().append(&content);
    shell.close_button("Cancel");
    let connect = shell.button("Connect");
    connect.add_css_class("suggested-action");
    connect.set_sensitive(false);
    shell.window().set_default_widget(Some(&connect));
    {
        let connect = connect.clone();
        entry.connect_changed(move |e| connect.set_sensitive(!e.text().is_empty()));
    }
    {
        let (window, entry) = (shell.window().clone(), entry.clone());
        connect.connect_clicked(move |_| {
            let password = entry.text().to_string();
            entry.set_text("");
            on_submit(password);
            window.close();
        });
    }
    shell.present();
    entry.grab_focus();
}
