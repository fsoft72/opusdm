//! `opusdm-apps-editor`: edits the application menu (`menu.json`) and the
//! `.desktop` entries it lists, in the style of KDE's menu editor. See
//! `docs/usage.md`.

mod files;
mod ui;

use gtk4 as gtk;
use gtk::prelude::*;

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-apps-editor");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings));
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
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
