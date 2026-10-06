//! Manual check: run the panel on its own, with no hub desktop window.
use gtk4 as gtk;
use gtk::prelude::*;

fn main() {
    let app = gtk::Application::builder()
        .application_id("org.opusdm.PanelStandalone")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(|app| {
        let position = opusdm_ipc::settings::load_desktop_settings("panel-standalone").panel_position;
        opusdm_panel::attach(app, position, std::rc::Rc::new(|path| {
            let _ = std::process::Command::new("xdg-open").arg(path).spawn();
        }));
    });
    app.run_with_args::<&str>(&[]);
}
