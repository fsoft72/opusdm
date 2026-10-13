//! `opusdm-notify`: the desktop notification server of the OpusDM session.
//! The hub starts it at login. Applications (browsers, mail, `notify-send`)
//! send notifications to `org.freedesktop.Notifications`; this program shows
//! them as popups and keeps a history the panel's bell reads through
//! `org.opusdm.Notifications1`. See
//! `docs/superpowers/specs/2026-10-14-notification-server-design.md`.

mod config;
mod control;
mod markup;
mod model;
mod placement;
mod popup;
mod server;
mod store;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use popup::{PopupEvent, Popups};
use server::{Msg, Shared, Signals};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use store::{CloseReason, Store};

/// What keeps the server alive: the session bus connection its names live
/// on, and the hold that keeps the application running with no window.
struct Running {
    _conn: zbus::blocking::Connection,
    _hold: gtk::gio::ApplicationHoldGuard,
}

/// The GTK side: the popups, the store and the signals.
struct Screen {
    shared: Arc<Shared>,
    popups: Rc<Popups>,
    signals: Signals,
}

impl Screen {
    /// Act on one message of the D-Bus side.
    fn on_msg(&self, msg: Msg) {
        match msg {
            Msg::Show(n) => {
                let expire = self.shared.store().expire_ms(&n);
                self.popups.show(*n, expire);
            }
            Msg::Hide(id) => self.popups.hide(id),
            Msg::Invoke { id, key, resident } => self.invoke(id, &key, resident),
        }
    }

    /// Act on what the user did with a popup.
    fn on_popup(&self, event: PopupEvent) {
        match event {
            PopupEvent::Expired(id) => {
                let closed = self.shared.store().popup_expired(id);
                if closed {
                    self.signals.closed(id, CloseReason::Expired);
                }
                self.popups.hide(id);
            }
            PopupEvent::Dismissed(id) => {
                let removed = self.shared.store().remove(id);
                self.popups.hide(id);
                if removed.is_some() {
                    self.signals.closed(id, CloseReason::Dismissed);
                    self.signals.changed();
                }
            }
            PopupEvent::Action { id, key } => {
                let resident = self.shared.store().get(id).is_some_and(|n| n.resident);
                self.invoke(id, &key, resident);
            }
        }
    }

    /// Tell the application its action `key` was chosen, then close the
    /// notification unless it is `resident`.
    fn invoke(&self, id: u32, key: &str, resident: bool) {
        self.signals.action(id, key, activation_token().as_deref());
        if resident { return; }
        let removed = self.shared.store().remove(id);
        self.popups.hide(id);
        if removed.is_some() {
            self.signals.closed(id, CloseReason::Dismissed);
            self.signals.changed();
        }
    }
}

/// Wayland: a token that lets the application bring its window forward
/// (`ActivationToken`). X11 needs none: the hub follows activation requests.
fn activation_token() -> Option<String> {
    if !opusdm_ipc::session::is_wayland() { return None; }
    let display = gtk::gdk::Display::default()?;
    let context = display.app_launch_context();
    context.startup_notify_id(None::<&gtk::gio::AppInfo>, &[]).map(|s| s.to_string())
}

/// Connect to the session bus with both interfaces served and take the
/// names. Never replaces another notification server.
fn connect(shared: &Arc<Shared>) -> Result<zbus::blocking::Connection, String> {
    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(server::NOTIFY_PATH, server::Notifications { shared: shared.clone() }))
        .and_then(|b| b.serve_at(opusdm_ipc::notify::CONTROL_PATH, control::Control { shared: shared.clone() }))
        .and_then(|b| b.build())
        .map_err(|e| format!("cannot connect to the session bus: {e}"))?;
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    match conn.request_name_with_flags(server::NOTIFY_NAME, flags) {
        Ok(_) => {}
        Err(zbus::Error::NameTaken) => return Err(format!("another notification server owns {}, leaving it alone", server::NOTIFY_NAME)),
        Err(e) => return Err(format!("cannot take {}: {e}", server::NOTIFY_NAME)),
    }
    conn.request_name_with_flags(opusdm_ipc::notify::CONTROL_NAME, flags)
        .map_err(|e| format!("cannot take {}: {e}", opusdm_ipc::notify::CONTROL_NAME))?;
    Ok(conn)
}

/// Serve notifications until the session ends. Errors end the application.
fn start(app: &gtk::Application, running: &RefCell<Option<Running>>) {
    let config = config::load();
    let corner = config.corner;
    let (tx, rx) = async_channel::unbounded();
    let shared = Arc::new(Shared { store: Mutex::new(Store::new(config)), tx });
    let conn = match connect(&shared) {
        Ok(conn) => conn,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-notify", "{e}");
            return;
        }
    };
    opusdm_ui::install_app_css("opusdm-notify", popup::NOTIFY_CSS);

    let (event_tx, event_rx) = async_channel::unbounded();
    let popups = Popups::new(app, corner, move |event| {
        // Unbounded: never full, fails only once the loop below is gone.
        let _ = event_tx.try_send(event);
    });
    let screen = Rc::new(Screen { shared, popups, signals: Signals::new(conn.clone()) });
    {
        let screen = screen.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                screen.on_msg(msg);
            }
        });
    }
    glib::spawn_future_local(async move {
        while let Ok(event) = event_rx.recv().await {
            screen.on_popup(event);
        }
    });
    running.replace(Some(Running { _conn: conn, _hold: app.hold() }));
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Notify").build();
    let running: Rc<RefCell<Option<Running>>> = Rc::new(RefCell::new(None));
    // Startup runs only in the first instance: a second launch (a restarted
    // hub) hands over to it and exits.
    app.connect_startup(move |app| start(app, &running));
    // Nothing to show on activation: popups open on request.
    app.connect_activate(|_| {});
    app.run()
}
