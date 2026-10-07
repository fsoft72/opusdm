//! A NetworkManager secret agent: NetworkManager asks it for the key of a
//! profile that keeps the key out of its own files (`psk-flags` 1), and it
//! answers from the keyring. It never asks the user: the panel does that
//! before it connects, and keeps the answer in the keyring.

use crate::error::NetError;
use crate::keystore::{KeyStore, PSK_KEY, SECURITY_GROUP};
use std::collections::HashMap;
use std::sync::Arc;
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const MANAGER_PATH: &str = "/org/freedesktop/NetworkManager/AgentManager";
const MANAGER_IFACE: &str = "org.freedesktop.NetworkManager.AgentManager";
const SERVICE: &str = "org.freedesktop.NetworkManager";
/// How NetworkManager tells this agent from the others.
const IDENTIFIER: &str = "org.opusdm.network";
/// `NM_SECRET_AGENT_GET_SECRETS_FLAG_REQUEST_NEW`: the key given before was refused.
const REQUEST_NEW: u32 = 0x2;
/// The settings group that names the profile.
const CONNECTION_GROUP: &str = "connection";

/// `a{sa{sv}}`: a connection profile.
pub type Settings = HashMap<String, HashMap<String, OwnedValue>>;

/// The errors a secret agent may answer with.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The agent has no key for this profile.
    NoSecrets(String),
    /// The caller is not NetworkManager.
    NotAuthorized(String),
    /// The keyring failed.
    InternalError(String),
}

/// One text value of a profile.
fn text_of(settings: &Settings, group: &str, key: &str) -> Option<String> {
    let value = settings.get(group)?.get(key)?;
    String::try_from(value.clone()).ok()
}

/// The UUID of a profile, the key that finds its secret.
fn uuid_of(settings: &Settings) -> Result<String, AgentError> {
    text_of(settings, CONNECTION_GROUP, "uuid").ok_or_else(|| AgentError::NoSecrets("The profile has no uuid".to_string()))
}

/// The answer to `GetSecrets`: the key in the group NetworkManager asks for.
/// When NetworkManager asks for a new one, the kept key was refused: it is
/// forgotten, so the next connect asks the user again.
fn secrets_for(store: &dyn KeyStore, connection: &Settings, setting_name: &str, flags: u32) -> Result<Settings, AgentError> {
    if setting_name != SECURITY_GROUP { return Err(AgentError::NoSecrets(format!("No secrets for {setting_name}"))); }
    let uuid = uuid_of(connection)?;
    if flags & REQUEST_NEW != 0 {
        store.delete(&uuid).map_err(|e| AgentError::InternalError(e.text()))?;
        return Err(AgentError::NoSecrets("The key was refused".to_string()));
    }
    let key = store.get(&uuid).map_err(|e| AgentError::InternalError(e.text()))?;
    let key = key.ok_or_else(|| AgentError::NoSecrets("No key in the keyring".to_string()))?;
    let value = OwnedValue::try_from(Value::from(key)).map_err(|e| AgentError::InternalError(e.to_string()))?;
    Ok(HashMap::from([(SECURITY_GROUP.to_string(), HashMap::from([(PSK_KEY.to_string(), value)]))]))
}

/// What `SaveSecrets` keeps: the key of a new profile. A profile with no key
/// (an open network) has nothing to keep.
fn save_secrets(store: &dyn KeyStore, connection: &Settings) -> Result<(), AgentError> {
    let Some(key) = text_of(connection, SECURITY_GROUP, PSK_KEY) else { return Ok(()) };
    let uuid = uuid_of(connection)?;
    let name = text_of(connection, CONNECTION_GROUP, "id").unwrap_or_else(|| uuid.clone());
    store.set(&uuid, &name, &key).map_err(|e| AgentError::InternalError(e.text()))
}

/// What `DeleteSecrets` does: forget the key of a profile that was removed.
fn delete_secrets(store: &dyn KeyStore, connection: &Settings) -> Result<(), AgentError> {
    let uuid = uuid_of(connection)?;
    store.delete(&uuid).map_err(|e| AgentError::InternalError(e.text()))
}

/// The object NetworkManager calls.
struct SecretAgent {
    store: Arc<dyn KeyStore>,
    /// The unique bus name of NetworkManager: the only caller served, or any
    /// process on the bus could read the keys.
    nm_owner: String,
}

impl SecretAgent {
    /// Refuse a call that does not come from NetworkManager.
    fn check_sender(&self, header: &Header<'_>) -> Result<(), AgentError> {
        match header.sender() {
            Some(sender) if sender.as_str() == self.nm_owner => Ok(()),
            _ => Err(AgentError::NotAuthorized("Only NetworkManager may call the agent".to_string())),
        }
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl SecretAgent {
    /// The secrets of one setting of a profile.
    fn get_secrets(
        &self,
        connection: Settings,
        _connection_path: ObjectPath<'_>,
        setting_name: &str,
        _hints: Vec<String>,
        flags: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Settings, AgentError> {
        self.check_sender(&header)?;
        secrets_for(&*self.store, &connection, setting_name, flags)
    }

    /// Nothing waits for the user here, so there is nothing to cancel.
    fn cancel_get_secrets(&self, _connection_path: ObjectPath<'_>, _setting_name: &str) {}

    /// Keep the secrets of a profile that was just added.
    fn save_secrets(&self, connection: Settings, _connection_path: ObjectPath<'_>, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.check_sender(&header)?;
        save_secrets(&*self.store, &connection)
    }

    /// Forget the secrets of a profile that was removed.
    fn delete_secrets(&self, connection: Settings, _connection_path: ObjectPath<'_>, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.check_sender(&header)?;
        delete_secrets(&*self.store, &connection)
    }
}

/// Serve the agent on `conn` and register it with NetworkManager, which must
/// be running. The registration ends with the connection.
pub fn register(conn: &Connection, store: Arc<dyn KeyStore>) -> Result<(), NetError> {
    let bus = zbus::blocking::fdo::DBusProxy::new(conn).map_err(|e| NetError::from_zbus(&e))?;
    let name = zbus::names::BusName::try_from(SERVICE).map_err(|e| NetError::other(e.to_string()))?;
    let nm_owner = bus.get_name_owner(name).map_err(|e| NetError::other(e.to_string()))?.to_string();
    conn.object_server().at(AGENT_PATH, SecretAgent { store, nm_owner }).map_err(|e| NetError::from_zbus(&e))?;
    let manager = zbus::blocking::Proxy::new(conn, SERVICE, MANAGER_PATH, MANAGER_IFACE).map_err(|e| NetError::from_zbus(&e))?;
    manager.call::<_, _, ()>("Register", &(IDENTIFIER,)).map_err(|e| NetError::from_zbus(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystore::FakeKeyStore;

    fn owned(text: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(text)).unwrap()
    }

    /// A profile with the given uuid, name and key.
    fn profile(uuid: &str, id: &str, key: Option<&str>) -> Settings {
        let mut s = Settings::new();
        s.insert(CONNECTION_GROUP.to_string(), HashMap::from([("uuid".to_string(), owned(uuid)), ("id".to_string(), owned(id))]));
        if let Some(key) = key {
            s.insert(SECURITY_GROUP.to_string(), HashMap::from([(PSK_KEY.to_string(), owned(key))]));
        }
        s
    }

    #[test]
    fn a_kept_key_is_answered_in_the_security_group() {
        let store = FakeKeyStore::default();
        store.set("u1", "home", "secret").unwrap();
        let answer = secrets_for(&store, &profile("u1", "home", None), SECURITY_GROUP, 0).unwrap();
        assert_eq!(text_of(&answer, SECURITY_GROUP, PSK_KEY).as_deref(), Some("secret"));
    }

    #[test]
    fn no_kept_key_is_no_secrets() {
        let store = FakeKeyStore::default();
        let err = secrets_for(&store, &profile("u1", "home", None), SECURITY_GROUP, 0).unwrap_err();
        assert!(matches!(err, AgentError::NoSecrets(_)));
    }

    #[test]
    fn other_settings_and_profiles_without_uuid_get_no_secrets() {
        let store = FakeKeyStore::default();
        store.set("u1", "home", "secret").unwrap();
        assert!(matches!(secrets_for(&store, &profile("u1", "home", None), "802-1x", 0), Err(AgentError::NoSecrets(_))));
        assert!(matches!(secrets_for(&store, &Settings::new(), SECURITY_GROUP, 0), Err(AgentError::NoSecrets(_))));
    }

    #[test]
    fn a_refused_key_is_forgotten_so_the_user_is_asked_again() {
        let store = FakeKeyStore::default();
        store.set("u1", "home", "wrong").unwrap();
        let err = secrets_for(&store, &profile("u1", "home", None), SECURITY_GROUP, REQUEST_NEW | 0x1).unwrap_err();
        assert!(matches!(err, AgentError::NoSecrets(_)));
        assert_eq!(store.get("u1").unwrap(), None);
    }

    #[test]
    fn a_new_profile_has_its_key_kept_and_an_open_one_nothing() {
        let store = FakeKeyStore::default();
        save_secrets(&store, &profile("u1", "home", Some("secret"))).unwrap();
        assert_eq!(store.get("u1").unwrap().as_deref(), Some("secret"));
        save_secrets(&store, &profile("u2", "cafe", None)).unwrap();
        assert_eq!(store.get("u2").unwrap(), None);
    }

    #[test]
    fn a_removed_profile_loses_its_key() {
        let store = FakeKeyStore::default();
        store.set("u1", "home", "secret").unwrap();
        delete_secrets(&store, &profile("u1", "home", None)).unwrap();
        assert_eq!(store.get("u1").unwrap(), None);
    }
}
