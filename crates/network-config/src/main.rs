//! `opusdm-network-manager`: edit the NetworkManager connections (Wi-Fi,
//! Ethernet, WireGuard and OpenVPN) and import VPN files, like the
//! Connections module of KDE Plasma. See
//! `docs/superpowers/specs/2026-10-12-network-manager-design.md`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_network_config::{window, APP_CSS};

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
