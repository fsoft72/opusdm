//! Manual check: run the panel on its own, with no hub desktop window.
use gtk4 as gtk;
use gtk::prelude::*;

fn main() {
    let app = gtk::Application::builder()
        .application_id("org.opusdm.PanelStandalone")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(|app| {
        opusdm_panel::attach(
            app,
            opusdm_panel::PanelHooks { on_settings: Box::new(|| eprintln!("settings")) },
        );
    });
    app.run_with_args::<&str>(&[]);
}
