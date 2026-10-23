//! `opusdm-bluetooth-manager`: turn Bluetooth on and off, pair, connect and
//! manage the devices, like the Bluetooth module of KDE Plasma. With
//! `--pair` the "Add device" dialog opens at once. See
//! `docs/superpowers/specs/2026-10-23-bluetooth-design.md`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_bluetooth_config::{window, APP_CSS};

/// Open "Add device" at start.
const PAIR_FLAG: &str = "--pair";

fn main() -> gtk::glib::ExitCode {
    let pair = std::env::args().any(|a| a == PAIR_FLAG);
    let app = gtk::Application::builder().application_id("org.opusdm.BluetoothManager").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    app.connect_activate(move |app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        opusdm_ui::install_app_css("opusdm-bluetooth-manager", APP_CSS);
        window::open(app, pair);
    });
    // GTK would refuse the flags it does not know: they were read above.
    app.run_with_args::<&str>(&[])
}
