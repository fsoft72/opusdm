//! The panel's link to `opusdm-notify` over `org.opusdm.Notifications1`.
//! A thread listens for `Changed` and for the server coming and going, and
//! sends the fresh state to the GTK loop; the user's requests go out on
//! short threads so the panel never waits on the bus.

use opusdm_ipc::notify::{NotifyState, StateTuple, CHANGED_SIGNAL, CONTROL_IFACE, CONTROL_NAME, CONTROL_PATH};
use std::sync::{Arc, OnceLock};
use zbus::blocking::Connection;

/// What the listener thread reports.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// The server's current state.
    State(NotifyState),
    /// The server is not running.
    Gone,
}

/// Sends the user's requests to the server.
#[derive(Clone)]
pub struct Client {
    /// Set by the listener thread once it is connected.
    calls: Arc<OnceLock<Connection>>,
}

impl Client {
    /// Run the default action of history entry `id`.
    pub fn activate(&self, id: u32) {
        self.call("Activate", (id,));
    }

    /// Remove history entry `id`.
    pub fn remove(&self, id: u32) {
        self.call("Remove", (id,));
    }

    /// Empty the history.
    pub fn clear_history(&self) {
        self.call("ClearHistory", ());
    }

    /// The history has been seen.
    pub fn mark_read(&self) {
        self.call("MarkRead", ());
    }

    /// Turn Do Not Disturb on or off.
    pub fn set_dnd(&self, on: bool) {
        self.call("SetDnd", (on,));
    }

    /// Mute or unmute `app_name`.
    pub fn set_muted(&self, app_name: &str, muted: bool) {
        self.call(if muted { "Mute" } else { "Unmute" }, (app_name.to_string(),));
    }

    /// Call `method` on a short thread; a failure is logged. The answer
    /// comes back as a `Changed` signal.
    fn call<B>(&self, method: &'static str, body: B)
    where
        B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
    {
        let Some(conn) = self.calls.get().cloned() else {
            opusdm_ipc::err_log!("opusdm-panel", "notifications: not connected, {method} dropped");
            return;
        };
        let spawned = std::thread::Builder::new().name("opusdm-panel-notify-call".into()).spawn(move || {
            if let Err(e) = conn.call_method(Some(CONTROL_NAME), CONTROL_PATH, Some(CONTROL_IFACE), method, &body) {
                opusdm_ipc::err_log!("opusdm-panel", "notifications: {method} failed: {e}");
            }
        });
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("opusdm-panel", "notifications: cannot start a thread for {method}: {e}");
        }
    }
}

/// Start the listener thread. Events arrive on the returned receiver.
pub fn spawn() -> (Client, async_channel::Receiver<ClientEvent>) {
    let (tx, rx) = async_channel::unbounded();
    let client = Client { calls: Arc::new(OnceLock::new()) };
    let calls = client.calls.clone();
    let spawned = std::thread::Builder::new().name("opusdm-panel-notify".into()).spawn(move || {
        if let Err(e) = listen(&calls, &tx) {
            opusdm_ipc::err_log!("opusdm-panel", "notifications: listener stopped: {e}");
            let _ = tx.send_blocking(ClientEvent::Gone);
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-panel", "notifications: cannot start the listener: {e}");
    }
    (client, rx)
}

/// The match rule of the server's `Changed` signal.
fn changed_rule() -> String {
    format!("type='signal',sender='{CONTROL_NAME}',path='{CONTROL_PATH}',interface='{CONTROL_IFACE}',member='{CHANGED_SIGNAL}'")
}

/// The match rule of the server's name changing owner.
fn owner_rule() -> String {
    format!("type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='{CONTROL_NAME}'")
}

/// Connect, subscribe, then send the state after every change until the
/// session bus goes away.
fn listen(calls_slot: &OnceLock<Connection>, tx: &async_channel::Sender<ClientEvent>) -> Result<(), Box<dyn std::error::Error>> {
    let calls = Connection::session()?;
    let _ = calls_slot.set(calls.clone());
    // Signals only on this one: it never waits for a reply while the
    // iterator below is the only reader.
    let signals = Connection::session()?;
    let messages = zbus::blocking::MessageIterator::from(&signals);
    let bus = zbus::blocking::fdo::DBusProxy::new(&signals)?;
    for rule in [changed_rule(), owner_rule()] {
        bus.add_match_rule(zbus::MatchRule::try_from(rule.as_str())?)?;
    }
    tx.send_blocking(fetch(&calls))?;
    for msg in messages {
        let msg = msg?;
        let header = msg.header();
        if header.message_type() != zbus::message::Type::Signal { continue; }
        let member = header.member().map(|m| m.to_string()).unwrap_or_default();
        let event = match member.as_str() {
            "NameOwnerChanged" => match msg.body().deserialize::<(String, String, String)>() {
                Ok((_, _, new_owner)) if new_owner.is_empty() => ClientEvent::Gone,
                Ok(_) => fetch(&calls),
                Err(_) => continue,
            },
            CHANGED_SIGNAL => fetch(&calls),
            _ => continue,
        };
        tx.send_blocking(event)?;
    }
    Ok(())
}

/// Ask the server for its state; `Gone` when it does not answer.
fn fetch(calls: &Connection) -> ClientEvent {
    let reply = calls.call_method(Some(CONTROL_NAME), CONTROL_PATH, Some(CONTROL_IFACE), "GetState", &());
    match reply.and_then(|r| r.body().deserialize::<StateTuple>()) {
        Ok(state) => ClientEvent::State(NotifyState::from_tuple(state)),
        Err(e) => {
            opusdm_ipc::debug_log!("opusdm-panel", "notifications: no server: {e}");
            ClientEvent::Gone
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_rules_parse() {
        assert!(zbus::MatchRule::try_from(changed_rule().as_str()).is_ok());
        assert!(zbus::MatchRule::try_from(owner_rule().as_str()).is_ok());
        assert!(owner_rule().contains("arg0='org.opusdm.Notifications1'"));
    }
}
