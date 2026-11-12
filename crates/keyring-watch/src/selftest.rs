//! The Test action: write an item to the keyring, read it back, compare and
//! delete it, to show that the Secret Service really works.

use secret_service::blocking::SecretService;
use secret_service::EncryptionType;
use std::collections::HashMap;

/// The alias of the keyring apps use.
const DEFAULT_ALIAS: &str = "default";
/// The label of the probe item.
const ITEM_LABEL: &str = "opusdm-keyring-watch test";
/// The schema attribute of the probe item.
const SCHEMA: &str = "org.opusdm.KeyringWatchTest";
/// The attribute that holds the id of the probe item.
const ID_ATTRIBUTE: &str = "test-id";
/// How many random bytes a value or an id has.
const RANDOM_BYTES: usize = 16;

/// Where the test item is kept.
pub trait ItemStore {
    /// Keep `value` in a new item and return the id to find it again.
    fn create(&self, value: &str) -> Result<String, String>;
    /// The value of the item `id`.
    fn read(&self, id: &str) -> Result<String, String>;
    /// Remove the item `id`.
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// Create, read, compare and delete one item. After a failure that follows
/// the create, the delete is still tried, and its failure is added to the
/// error, so the item is never left behind silently.
pub fn run(store: &dyn ItemStore, value: &str) -> Result<(), String> {
    let id = store.create(value)?;
    let checked = store.read(&id).and_then(|got| if got == value { Ok(()) } else { Err("the value read back is not the one written".to_string()) });
    let deleted = store.delete(&id);
    match (checked, deleted) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(d)) => Err(format!("the test item could not be deleted: {d}")),
        (Err(c), Ok(())) => Err(c),
        (Err(c), Err(d)) => Err(format!("{c}; and the test item could not be deleted: {d}")),
    }
}

/// A random lowercase hex string of [`RANDOM_BYTES`] bytes.
pub fn random_value() -> String {
    let mut bytes = [0u8; RANDOM_BYTES];
    if let Err(e) = getrandom::getrandom(&mut bytes) {
        // Not secret material, but never a constant: fall back to the clock.
        opusdm_ipc::err_log!("opusdm-keyring-watch", "no random bytes ({e}), using the clock");
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        bytes.copy_from_slice(&nanos.to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The Secret Service store: the collection behind the `default` alias.
pub struct SecretServiceStore;

impl SecretServiceStore {
    /// The attributes that find the item `id`.
    fn _attributes(id: &str) -> HashMap<&str, &str> {
        HashMap::from([("xdg:schema", SCHEMA), (ID_ATTRIBUTE, id)])
    }
}

/// A keyring error as one line.
fn _keyring_error(e: secret_service::Error) -> String {
    format!("Keyring: {e}")
}

impl ItemStore for SecretServiceStore {
    /// See [`ItemStore::create`]; the id is a random string kept as an attribute.
    fn create(&self, value: &str) -> Result<String, String> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(_keyring_error)?;
        let collection = ss.get_collection_by_alias(DEFAULT_ALIAS).map_err(_keyring_error)?;
        collection.unlock().map_err(_keyring_error)?;
        let id = random_value();
        collection.create_item(ITEM_LABEL, Self::_attributes(&id), value.as_bytes(), true, "text/plain").map_err(_keyring_error)?;
        Ok(id)
    }

    /// See [`ItemStore::read`].
    fn read(&self, id: &str) -> Result<String, String> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(_keyring_error)?;
        let collection = ss.get_collection_by_alias(DEFAULT_ALIAS).map_err(_keyring_error)?;
        let items = collection.search_items(Self::_attributes(id)).map_err(_keyring_error)?;
        let item = items.first().ok_or("the test item was not found after it was written")?;
        let secret = item.get_secret().map_err(_keyring_error)?;
        String::from_utf8(secret).map_err(|e| format!("the test item is not text: {e}"))
    }

    /// See [`ItemStore::delete`]; no item left is not an error.
    fn delete(&self, id: &str) -> Result<(), String> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(_keyring_error)?;
        let collection = ss.get_collection_by_alias(DEFAULT_ALIAS).map_err(_keyring_error)?;
        for item in collection.search_items(Self::_attributes(id)).map_err(_keyring_error)? {
            item.delete().map_err(_keyring_error)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A store that keeps one item in memory and fails where told to.
    #[derive(Default)]
    struct FakeStore {
        item: RefCell<Option<String>>,
        corrupt: bool,
        read_fails: bool,
        delete_fails: bool,
    }

    impl ItemStore for FakeStore {
        fn create(&self, value: &str) -> Result<String, String> {
            let kept = if self.corrupt { "other".to_string() } else { value.to_string() };
            *self.item.borrow_mut() = Some(kept);
            Ok("id".into())
        }
        fn read(&self, _id: &str) -> Result<String, String> {
            if self.read_fails { return Err("read failed".into()); }
            self.item.borrow().clone().ok_or_else(|| "gone".to_string())
        }
        fn delete(&self, _id: &str) -> Result<(), String> {
            if self.delete_fails { return Err("delete failed".into()); }
            *self.item.borrow_mut() = None;
            Ok(())
        }
    }

    #[test]
    fn ok_creates_reads_and_deletes() {
        let store = FakeStore::default();
        assert_eq!(run(&store, "v"), Ok(()));
        assert!(store.item.borrow().is_none());
    }

    #[test]
    fn mismatch_still_deletes() {
        let store = FakeStore { corrupt: true, ..Default::default() };
        let err = run(&store, "v").unwrap_err();
        assert!(err.contains("not the one written"), "{err}");
        assert!(store.item.borrow().is_none());
    }

    #[test]
    fn read_error_still_deletes() {
        let store = FakeStore { read_fails: true, ..Default::default() };
        assert_eq!(run(&store, "v"), Err("read failed".to_string()));
        assert!(store.item.borrow().is_none());
    }

    #[test]
    fn delete_error_is_reported() {
        let store = FakeStore { delete_fails: true, ..Default::default() };
        let err = run(&store, "v").unwrap_err();
        assert!(err.contains("delete failed"), "{err}");
    }

    #[test]
    fn read_and_delete_errors_are_both_reported() {
        let store = FakeStore { read_fails: true, delete_fails: true, ..Default::default() };
        let err = run(&store, "v").unwrap_err();
        assert!(err.contains("read failed") && err.contains("delete failed"), "{err}");
    }

    #[test]
    fn random_values_differ_and_are_hex() {
        let (a, b) = (random_value(), random_value());
        assert_ne!(a, b);
        assert_eq!(a.len(), RANDOM_BYTES * 2);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
