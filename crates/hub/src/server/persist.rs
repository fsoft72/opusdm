//! Off-thread persistence of the hub's settings files.

use crate::{desktop_settings, settings};
use opusdm_ipc::bindings::{self, Bindings};
use opusdm_ipc::format::{self, FormatStore};
use opusdm_ipc::DesktopSettings;
use std::sync::mpsc::{channel, Sender};

/// Messages persisted to disk off the owner thread.
pub(super) enum PersistMsg {
    Settings(opusdm_ipc::Settings),
    Desktop(DesktopSettings),
    Bindings(Bindings),
    Formats(FormatStore),
}

/// Start the persistence thread and return the channel that feeds it.
///
/// Settings, desktop settings, bindings and formats are written to disk on a
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
                    PersistMsg::Formats(f) => {
                        if let Err(e) = format::save_formats(&f) {
                            opusdm_ipc::err_log!("hub", "cannot save formats.json: {e}");
                        }
                    }
                }
            }
            opusdm_ipc::debug_log!("hub", "persist thread exiting");
        })
        .expect("spawn persist thread");
    persist_tx
}
