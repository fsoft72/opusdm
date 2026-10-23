//! `opusdm-printer-manager`: add, edit and remove CUPS printers and manage
//! their print queues, like the Printers module of KDE Plasma. See
//! `docs/superpowers/specs/2026-10-23-printer-manager-design.md`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_printer_config::window::{self, APP_CSS};

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.PrinterManager").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    app.connect_activate(|app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        opusdm_ui::install_app_css("opusdm-printer-manager", APP_CSS);
        window::open(app);
    });
    app.run()
}
