//! `opusdm-applets` binary: owns the applets socket, runs the GTK application
//! and the host that shows every enabled applet in its own window.
//!
//! Ownership for the whole run: `main` keeps the only strong `Rc<Host>` and
//! the application hold guard in locals until `app.run()` returns. The
//! startup closure and the request loop hold only a `Weak<Host>` and log with
//! `err_log!` if it is gone.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_applets::host::{self, Host};
use opusdm_applets::instance::{self, Exit};
use opusdm_applets::server::{self, HostMessage, Shared};
use opusdm_applets::protocol;
use opusdm_ipc::err_log;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// GTK application id.
const APP_ID: &str = "org.opusdm.Applets";

/// Acquire the single-instance socket, start the accept thread, then run the
/// GTK application with the host until it quits.
fn main() -> gtk::glib::ExitCode {
    let listener = match instance::acquire() {
        Ok(l) => l,
        Err(Exit::AlreadyRunning) => {
            err_log!(APP_TAG, "opusdm-applets already running at {}", protocol::socket_path().display());
            std::process::exit(opusdm_ipc::instance::EXIT_ALREADY_RUNNING);
        }
        Err(Exit::Failed(msg)) => {
            err_log!(APP_TAG, "cannot start: {msg}");
            std::process::exit(1);
        }
    };
    let (to_host, from_conns) = async_channel::unbounded::<HostMessage>();
    let shared = Shared::new(Arc::new(AtomicBool::new(false)), to_host);
    // The accept thread owns the listener for the whole run; a fatal accept
    // error logs and exits the process, so the handle is never joined.
    let _accept = match server::spawn_accept(listener, shared) {
        Ok(h) => h,
        Err(e) => {
            err_log!(APP_TAG, "cannot start the accept thread: {e}");
            std::process::exit(1);
        }
    };
    // NON_UNIQUE: the socket above is the single-instance authority. A
    // D-Bus unique name would make a second process a silent "remote"
    // instance that never emits `startup`.
    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    // OWNER: this local is the only strong reference to the host for the
    // whole run; it is dropped only after `app.run()` returns.
    let host: Rc<Host> = Host::new(&app);
    // With every applet disabled there is no window: the hold keeps the
    // application running anyway. Kept in a local until `app.run()` returns.
    let hold = app.hold();
    let weak = Rc::downgrade(&host);
    let requests = RefCell::new(Some(from_conns));
    app.connect_startup(move |_| {
        let Some(host) = weak.upgrade() else {
            err_log!(APP_TAG, "startup: the host is gone, no applet is loaded and no request will be answered");
            return;
        };
        match requests.take() {
            Some(rx) => host::spawn_request_loop(&host, rx),
            None => err_log!(APP_TAG, "startup ran twice; the request loop is already running"),
        }
        host.load_enabled();
        host.watch_monitors();
    });
    // Nothing to do on activation: the applets open at startup.
    app.connect_activate(|_| {});
    // No argv: GApplication must never parse or refuse the process arguments.
    let code = app.run_with_args::<&str>(&[]);
    drop(hold);
    drop(host);
    code
}
