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
    let app = gtk::Application::builder().application_id("org.opusdm.AppsEditor").build();
    app.connect_activate(|app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        apply_theme();
        ui::open(app);
    });
    app.run()
}
