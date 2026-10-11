//! `opusdm-polkit`: the polkit authentication agent of the OpusDM session.
//! The hub starts it at login. When a program needs administrator rights
//! through polkit (`pkexec`, Synaptic, disk tools), polkitd asks this agent,
//! which shows a password dialog. See
//! `docs/superpowers/specs/2026-10-11-polkit-agent-design.md`.

mod agent;
mod dialog;
mod helper;
mod identity;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Load the shared OpusDM look (theme mode, fonts) plus the dialog style.
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-polkit");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {} {}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings), dialog::POLKIT_CSS);
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// What keeps the agent alive: the system bus connection its registration
/// lives on, and the hold that keeps the application running with no window.
struct Running {
    _conn: zbus::blocking::Connection,
    _hold: gtk::gio::ApplicationHoldGuard,
}

/// Register with polkitd and show a dialog for every request. Errors end
/// the application.
fn start(app: &gtk::Application, running: &RefCell<Option<Running>>) {
    let conn = match zbus::blocking::Connection::system() {
        Ok(conn) => conn,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-polkit", "cannot connect to the system bus: {e}");
            return;
        }
    };
    let (tx, rx) = async_channel::unbounded();
    if let Err(e) = agent::register(&conn, tx) {
        opusdm_ipc::err_log!("opusdm-polkit", "{e}");
        return;
    }
    apply_theme();
    let dialogs = dialog::Dialogs::new(app);
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            dialogs.handle(msg);
        }
    });
    running.replace(Some(Running { _conn: conn, _hold: app.hold() }));
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.PolkitAgent").build();
    let running: Rc<RefCell<Option<Running>>> = Rc::new(RefCell::new(None));
    // Startup runs only in the first instance: a second launch (a restarted
    // hub) hands over to it and exits.
    app.connect_startup(move |app| start(app, &running));
    // Nothing to show on activation: the dialogs open on request.
    app.connect_activate(|_| {});
    app.run()
}
