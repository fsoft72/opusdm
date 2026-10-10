//! `opusdm-input-config`: mouse, touchpad and keyboard settings. Every change
//! is applied at once; Save keeps them, closing the window any other way puts
//! the previous values back. See
//! `docs/superpowers/specs/2026-10-06-input-config-design.md`.

mod ui;

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_input_config::backend::{for_session, session_problem};

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-input-config");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings));
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.InputConfig").build();
    app.connect_activate(|app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        apply_theme();
        match session_problem() {
            Some(problem) => ui::open_problem(app, &problem),
            None => ui::open(app, for_session()),
        }
    });
    app.run()
}
