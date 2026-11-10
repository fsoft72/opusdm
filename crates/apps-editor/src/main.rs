//! `opusdm-apps-editor`: edits the application menu (`menu.json`) and the
//! `.desktop` entries it lists, in the style of KDE's menu editor. See
//! `docs/usage.md`.

mod files;
mod ui;

use gtk4 as gtk;
use gtk::prelude::*;

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    opusdm_ui::install_app_css("opusdm-apps-editor", "");
}

fn main() -> gtk::glib::ExitCode {
    // Optional first argument: the desktop id of the entry to select (given by
    // the panel's "Edit App"). GTK gets no arguments of its own.
    let select = std::env::args().nth(1);
    let app = gtk::Application::builder().application_id("org.opusdm.AppsEditor").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    opusdm_ui::set_app_window_icon(&app, "opusdm-apps-editor", include_bytes!("../../../assets/apps/icons/app-editor.svg"));
    app.connect_activate(move |app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        apply_theme();
        ui::open(app, select.clone());
    });
    app.run_with_args::<&str>(&[])
}
