//! Where the Wi-Fi keys are kept: the desktop keyring (Secret Service), with
//! the same attributes `nm-applet` and GNOME use, so the other tools find them.

use crate::error::NetError;
use secret_service::blocking::SecretService;
use secret_service::EncryptionType;
use std::collections::HashMap;
use std::sync::Mutex;

/// The alias of the keyring that is encrypted and opened by the login. The
/// default keyring may be a plain text file, which is no place for a Wi-Fi key.
const LOGIN_ALIAS: &str = "login";
/// The schema `nm-applet` and GNOME give to the secrets of a connection.
const SCHEMA: &str = "org.freedesktop.NetworkManager.Connection";
/// The settings group that holds the Wi-Fi key.
pub const SECURITY_GROUP: &str = "802-11-wireless-security";
/// The name of the Wi-Fi key inside that group.
pub const PSK_KEY: &str = "psk";

/// Keys by the UUID of their connection profile.
pub trait KeyStore: Send + Sync {
    /// The key of the profile `uuid`, `None` when none is kept.
    fn get(&self, uuid: &str) -> Result<Option<String>, NetError>;
    /// Keep the key of the profile `uuid`; `name` is only the label in the keyring.
    fn set(&self, uuid: &str, name: &str, key: &str) -> Result<(), NetError>;
    /// Forget the key of the profile `uuid`; no key is not an error.
    fn delete(&self, uuid: &str) -> Result<(), NetError>;
}

/// The attributes that find the key of a profile.
fn attributes(uuid: &str) -> HashMap<&str, &str> {
    HashMap::from([("xdg:schema", SCHEMA), ("connection-uuid", uuid), ("setting-name", SECURITY_GROUP), ("setting-key", PSK_KEY)])
}

/// The label of a key in the keyring, like the one `nm-applet` writes.
fn label(name: &str) -> String {
    format!("Network secret for {name}/{SECURITY_GROUP}/{PSK_KEY}")
}

/// A keyring error as a readable one line error.
fn keyring_error(e: secret_service::Error) -> NetError {
    NetError::other(format!("Keyring: {e}"))
}

/// The Secret Service on the session bus. Keys are written to the login
/// keyring and looked up in all of them. Connects at every call: the calls
/// are rare and this way a keyring that starts later or restarts just works.
pub struct SecretServiceStore;

impl KeyStore for SecretServiceStore {
    fn get(&self, uuid: &str) -> Result<Option<String>, NetError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(keyring_error)?;
        let found = ss.search_items(attributes(uuid)).map_err(keyring_error)?;
        // A locked item is asked to unlock: the keyring shows its own prompt.
        let Some(item) = found.unlocked.into_iter().chain(found.locked).next() else { return Ok(None) };
        item.unlock().map_err(keyring_error)?;
        let secret = item.get_secret().map_err(keyring_error)?;
        String::from_utf8(secret).map(Some).map_err(|_| NetError::other("Keyring: the key is not text"))
    }

    fn set(&self, uuid: &str, name: &str, key: &str) -> Result<(), NetError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(keyring_error)?;
        let collection = ss.get_collection_by_alias(LOGIN_ALIAS).map_err(keyring_error)?;
        collection.unlock().map_err(keyring_error)?;
        collection.create_item(&label(name), attributes(uuid), key.as_bytes(), true, "text/plain").map_err(keyring_error)?;
        Ok(())
    }

    fn delete(&self, uuid: &str) -> Result<(), NetError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(keyring_error)?;
        let found = ss.search_items(attributes(uuid)).map_err(keyring_error)?;
        for item in found.unlocked.into_iter().chain(found.locked) {
            item.delete().map_err(keyring_error)?;
        }
        Ok(())
    }
}

/// A store in memory, for tests. Never shows a key in `Debug`.
#[derive(Default)]
pub struct FakeKeyStore {
    keys: Mutex<HashMap<String, String>>,
}

impl KeyStore for FakeKeyStore {
    fn get(&self, uuid: &str) -> Result<Option<String>, NetError> {
        Ok(self.keys.lock().unwrap_or_else(|e| e.into_inner()).get(uuid).cloned())
    }

    fn set(&self, uuid: &str, _name: &str, key: &str) -> Result<(), NetError> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).insert(uuid.to_string(), key.to_string());
        Ok(())
    }

    fn delete(&self, uuid: &str) -> Result<(), NetError> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).remove(uuid);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attributes_are_the_ones_networkmanager_tools_use() {
        let a = attributes("abc");
        assert_eq!(a["xdg:schema"], "org.freedesktop.NetworkManager.Connection");
        assert_eq!(a["connection-uuid"], "abc");
        assert_eq!(a["setting-name"], "802-11-wireless-security");
        assert_eq!(a["setting-key"], "psk");
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn the_label_names_the_network() {
        assert_eq!(label("home"), "Network secret for home/802-11-wireless-security/psk");
    }

    #[test]
    fn the_fake_keeps_replaces_and_forgets() {
        let store = FakeKeyStore::default();
        assert_eq!(store.get("u").unwrap(), None);
        store.set("u", "home", "one").unwrap();
        store.set("u", "home", "two").unwrap();
        assert_eq!(store.get("u").unwrap().as_deref(), Some("two"));
        store.delete("u").unwrap();
        store.delete("u").unwrap();
        assert_eq!(store.get("u").unwrap(), None);
    }
}
