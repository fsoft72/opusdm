//! `opusdm-applet-browser`: list the applets, enable or disable them, reload
//! them and turn their edit mode on or off, over the socket of the
//! `opusdm-applets` process.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_applet_browser::window;

/// GTK application id; unique, so a second launch raises the first window.
const APP_ID: &str = "org.opusdm.AppletBrowser";

/// Run the GTK application; every activation opens or raises the window.
fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    app.connect_activate(window::activate);
    app.run()
}
