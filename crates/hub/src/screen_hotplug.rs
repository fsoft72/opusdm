//! Applies the saved screen layout (`screens.json`) when the hub starts and
//! whenever a monitor is plugged in or removed. The X server tells us through
//! RandR; on Wayland the hub's GDK monitor watcher calls [`monitors_changed`]
//! and the layout is set with `wlr-randr`. The decision is
//! `opusdm_screen::reapply::reapply_if_changed`.

use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use opusdm_screen::backend::{ScreenBackend, WlrRandrBackend, XrandrBackend};
use opusdm_screen::reapply::reapply_if_changed;
use opusdm_screen::store;
use x11rb::connection::Connection;
use x11rb::protocol::randr::{ConnectionExt as _, NotifyMask};
use x11rb::protocol::Event;

/// Quiet time after the last RandR event before the monitors are read: a
/// plug sends several events and the outputs need a moment to settle.
const SETTLE_MS: u64 = 500;

/// Start the thread that keeps the saved screen layout applied. A failure is
/// logged and never stops the hub.
pub(crate) fn start() {
    if opusdm_ipc::session::is_wayland() { return start_wayland(); }
    if !opusdm_ipc::session::is_x11() { return; }
    let spawned = std::thread::Builder::new().name("screen-hotplug".to_string()).spawn(|| {
        if let Err(e) = run() {
            opusdm_ipc::err_log!("opusdm-hub", "screen hot plug stopped: {e}");
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start the screen hot plug thread: {e}");
    }
}

/// Apply the profile now, then once per burst of RandR events.
fn run() -> Result<(), String> {
    let opusdm_x11::Session { conn, root, .. } = opusdm_x11::connect().map_err(|e| format!("cannot connect to X: {e}"))?;
    conn.randr_select_input(root, NotifyMask::SCREEN_CHANGE)
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| format!("no RandR events: {e}"))?;
    let backend = XrandrBackend::new();
    let path = store::profiles_path();
    let mut last_key = None;
    reapply(&backend, &mut last_key, &path);
    loop {
        let event = conn.wait_for_event().map_err(|e| e.to_string())?;
        if !matches!(event, Event::RandrScreenChangeNotify(_)) { continue; }
        // Wait until no event has come for a while.
        loop {
            std::thread::sleep(Duration::from_millis(SETTLE_MS));
            let mut more = false;
            while conn.poll_for_event().map_err(|e| e.to_string())?.is_some() {
                more = true;
            }
            if !more { break; }
        }
        reapply(&backend, &mut last_key, &path);
    }
}

/// Where [`monitors_changed`] sends on Wayland, once [`start`] ran.
static WAYLAND_CHANGES: OnceLock<mpsc::Sender<()>> = OnceLock::new();

/// The monitors changed (Wayland: called by the hub's GDK monitor watcher).
/// Does nothing on X11, where RandR tells the hot plug thread directly.
pub(crate) fn monitors_changed() {
    if let Some(tx) = WAYLAND_CHANGES.get() {
        let _ = tx.send(());
    }
}

/// [`start`] on Wayland: apply the profile now, then once per burst of
/// [`monitors_changed`] calls.
fn start_wayland() {
    let (tx, rx) = mpsc::channel::<()>();
    if WAYLAND_CHANGES.set(tx).is_err() { return; }
    let spawned = std::thread::Builder::new().name("screen-hotplug".to_string()).spawn(move || {
        let backend = WlrRandrBackend;
        let path = store::profiles_path();
        let mut last_key = None;
        reapply(&backend, &mut last_key, &path);
        while rx.recv().is_ok() {
            // Wait until no change has come for a while.
            loop {
                std::thread::sleep(Duration::from_millis(SETTLE_MS));
                if rx.try_iter().count() == 0 { break; }
            }
            reapply(&backend, &mut last_key, &path);
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start the screen hot plug thread: {e}");
    }
}

/// One look at the monitors; a problem is logged.
fn reapply(backend: &impl ScreenBackend, last_key: &mut Option<Vec<String>>, path: &std::path::Path) {
    match reapply_if_changed(backend, last_key, path) {
        Ok(true) => opusdm_ipc::debug_log!("opusdm-hub", "applied the saved screen layout"),
        Ok(false) => {}
        Err(e) => opusdm_ipc::err_log!("opusdm-hub", "screen layout: {e}"),
    }
}
