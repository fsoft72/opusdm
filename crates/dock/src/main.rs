//! Standalone entry point for `opusdm-dock`, handy for developing the dock
//! without the hub. The hub embeds the same dock through `opusdm_dock::attach`.

use gtk4 as gtk;
use gtk::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Dock").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    app.connect_activate(|app| {
        let desktop = opusdm_ipc::settings::load_desktop_settings("opusdm-dock");
        let layout = opusdm_dock::DockLayout {
            position: desktop.dock_position,
            icon_size: desktop.dock_icon_size,
            panel: desktop.panel_position,
            panel_height: opusdm_ipc::PANEL_HEIGHT,
        };
        // The setter is dropped: the standalone dock never re-lays out live.
        let _ = opusdm_dock::attach(app, layout);
    });
    app.run()
}
