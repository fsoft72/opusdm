//! `opusdm-network-manager`: edit the NetworkManager connections (Wi-Fi,
//! Ethernet, WireGuard and OpenVPN) and import VPN files, like the
//! Connections module of KDE Plasma. See
//! `docs/superpowers/specs/2026-10-12-network-manager-design.md`.

mod dialogs;
mod editor;
mod form;
mod pages;
mod vpn_pages;
mod window;

use gtk4 as gtk;
use gtk::prelude::*;

/// A field with an error gets a red outline, an error message red text and
/// the "not saved yet" banner the warning colour.
const APP_CSS: &str = "
.error:not(label) { outline: 1px solid #e01b24; outline-offset: -1px; }
label.error { color: #ff7b63; }
label.warning { color: #f6d32d; }
";

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.NetworkManager").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    app.connect_activate(|app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        opusdm_ui::install_app_css("opusdm-network-manager", APP_CSS);
        window::open(app);
    });
    app.run()
}
