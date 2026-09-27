//! Standalone entry point for `opusdm-dock`, handy for developing the dock
//! without the hub. The hub embeds the same dock through `opusdm_dock::attach`.

use gtk4 as gtk;
use gtk::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Dock").build();
    app.connect_activate(|app| {
        opusdm_dock::attach(app);
    });
    app.run()
}
