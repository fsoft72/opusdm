//! Where the secrets of the profiles are kept: the desktop keyring (Secret
//! Service), with the same attributes `nm-applet` and GNOME use, so the other
//! tools find them. A secret is found by the profile UUID, the setting and
//! the key inside it.

use crate::error::NetError;
use crate::profile::SecretKey;
use secret_service::blocking::SecretService;
use secret_service::EncryptionType;
use std::collections::HashMap;
use std::sync::Mutex;

/// The alias of the keyring that is encrypted and opened by the login. The
/// default keyring may be a plain text file, which is no place for a key.
const LOGIN_ALIAS: &str = "login";
/// The schema `nm-applet` and GNOME give to the secrets of a connection.
const SCHEMA: &str = "org.freedesktop.NetworkManager.Connection";
/// The settings group that holds the Wi-Fi key.
pub const SECURITY_GROUP: &str = "802-11-wireless-security";
/// The name of the Wi-Fi key inside that group.
pub const PSK_KEY: &str = "psk";

/// The keyring key of the Wi-Fi password.
pub fn psk_key() -> SecretKey {
    SecretKey::new(SECURITY_GROUP, PSK_KEY)
}

/// Secrets by profile UUID and key.
pub trait KeyStore: Send + Sync {
    /// The secret `key` of the profile `uuid`, `None` when none is kept.
    fn get(&self, uuid: &str, key: &SecretKey) -> Result<Option<String>, NetError>;
    /// Keep a secret of the profile `uuid`; `name` is only the label in the keyring.
    fn set(&self, uuid: &str, name: &str, key: &SecretKey, value: &str) -> Result<(), NetError>;
    /// Forget one secret; no secret is not an error.
    fn delete(&self, uuid: &str, key: &SecretKey) -> Result<(), NetError>;
    /// Forget every secret of the profile `uuid`.
    fn delete_all(&self, uuid: &str) -> Result<(), NetError>;
}

/// The attributes that find one secret of a profile.
fn attributes<'a>(uuid: &'a str, key: &'a SecretKey) -> HashMap<&'a str, &'a str> {
    HashMap::from([("xdg:schema", SCHEMA), ("connection-uuid", uuid), ("setting-name", key.setting.as_str()), ("setting-key", key.key.as_str())])
}

/// The attributes that find every secret of a profile.
fn profile_attributes(uuid: &str) -> HashMap<&str, &str> {
    HashMap::from([("xdg:schema", SCHEMA), ("connection-uuid", uuid)])
}

/// The label of a secret in the keyring, like the one `nm-applet` writes.
fn label(name: &str, key: &SecretKey) -> String {
    format!("Network secret for {name}/{}/{}", key.setting, key.key)
}

/// A keyring error as a readable one line error.
fn keyring_error(e: secret_service::Error) -> NetError {
    NetError::other(format!("Keyring: {e}"))
}

/// The Secret Service on the session bus. Secrets are written to the login
/// keyring and looked up in all of them. Connects at every call: the calls
/// are rare and this way a keyring that starts later or restarts just works.
pub struct SecretServiceStore;

impl SecretServiceStore {
    /// Delete every item that matches `attrs`.
    fn delete_matching(attrs: HashMap<&str, &str>) -> Result<(), NetError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(keyring_error)?;
        let found = ss.search_items(attrs).map_err(keyring_error)?;
        for item in found.unlocked.into_iter().chain(found.locked) {
            item.delete().map_err(keyring_error)?;
        }
        Ok(())
    }
}

impl KeyStore for SecretServiceStore {
    fn get(&self, uuid: &str, key: &SecretKey) -> Result<Option<String>, NetError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(keyring_error)?;
        let found = ss.search_items(attributes(uuid, key)).map_err(keyring_error)?;
        // A locked item is asked to unlock: the keyring shows its own prompt.
        let Some(item) = found.unlocked.into_iter().chain(found.locked).next() else { return Ok(None) };
        item.unlock().map_err(keyring_error)?;
        let secret = item.get_secret().map_err(keyring_error)?;
        String::from_utf8(secret).map(Some).map_err(|_| NetError::other("Keyring: the secret is not text"))
    }

    fn set(&self, uuid: &str, name: &str, key: &SecretKey, value: &str) -> Result<(), NetError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(keyring_error)?;
        let collection = ss.get_collection_by_alias(LOGIN_ALIAS).map_err(keyring_error)?;
        collection.unlock().map_err(keyring_error)?;
        collection.create_item(&label(name, key), attributes(uuid, key), value.as_bytes(), true, "text/plain").map_err(keyring_error)?;
        Ok(())
    }

    fn delete(&self, uuid: &str, key: &SecretKey) -> Result<(), NetError> {
        Self::delete_matching(attributes(uuid, key))
    }

    fn delete_all(&self, uuid: &str) -> Result<(), NetError> {
        Self::delete_matching(profile_attributes(uuid))
    }
}

/// A store in memory, for tests. Never shows a secret in `Debug`.
#[derive(Default)]
pub struct FakeKeyStore {
    keys: Mutex<HashMap<(String, SecretKey), String>>,
}

impl FakeKeyStore {
    fn keys(&self) -> std::sync::MutexGuard<'_, HashMap<(String, SecretKey), String>> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl KeyStore for FakeKeyStore {
    fn get(&self, uuid: &str, key: &SecretKey) -> Result<Option<String>, NetError> {
        Ok(self.keys().get(&(uuid.to_string(), key.clone())).cloned())
    }

    fn set(&self, uuid: &str, _name: &str, key: &SecretKey, value: &str) -> Result<(), NetError> {
        self.keys().insert((uuid.to_string(), key.clone()), value.to_string());
        Ok(())
    }

    fn delete(&self, uuid: &str, key: &SecretKey) -> Result<(), NetError> {
        self.keys().remove(&(uuid.to_string(), key.clone()));
        Ok(())
    }

    fn delete_all(&self, uuid: &str) -> Result<(), NetError> {
        self.keys().retain(|(u, _), _| u != uuid);
        Ok(())
    }
}

/// Secrets given for one connect only (a VPN password asked every time).
/// The secret agent looks here before the keyring.
#[derive(Default)]
pub struct OneShotSecrets {
    secrets: Mutex<HashMap<(String, SecretKey), String>>,
}

impl OneShotSecrets {
    fn secrets(&self) -> std::sync::MutexGuard<'_, HashMap<(String, SecretKey), String>> {
        self.secrets.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hold a secret of the profile `uuid` until `clear`.
    pub fn put(&self, uuid: &str, key: &SecretKey, value: &str) {
        self.secrets().insert((uuid.to_string(), key.clone()), value.to_string());
    }

    /// The held secret, if any.
    pub fn get(&self, uuid: &str, key: &SecretKey) -> Option<String> {
        self.secrets().get(&(uuid.to_string(), key.clone())).cloned()
    }

    /// Forget the secrets of the profile `uuid`.
    pub fn clear(&self, uuid: &str) {
        self.secrets().retain(|(u, _), _| u != uuid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attributes_are_the_ones_networkmanager_tools_use() {
        let key = psk_key();
        let a = attributes("abc", &key);
        assert_eq!(a["xdg:schema"], "org.freedesktop.NetworkManager.Connection");
        assert_eq!(a["connection-uuid"], "abc");
        assert_eq!(a["setting-name"], "802-11-wireless-security");
        assert_eq!(a["setting-key"], "psk");
        assert_eq!(a.len(), 4);
        assert_eq!(profile_attributes("abc").len(), 2);
    }

    #[test]
    fn the_label_names_the_network_and_the_key() {
        assert_eq!(label("home", &psk_key()), "Network secret for home/802-11-wireless-security/psk");
        assert_eq!(label("work", &SecretKey::new("vpn", "password")), "Network secret for work/vpn/password");
    }

    #[test]
    fn the_fake_keeps_replaces_and_forgets() {
        let store = FakeKeyStore::default();
        let pw = SecretKey::new("vpn", "password");
        assert_eq!(store.get("u", &psk_key()).unwrap(), None);
        store.set("u", "home", &psk_key(), "one").unwrap();
        store.set("u", "home", &psk_key(), "two").unwrap();
        store.set("u", "home", &pw, "three").unwrap();
        store.set("v", "other", &pw, "four").unwrap();
        assert_eq!(store.get("u", &psk_key()).unwrap().as_deref(), Some("two"));
        store.delete("u", &psk_key()).unwrap();
        store.delete("u", &psk_key()).unwrap();
        assert_eq!(store.get("u", &psk_key()).unwrap(), None);
        assert_eq!(store.get("u", &pw).unwrap().as_deref(), Some("three"));
        store.delete_all("u").unwrap();
        assert_eq!(store.get("u", &pw).unwrap(), None);
        assert_eq!(store.get("v", &pw).unwrap().as_deref(), Some("four"));
    }

    #[test]
    fn one_shot_secrets_are_cleared_per_profile() {
        let once = OneShotSecrets::default();
        let pw = SecretKey::new("vpn", "password");
        once.put("u", &pw, "a");
        once.put("v", &pw, "b");
        assert_eq!(once.get("u", &pw).as_deref(), Some("a"));
        once.clear("u");
        assert_eq!(once.get("u", &pw), None);
        assert_eq!(once.get("v", &pw).as_deref(), Some("b"));
    }
}
