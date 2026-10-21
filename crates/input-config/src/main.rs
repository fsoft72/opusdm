//! `opusdm-input-config`: mouse, touchpad and keyboard settings. Every change
//! is applied at once; Save keeps them, closing the window any other way puts
//! the previous values back. See
//! `docs/superpowers/specs/2026-10-06-input-config-design.md`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_input_config::backend::session_problem;
use opusdm_input_config::ui;

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    opusdm_ui::install_app_css("opusdm-input-config", "");
}

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.InputConfig").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    app.connect_activate(|app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        apply_theme();
        match session_problem() {
            Some(problem) => ui::open_problem(app, &problem),
            None => ui::open(app),
        }
    });
    app.run()
}
