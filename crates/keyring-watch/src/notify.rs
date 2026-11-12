//! Desktop notifications through `org.freedesktop.Notifications`.

use std::collections::HashMap;
use std::time::Duration;
use zbus::blocking::Proxy;
use zbus::zvariant::Value;

/// The notification service.
const SERVICE: &str = "org.freedesktop.Notifications";
/// Its object path.
const PATH: &str = "/org/freedesktop/Notifications";
/// Its interface.
const IFACE: &str = "org.freedesktop.Notifications";
/// The application name shown by the notification service.
const APP_NAME: &str = "OpusDM";
/// The themed icon of the notification.
const ICON: &str = "dialog-password";
/// How long a call may wait for the notification service.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// The server decides how long the notification stays.
const DEFAULT_TIMEOUT: i32 = -1;

/// Show a notification and return its id. A `replaces_id` other than 0
/// replaces the notification with that id instead of adding a new one.
pub fn send(summary: &str, body: &str, replaces_id: u32) -> Result<u32, String> {
    let conn = crate::probe::session_bus(CALL_TIMEOUT).map_err(|e| format!("session bus: {e}"))?;
    let proxy = Proxy::new(&conn, SERVICE, PATH, IFACE).map_err(|e| format!("notification service: {e}"))?;
    let actions: Vec<&str> = Vec::new();
    let hints: HashMap<&str, Value> = HashMap::new();
    proxy
        .call("Notify", &(APP_NAME, replaces_id, ICON, summary, body, actions, hints, DEFAULT_TIMEOUT))
        .map_err(|e| format!("Notify: {e}"))
}
