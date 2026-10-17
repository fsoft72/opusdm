//! Off-thread persistence of the hub's settings files.

use crate::{desktop_settings, settings};
use opusdm_ipc::bindings::{self, Bindings};
use opusdm_ipc::format::{self, FormatStore};
use opusdm_ipc::DesktopSettings;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

/// Messages persisted to disk off the owner thread.
pub(super) enum PersistMsg {
    Settings(Box<opusdm_ipc::Settings>),
    Desktop(DesktopSettings),
    Bindings(Bindings),
    Formats(FormatStore),
    Functions(Vec<opusdm_ipc::function::Function>),
    /// Answered once every message queued before it has been written: the
    /// hub waits for it before it lets the process exit.
    Flush(Sender<()>),
}

/// Longest the shutdown waits for the saves still queued.
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

/// Wait until every save queued on `persist_tx` so far is on disk, at most
/// `timeout`. A timeout or a dead persist thread is logged: the caller exits
/// anyway.
pub(super) fn flush(persist_tx: &Sender<PersistMsg>, timeout: Duration) {
    let (done_tx, done_rx) = channel();
    if persist_tx.send(PersistMsg::Flush(done_tx)).is_err() {
        opusdm_ipc::err_log!("hub", "the persist thread is gone, pending saves are lost");
        return;
    }
    if done_rx.recv_timeout(timeout).is_err() {
        opusdm_ipc::err_log!("hub", "saves still pending after {timeout:?}, exiting anyway");
    }
}

/// Start the persistence thread and return the channel that feeds it.
///
/// Settings, desktop settings, bindings, formats and functions are written to disk on a
/// thread of their own, so a slow filesystem cannot stall the owner loop; the
/// owner keeps the authoritative in-memory copy and only hands off the bytes.
pub(super) fn spawn_persist_thread() -> Sender<PersistMsg> {
    let (persist_tx, persist_rx) = channel::<PersistMsg>();
    std::thread::Builder::new()
        .name("opusdm-persist".into())
        .spawn(move || {
            for msg in persist_rx {
                match msg {
                    PersistMsg::Settings(s) => {
                        if let Err(e) = settings::save_settings(&s) {
                            opusdm_ipc::err_log!("hub", "cannot save settings.json: {e}");
                        }
                    }
                    PersistMsg::Desktop(d) => {
                        if let Err(e) = desktop_settings::save(&d) {
                            opusdm_ipc::err_log!("hub", "cannot save desktop.json: {e}");
                        }
                    }
                    PersistMsg::Bindings(b) => {
                        if let Err(e) = bindings::save_bindings(&b) {
                            opusdm_ipc::err_log!("hub", "cannot save bindings: {e}");
                        }
                    }
                    PersistMsg::Functions(f) => {
                        if let Err(e) = opusdm_ipc::function::save_functions(&f) {
                            opusdm_ipc::err_log!("hub", "cannot save functions.json: {e}");
                        }
                    }
                    PersistMsg::Formats(f) => {
                        if let Err(e) = format::save_formats(&f) {
                            opusdm_ipc::err_log!("hub", "cannot save formats.json: {e}");
                        }
                    }
                    PersistMsg::Flush(done) => {
                        // The waiter may have timed out already.
                        let _ = done.send(());
                    }
                }
            }
            opusdm_ipc::debug_log!("hub", "persist thread exiting");
        })
        .expect("spawn persist thread");
    persist_tx
}
