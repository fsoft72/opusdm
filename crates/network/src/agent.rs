//! A NetworkManager secret agent: NetworkManager asks it for the secrets of
//! a profile that keeps them out of its own files (flag 1, or 2 for a VPN
//! password asked every time), and it answers from the one-shot store, then
//! from the keyring. It never asks the user: the panel and the network
//! manager program do that before they connect.
//!
//! It answers for the Wi-Fi key, the OpenVPN secrets (`vpn.secrets`) and the
//! WireGuard keys.

use crate::error::NetError;
use crate::keystore::{psk_key, KeyStore, OneShotSecrets, PSK_KEY, SECURITY_GROUP};
use crate::profile::{peer_secret_key, ovpn_key, SecretKey, TYPE_WIREGUARD};
use crate::settings::{self, owned, Settings};
use std::collections::HashMap;
use std::sync::Arc;
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, Value};

const AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const MANAGER_PATH: &str = "/org/freedesktop/NetworkManager/AgentManager";
const MANAGER_IFACE: &str = "org.freedesktop.NetworkManager.AgentManager";
const SERVICE: &str = "org.freedesktop.NetworkManager";
/// The identifier of the panel's agent.
pub const PANEL_IDENTIFIER: &str = "org.opusdm.network";
/// The identifier of the network manager program's agent.
pub const MANAGER_IDENTIFIER: &str = "org.opusdm.network.manager";
/// The identifier of the agent of the Network tab in `opusdm-settings`, so it
/// never clashes with the network manager program open at the same time.
pub const SETTINGS_IDENTIFIER: &str = "org.opusdm.network.settings";
/// `NM_SECRET_AGENT_GET_SECRETS_FLAG_REQUEST_NEW`: the key given before was refused.
const REQUEST_NEW: u32 = 0x2;
/// The settings group that names the profile.
const CONNECTION_GROUP: &str = "connection";
/// The settings group of a VPN plugin profile.
const VPN_GROUP: &str = "vpn";
/// The VPN secrets always looked for, besides the ones NetworkManager hints.
const VPN_SECRETS: [&str; 2] = [ovpn_key::PASSWORD, ovpn_key::CERT_PASS];

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

/// Where the agent finds secrets: the one-shot store first, then the keyring.
pub struct SecretSources {
    pub keys: Arc<dyn KeyStore>,
    pub once: Arc<OneShotSecrets>,
}

impl SecretSources {
    /// One secret of the profile `uuid`.
    fn find(&self, uuid: &str, key: &SecretKey) -> Result<Option<String>, AgentError> {
        if let Some(v) = self.once.get(uuid, key) { return Ok(Some(v)); }
        self.keys.get(uuid, key).map_err(|e| AgentError::InternalError(e.text()))
    }
}

/// The UUID of a profile, the key that finds its secret.
fn uuid_of(settings: &Settings) -> Result<String, AgentError> {
    settings::text(settings, CONNECTION_GROUP, "uuid").ok_or_else(|| AgentError::NoSecrets("The profile has no uuid".to_string()))
}

/// The answer to `GetSecrets` for the group NetworkManager asks for.
pub fn secrets_for(src: &SecretSources, connection: &Settings, setting_name: &str, hints: &[String], flags: u32) -> Result<Settings, AgentError> {
    let uuid = uuid_of(connection)?;
    match setting_name {
        SECURITY_GROUP => wifi_secrets(src, &uuid, flags),
        VPN_GROUP => vpn_secrets(src, &uuid, hints, flags),
        TYPE_WIREGUARD => wireguard_secrets(src, connection, &uuid, flags),
        other => Err(AgentError::NoSecrets(format!("No secrets for {other}"))),
    }
}

/// The Wi-Fi key. When NetworkManager asks for a new one, the kept key was
/// refused: it is forgotten, so the next connect asks the user again.
fn wifi_secrets(src: &SecretSources, uuid: &str, flags: u32) -> Result<Settings, AgentError> {
    if flags & REQUEST_NEW != 0 {
        src.keys.delete(uuid, &psk_key()).map_err(|e| AgentError::InternalError(e.text()))?;
        return Err(AgentError::NoSecrets("The key was refused".to_string()));
    }
    let key = src.find(uuid, &psk_key())?.ok_or_else(|| AgentError::NoSecrets("No key in the keyring".to_string()))?;
    Ok(HashMap::from([(SECURITY_GROUP.to_string(), HashMap::from([(PSK_KEY.to_string(), owned(key))]))]))
}

/// The VPN plugin secrets, as the `vpn.secrets` dictionary. A refused VPN
/// secret is not forgotten: it is changed in the network manager program.
fn vpn_secrets(src: &SecretSources, uuid: &str, hints: &[String], flags: u32) -> Result<Settings, AgentError> {
    if flags & REQUEST_NEW != 0 { return Err(AgentError::NoSecrets("The VPN secrets were refused".to_string())); }
    let mut names: Vec<&str> = VPN_SECRETS.to_vec();
    // Hints starting with `x-` are messages for an interactive dialog.
    names.extend(hints.iter().map(String::as_str).filter(|h| !h.starts_with("x-") && !VPN_SECRETS.contains(h)));
    let mut found = HashMap::new();
    for name in names {
        if let Some(v) = src.find(uuid, &SecretKey::new(VPN_GROUP, name))? {
            found.insert(name.to_string(), v);
        }
    }
    if found.is_empty() { return Err(AgentError::NoSecrets("No VPN secrets in the keyring".to_string())); }
    Ok(HashMap::from([(VPN_GROUP.to_string(), HashMap::from([("secrets".to_string(), owned(found))]))]))
}

/// The WireGuard private key and the preshared key of each peer that has one.
fn wireguard_secrets(src: &SecretSources, connection: &Settings, uuid: &str, flags: u32) -> Result<Settings, AgentError> {
    if flags & REQUEST_NEW != 0 { return Err(AgentError::NoSecrets("The WireGuard keys were refused".to_string())); }
    let mut group = HashMap::new();
    if let Some(private) = src.find(uuid, &SecretKey::new(TYPE_WIREGUARD, "private-key"))? {
        group.insert("private-key".to_string(), owned(private));
    }
    let mut peers: Vec<HashMap<String, Value>> = Vec::new();
    for peer in settings::dicts(connection, TYPE_WIREGUARD, "peers") {
        let Some(public) = peer.get("public-key").and_then(settings::as_text) else { continue };
        let Some(psk) = src.find(uuid, &peer_secret_key(&public))? else { continue };
        peers.push(HashMap::from([("public-key".to_string(), Value::from(public)), ("preshared-key".to_string(), Value::from(psk))]));
    }
    if !peers.is_empty() { group.insert("peers".to_string(), owned(peers)); }
    if group.is_empty() { return Err(AgentError::NoSecrets("No WireGuard keys in the keyring".to_string())); }
    Ok(HashMap::from([(TYPE_WIREGUARD.to_string(), group)]))
}

/// What `SaveSecrets` keeps: the secrets present in a profile NetworkManager
/// just stored (the Wi-Fi key of a new network, VPN secrets, the WireGuard key).
pub fn save_secrets(store: &dyn KeyStore, connection: &Settings) -> Result<(), AgentError> {
    let uuid = uuid_of(connection)?;
    let name = settings::text(connection, CONNECTION_GROUP, "id").unwrap_or_else(|| uuid.clone());
    let mut found: Vec<(SecretKey, String)> = Vec::new();
    if let Some(psk) = settings::text(connection, SECURITY_GROUP, PSK_KEY) { found.push((psk_key(), psk)); }
    for (k, v) in settings::text_map(connection, VPN_GROUP, "secrets") {
        found.push((SecretKey::new(VPN_GROUP, &k), v));
    }
    if let Some(private) = settings::text(connection, TYPE_WIREGUARD, "private-key") {
        found.push((SecretKey::new(TYPE_WIREGUARD, "private-key"), private));
    }
    for (key, value) in found {
        store.set(&uuid, &name, &key, &value).map_err(|e| AgentError::InternalError(e.text()))?;
    }
    Ok(())
}

/// What `DeleteSecrets` does: forget every secret of a profile that was removed.
pub fn delete_secrets(store: &dyn KeyStore, connection: &Settings) -> Result<(), AgentError> {
    let uuid = uuid_of(connection)?;
    store.delete_all(&uuid).map_err(|e| AgentError::InternalError(e.text()))
}

/// The object NetworkManager calls.
struct SecretAgent {
    src: SecretSources,
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
        hints: Vec<String>,
        flags: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Settings, AgentError> {
        self.check_sender(&header)?;
        secrets_for(&self.src, &connection, setting_name, &hints, flags)
    }

    /// Nothing waits for the user here, so there is nothing to cancel.
    fn cancel_get_secrets(&self, _connection_path: ObjectPath<'_>, _setting_name: &str) {}

    /// Keep the secrets of a profile that was just added.
    fn save_secrets(&self, connection: Settings, _connection_path: ObjectPath<'_>, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.check_sender(&header)?;
        save_secrets(&*self.src.keys, &connection)
    }

    /// Forget the secrets of a profile that was removed.
    fn delete_secrets(&self, connection: Settings, _connection_path: ObjectPath<'_>, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.check_sender(&header)?;
        delete_secrets(&*self.src.keys, &connection)
    }
}

/// Serve the agent on `conn` and register it with NetworkManager under
/// `identifier`; NetworkManager must be running. The registration ends with
/// the connection.
pub fn register(conn: &Connection, src: SecretSources, identifier: &str) -> Result<(), NetError> {
    let bus = zbus::blocking::fdo::DBusProxy::new(conn)?;
    let name = zbus::names::BusName::try_from(SERVICE).map_err(|e| NetError::other(e.to_string()))?;
    let nm_owner = bus.get_name_owner(name).map_err(|e| NetError::other(e.to_string()))?.to_string();
    conn.object_server().at(AGENT_PATH, SecretAgent { src, nm_owner })?;
    let manager = zbus::blocking::Proxy::new(conn, SERVICE, MANAGER_PATH, MANAGER_IFACE)?;
    manager.call::<_, _, ()>("Register", &(identifier,)).map_err(NetError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystore::FakeKeyStore;

    /// A profile with the given uuid, name and Wi-Fi key.
    fn profile(uuid: &str, id: &str, key: Option<&str>) -> Settings {
        let mut s = Settings::new();
        settings::set(&mut s, CONNECTION_GROUP, "uuid", uuid);
        settings::set(&mut s, CONNECTION_GROUP, "id", id);
        if let Some(key) = key {
            settings::set(&mut s, SECURITY_GROUP, PSK_KEY, key);
        }
        s
    }

    fn sources() -> (SecretSources, Arc<FakeKeyStore>) {
        let keys = Arc::new(FakeKeyStore::default());
        (SecretSources { keys: keys.clone(), once: Arc::new(OneShotSecrets::default()) }, keys)
    }

    #[test]
    fn a_kept_key_is_answered_in_the_security_group() {
        let (src, keys) = sources();
        keys.set("u1", "home", &psk_key(), "secret").unwrap();
        let answer = secrets_for(&src, &profile("u1", "home", None), SECURITY_GROUP, &[], 0).unwrap();
        assert_eq!(settings::text(&answer, SECURITY_GROUP, PSK_KEY).as_deref(), Some("secret"));
    }

    #[test]
    fn no_kept_key_is_no_secrets() {
        let (src, _) = sources();
        let err = secrets_for(&src, &profile("u1", "home", None), SECURITY_GROUP, &[], 0).unwrap_err();
        assert!(matches!(err, AgentError::NoSecrets(_)));
    }

    #[test]
    fn other_settings_and_profiles_without_uuid_get_no_secrets() {
        let (src, keys) = sources();
        keys.set("u1", "home", &psk_key(), "secret").unwrap();
        assert!(matches!(secrets_for(&src, &profile("u1", "home", None), "802-1x", &[], 0), Err(AgentError::NoSecrets(_))));
        assert!(matches!(secrets_for(&src, &Settings::new(), SECURITY_GROUP, &[], 0), Err(AgentError::NoSecrets(_))));
    }

    #[test]
    fn a_refused_wifi_key_is_forgotten_so_the_user_is_asked_again() {
        let (src, keys) = sources();
        keys.set("u1", "home", &psk_key(), "wrong").unwrap();
        let err = secrets_for(&src, &profile("u1", "home", None), SECURITY_GROUP, &[], REQUEST_NEW | 0x1).unwrap_err();
        assert!(matches!(err, AgentError::NoSecrets(_)));
        assert_eq!(keys.get("u1", &psk_key()).unwrap(), None);
    }

    #[test]
    fn vpn_secrets_come_from_the_one_shot_store_first() {
        let (src, keys) = sources();
        let pw = SecretKey::new(VPN_GROUP, ovpn_key::PASSWORD);
        keys.set("u1", "work", &pw, "kept").unwrap();
        keys.set("u1", "work", &SecretKey::new(VPN_GROUP, ovpn_key::CERT_PASS), "pass").unwrap();
        let answer = secrets_for(&src, &profile("u1", "work", None), VPN_GROUP, &[], 0).unwrap();
        let secrets = settings::text_map(&answer, VPN_GROUP, "secrets");
        assert_eq!(secrets[ovpn_key::PASSWORD], "kept");
        assert_eq!(secrets[ovpn_key::CERT_PASS], "pass");
        src.once.put("u1", &pw, "typed");
        let answer = secrets_for(&src, &profile("u1", "work", None), VPN_GROUP, &[], 0).unwrap();
        assert_eq!(settings::text_map(&answer, VPN_GROUP, "secrets")[ovpn_key::PASSWORD], "typed");
    }

    #[test]
    fn a_refused_vpn_secret_is_kept_and_none_is_no_secrets() {
        let (src, keys) = sources();
        let pw = SecretKey::new(VPN_GROUP, ovpn_key::PASSWORD);
        assert!(matches!(secrets_for(&src, &profile("u1", "w", None), VPN_GROUP, &[], 0), Err(AgentError::NoSecrets(_))));
        keys.set("u1", "w", &pw, "x").unwrap();
        assert!(matches!(secrets_for(&src, &profile("u1", "w", None), VPN_GROUP, &[], REQUEST_NEW), Err(AgentError::NoSecrets(_))));
        assert_eq!(keys.get("u1", &pw).unwrap().as_deref(), Some("x"));
    }

    #[test]
    fn hinted_vpn_secrets_are_looked_up_and_messages_skipped() {
        let (src, keys) = sources();
        keys.set("u1", "w", &SecretKey::new(VPN_GROUP, "http-proxy-password"), "p").unwrap();
        let hints = vec!["http-proxy-password".to_string(), "x-vpn-message:Hello".to_string()];
        let answer = secrets_for(&src, &profile("u1", "w", None), VPN_GROUP, &hints, 0).unwrap();
        assert_eq!(settings::text_map(&answer, VPN_GROUP, "secrets").len(), 1);
    }

    #[test]
    fn wireguard_keys_are_answered_with_the_peer_keys() {
        let (src, keys) = sources();
        let mut conn = profile("u1", "wg", None);
        let peers: Vec<HashMap<String, Value>> = vec![
            HashMap::from([("public-key".to_string(), Value::from("PUB1"))]),
            HashMap::from([("public-key".to_string(), Value::from("PUB2"))]),
        ];
        settings::set(&mut conn, TYPE_WIREGUARD, "peers", peers);
        keys.set("u1", "wg", &SecretKey::new(TYPE_WIREGUARD, "private-key"), "PRIV").unwrap();
        keys.set("u1", "wg", &peer_secret_key("PUB2"), "PSK2").unwrap();
        let answer = secrets_for(&src, &conn, TYPE_WIREGUARD, &[], 0).unwrap();
        assert_eq!(settings::text(&answer, TYPE_WIREGUARD, "private-key").as_deref(), Some("PRIV"));
        let peers = settings::dicts(&answer, TYPE_WIREGUARD, "peers");
        assert_eq!(peers.len(), 1);
        assert_eq!(settings::as_text(&peers[0]["public-key"]).as_deref(), Some("PUB2"));
        assert_eq!(settings::as_text(&peers[0]["preshared-key"]).as_deref(), Some("PSK2"));
    }

    #[test]
    fn a_new_profile_has_its_secrets_kept_and_an_open_one_nothing() {
        let (src, keys) = sources();
        save_secrets(&*src.keys, &profile("u1", "home", Some("secret"))).unwrap();
        assert_eq!(keys.get("u1", &psk_key()).unwrap().as_deref(), Some("secret"));
        save_secrets(&*src.keys, &profile("u2", "cafe", None)).unwrap();
        assert_eq!(keys.get("u2", &psk_key()).unwrap(), None);
        let mut vpn = profile("u3", "work", None);
        settings::set(&mut vpn, VPN_GROUP, "secrets", HashMap::from([("password".to_string(), "pw".to_string())]));
        save_secrets(&*src.keys, &vpn).unwrap();
        assert_eq!(keys.get("u3", &SecretKey::new(VPN_GROUP, "password")).unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn a_removed_profile_loses_every_secret() {
        let (src, keys) = sources();
        keys.set("u1", "home", &psk_key(), "secret").unwrap();
        keys.set("u1", "home", &SecretKey::new(VPN_GROUP, "password"), "pw").unwrap();
        delete_secrets(&*src.keys, &profile("u1", "home", None)).unwrap();
        assert_eq!(keys.get("u1", &psk_key()).unwrap(), None);
        assert_eq!(keys.get("u1", &SecretKey::new(VPN_GROUP, "password")).unwrap(), None);
    }
}
