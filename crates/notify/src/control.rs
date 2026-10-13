//! `org.opusdm.Notifications1`: the panel's bell reads the history and
//! changes Do Not Disturb and the mute list here. Every change ends with the
//! `Changed` signal; the panel then asks [`Control::get_state`] again.

use crate::config;
use crate::server::{emit_changed, emit_closed, Msg, Shared};
use crate::store::CloseReason;
use crate::model::DEFAULT_ACTION;
use opusdm_ipc::notify::StateTuple;
use std::sync::Arc;

/// The object the panel calls.
pub struct Control {
    pub shared: Arc<Shared>,
}

impl Control {
    /// Save the settings after a change of DND or the mute list.
    fn save(&self) {
        let config = self.shared.store().config().clone();
        config::save(&config);
    }
}

#[zbus::interface(name = "org.opusdm.Notifications1")]
impl Control {
    /// DND, unread count, history (newest first), muted apps.
    fn get_state(&self) -> StateTuple {
        self.shared.store().state().to_tuple()
    }

    /// Run the default action of the history entry `id`.
    async fn activate(&self, id: u32) {
        let resident = {
            let store = self.shared.store();
            let Some(n) = store.get(id) else { return };
            if n.default_action().is_none() { return; }
            n.resident
        };
        self.shared.send(Msg::Invoke { id, key: DEFAULT_ACTION.to_string(), resident }).await;
    }

    /// Close the notification `id` (removed from the history by the user).
    async fn remove(&self, id: u32, #[zbus(connection)] conn: &zbus::Connection) {
        let Some(removed) = self.shared.store().remove(id) else { return };
        if removed.had_popup {
            self.shared.send(Msg::Hide(id)).await;
        }
        emit_closed(conn, id, CloseReason::Dismissed).await;
        emit_changed(conn).await;
    }

    /// Close every notification of the history.
    async fn clear_history(&self, #[zbus(connection)] conn: &zbus::Connection) {
        let removed = self.shared.store().clear_history();
        for r in removed {
            let id = r.notification.id;
            if r.had_popup {
                self.shared.send(Msg::Hide(id)).await;
            }
            emit_closed(conn, id, CloseReason::Dismissed).await;
        }
        emit_changed(conn).await;
    }

    /// The user has seen the history.
    async fn mark_read(&self, #[zbus(connection)] conn: &zbus::Connection) {
        let changed = self.shared.store().mark_read();
        if changed {
            emit_changed(conn).await;
        }
    }

    /// Turn Do Not Disturb on or off.
    async fn set_dnd(&self, on: bool, #[zbus(connection)] conn: &zbus::Connection) {
        let changed = self.shared.store().set_dnd(on);
        if !changed { return; }
        self.save();
        emit_changed(conn).await;
    }

    /// Send the notifications of `app_name` to the history only.
    async fn mute(&self, app_name: String, #[zbus(connection)] conn: &zbus::Connection) {
        let changed = self.shared.store().mute(&app_name);
        if !changed { return; }
        self.save();
        emit_changed(conn).await;
    }

    /// Show the notifications of `app_name` again.
    async fn unmute(&self, app_name: String, #[zbus(connection)] conn: &zbus::Connection) {
        let changed = self.shared.store().unmute(&app_name);
        if !changed { return; }
        self.save();
        emit_changed(conn).await;
    }
}
