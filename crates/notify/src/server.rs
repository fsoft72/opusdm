//! `org.freedesktop.Notifications` on the session bus: applications send
//! their notifications here. Calls are answered at once from the shared
//! store; what the screen must show goes to the GTK loop as a [`Msg`].

use crate::model::Notification;
use crate::store::{CloseReason, Route, Store};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use zbus::message::Header;
use zbus::zvariant::OwnedValue;

/// Bus name of the specification.
pub const NOTIFY_NAME: &str = "org.freedesktop.Notifications";
/// Object path of the specification.
pub const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
/// Interface name of the specification.
pub const NOTIFY_IFACE: &str = "org.freedesktop.Notifications";
/// Version of the specification served.
const SPEC_VERSION: &str = "1.2";
/// What `GetCapabilities` answers.
const CAPABILITIES: [&str; 6] = ["actions", "body", "body-markup", "body-hyperlinks", "icon-static", "persistence"];

/// What the D-Bus side asks of the GTK loop.
#[derive(Debug)]
pub enum Msg {
    /// Show the popup of this notification, or update it when it is up.
    Show(Box<Notification>),
    /// Take the popup of this id away (no signal: the caller sent it).
    Hide(u32),
    /// Run the action `key` of the notification (from the panel history),
    /// then close it unless `resident`.
    Invoke { id: u32, key: String, resident: bool },
}

/// What both interfaces share with the GTK loop.
pub struct Shared {
    pub store: Mutex<Store>,
    pub tx: async_channel::Sender<Msg>,
}

impl Shared {
    /// The store, also after a panic of another holder (its data stays usable).
    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hand `msg` to the GTK loop.
    pub async fn send(&self, msg: Msg) {
        if self.tx.send(msg).await.is_err() {
            opusdm_ipc::err_log!("opusdm-notify", "the GTK loop is gone, message dropped");
        }
    }
}

/// The object applications call.
pub struct Notifications {
    pub shared: Arc<Shared>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    /// Show a notification; returns its id.
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> u32 {
        let mut n = Notification::from_call(app_name, app_icon, summary, body, &actions, &hints, expire_timeout);
        n.sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        n.time = now();
        opusdm_ipc::debug_log!("opusdm-notify", "Notify from {} ({}): {:?}", n.app_name, n.sender, n.summary);
        let (added, shown) = {
            let mut store = self.shared.store();
            let added = store.notify(n, replaces_id);
            let shown = store.get(added.id).cloned();
            (added, shown)
        };
        match (added.route, shown) {
            (Route::Popup { .. }, Some(n)) => self.shared.send(Msg::Show(Box::new(n))).await,
            _ if added.had_popup => self.shared.send(Msg::Hide(added.id)).await,
            _ => {}
        }
        if added.route == Route::Drop {
            emit_closed(conn, added.id, CloseReason::Expired).await;
        }
        for id in added.evicted {
            emit_closed(conn, id, CloseReason::Expired).await;
        }
        emit_changed(conn).await;
        added.id
    }

    /// The application takes its notification back.
    async fn close_notification(&self, id: u32, #[zbus(connection)] conn: &zbus::Connection) {
        let Some(removed) = self.shared.store().remove(id) else { return };
        if removed.had_popup {
            self.shared.send(Msg::Hide(id)).await;
        }
        emit_closed(conn, id, CloseReason::Closed).await;
        emit_changed(conn).await;
    }

    /// The optional parts of the specification this server supports.
    fn get_capabilities(&self) -> Vec<String> {
        CAPABILITIES.iter().map(|c| c.to_string()).collect()
    }

    /// Name, vendor, version, specification version.
    fn get_server_information(&self) -> (String, String, String, String) {
        ("OpusDM".into(), "OpusDM".into(), opusdm_ui::VERSION.into(), SPEC_VERSION.into())
    }
}

/// Seconds since the epoch.
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Send `NotificationClosed`; a failure is logged.
pub async fn emit_closed(conn: &zbus::Connection, id: u32, reason: CloseReason) {
    let sent = conn.emit_signal(None::<()>, NOTIFY_PATH, NOTIFY_IFACE, "NotificationClosed", &(id, reason as u32)).await;
    if let Err(e) = sent {
        opusdm_ipc::err_log!("opusdm-notify", "cannot send NotificationClosed: {e}");
    }
}

/// Tell the panel something changed; a failure is logged.
pub async fn emit_changed(conn: &zbus::Connection) {
    use opusdm_ipc::notify::{CHANGED_SIGNAL, CONTROL_IFACE, CONTROL_PATH};
    if let Err(e) = conn.emit_signal(None::<()>, CONTROL_PATH, CONTROL_IFACE, CHANGED_SIGNAL, &()).await {
        opusdm_ipc::err_log!("opusdm-notify", "cannot send Changed: {e}");
    }
}

/// The signals the GTK loop sends, on the blocking connection.
#[derive(Clone)]
pub struct Signals {
    conn: zbus::blocking::Connection,
}

impl Signals {
    /// Signals sent on `conn`.
    pub fn new(conn: zbus::blocking::Connection) -> Self {
        Signals { conn }
    }

    /// `NotificationClosed(id, reason)`.
    pub fn closed(&self, id: u32, reason: CloseReason) {
        self.emit(NOTIFY_PATH, NOTIFY_IFACE, "NotificationClosed", &(id, reason as u32));
    }

    /// `ActionInvoked(id, key)`, after `ActivationToken` when there is one.
    pub fn action(&self, id: u32, key: &str, token: Option<&str>) {
        if let Some(token) = token {
            self.emit(NOTIFY_PATH, NOTIFY_IFACE, "ActivationToken", &(id, token));
        }
        self.emit(NOTIFY_PATH, NOTIFY_IFACE, "ActionInvoked", &(id, key));
    }

    /// The panel's `Changed`.
    pub fn changed(&self) {
        use opusdm_ipc::notify::{CHANGED_SIGNAL, CONTROL_IFACE, CONTROL_PATH};
        self.emit(CONTROL_PATH, CONTROL_IFACE, CHANGED_SIGNAL, &());
    }

    /// Send one signal; a failure is logged.
    fn emit<B: serde::Serialize + zbus::zvariant::DynamicType>(&self, path: &str, iface: &str, name: &str, body: &B) {
        if let Err(e) = self.conn.emit_signal(None::<()>, path, iface, name, body) {
            opusdm_ipc::err_log!("opusdm-notify", "cannot send {name}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use opusdm_ipc::notify::{NotifyState, StateTuple, CONTROL_IFACE, CONTROL_PATH};
    use std::time::Duration;
    use zbus::blocking::{Connection, MessageIterator};

    /// Serve both interfaces on a fresh connection; returns it with the
    /// receiver of the messages for the GTK loop.
    fn serve() -> (Connection, async_channel::Receiver<Msg>) {
        let (tx, rx) = async_channel::unbounded();
        let shared = Arc::new(Shared { store: Mutex::new(Store::new(Config::default())), tx });
        let conn = zbus::blocking::connection::Builder::session()
            .unwrap()
            .serve_at(NOTIFY_PATH, Notifications { shared: shared.clone() })
            .unwrap()
            .serve_at(CONTROL_PATH, crate::control::Control { shared })
            .unwrap()
            .build()
            .unwrap();
        (conn, rx)
    }

    /// Call `Notify` on `server` from `client`.
    fn notify(client: &Connection, server: &str, replaces: u32, summary: &str) -> u32 {
        let hints: HashMap<String, zbus::zvariant::Value> = HashMap::new();
        let args = ("test", replaces, "", summary, "body", Vec::<String>::new(), hints, -1i32);
        client.call_method(Some(server), NOTIFY_PATH, Some(NOTIFY_IFACE), "Notify", &args).unwrap().body().deserialize().unwrap()
    }

    /// Call a control method of `server` from `client`.
    fn control<B: serde::Serialize + zbus::zvariant::DynamicType>(client: &Connection, server: &str, method: &str, body: &B) -> zbus::message::Message {
        client.call_method(Some(server), CONTROL_PATH, Some(CONTROL_IFACE), method, body).unwrap()
    }

    /// Live check on the session bus: `cargo test -p opusdm-notify --
    /// --ignored` inside `dbus-run-session` (never against the real session,
    /// where the real server runs). It writes the settings, so it points
    /// `XDG_CONFIG_HOME` to a temporary folder.
    #[test]
    #[ignore]
    fn the_server_answers_on_the_bus() {
        let config_home = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", config_home.path());
        let (conn, rx) = serve();
        let server = conn.unique_name().unwrap().to_string();
        let client = Connection::session().unwrap();
        let rule = format!("type='signal',sender='{server}',interface='{NOTIFY_IFACE}',member='NotificationClosed'");
        let mut closed = MessageIterator::for_match_rule(rule.as_str(), &client, None).unwrap();

        let id = notify(&client, &server, 0, "first");
        assert_eq!(id, 1);
        assert_eq!(notify(&client, &server, id, "second"), id, "replaces_id keeps the id");
        for summary in ["first", "second"] {
            match rx.recv_blocking().unwrap() {
                Msg::Show(n) => assert_eq!(n.summary, summary),
                other => panic!("unexpected {other:?}"),
            }
        }

        client.call_method(Some(server.as_str()), NOTIFY_PATH, Some(NOTIFY_IFACE), "CloseNotification", &(id,)).unwrap();
        let signal = closed.next().unwrap().unwrap();
        assert_eq!(signal.body().deserialize::<(u32, u32)>().unwrap(), (id, CloseReason::Closed as u32));
        assert!(matches!(rx.recv_blocking().unwrap(), Msg::Hide(i) if i == id));

        let caps: Vec<String> = client.call_method(Some(server.as_str()), NOTIFY_PATH, Some(NOTIFY_IFACE), "GetCapabilities", &()).unwrap().body().deserialize().unwrap();
        assert!(caps.iter().any(|c| c == "persistence") && caps.iter().any(|c| c == "actions"));
        let info: (String, String, String, String) = client.call_method(Some(server.as_str()), NOTIFY_PATH, Some(NOTIFY_IFACE), "GetServerInformation", &()).unwrap().body().deserialize().unwrap();
        assert_eq!((info.0.as_str(), info.3.as_str()), ("OpusDM", SPEC_VERSION));

        // Do Not Disturb: the next one goes to the history only.
        control(&client, &server, "SetDnd", &(true,));
        let quiet = notify(&client, &server, 0, "quiet");
        std::thread::sleep(Duration::from_millis(100));
        assert!(rx.try_recv().is_err(), "no popup while DND is on");
        let state: StateTuple = control(&client, &server, "GetState", &()).body().deserialize().unwrap();
        let state = NotifyState::from_tuple(state);
        assert!(state.dnd);
        assert_eq!(state.history.iter().map(|e| e.id).collect::<Vec<_>>(), vec![quiet]);
        assert_eq!(state.unread, 1);
        assert!(crate::config::path().exists(), "DND is saved");

        control(&client, &server, "Remove", &(quiet,));
        let signal = closed.next().unwrap().unwrap();
        assert_eq!(signal.body().deserialize::<(u32, u32)>().unwrap(), (quiet, CloseReason::Dismissed as u32));
    }
}
