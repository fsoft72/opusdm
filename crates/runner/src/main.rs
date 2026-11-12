//! `opusdm-run`: the Run window. A key combination (a hotkey bound to
//! `exec:opusdm-run`) opens it at the top of the screen; the user types the
//! name of an installed application or a command, picks one of the
//! suggestions and presses Enter. See
//! `docs/superpowers/specs/2026-10-05-run-dialog-design.md`.

mod launch;
mod sources;
mod ui;

use gtk4 as gtk;
use gtk::prelude::*;

/// Load the shared OpusDM look (theme mode, fonts) plus the window style.
fn apply_theme() {
    opusdm_ui::install_app_css("opusdm-run", ui::RUN_CSS);
}

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Run").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    opusdm_ui::set_app_window_icon(&app, "opusdm-run", include_bytes!("../../../assets/apps/icons/run-3.svg"));
    app.connect_activate(|app| {
        // A second launch only closes the open window: skip the heavy work.
        if app.windows().is_empty() {
            apply_theme();
        }
        let panel = opusdm_ipc::settings::load_desktop_settings("opusdm-run").panel_position;
        let entries = if app.windows().is_empty() { sources::app_entries() } else { Vec::new() };
        let icons = if app.windows().is_empty() { sources::app_icons() } else { Default::default() };
        // The PATH scan stats thousands of files: it runs on a worker and
        // its commands join the list when it is done. A command an app runs
        // gets that app's icon.
        ui::open(app, entries, move || sources::with_app_icons(sources::path_commands(), &icons), panel);
    });
    app.run()
}
