//! The `a{sa{sv}}` map of a NetworkManager connection profile, and small
//! helpers to read and write its values.

use std::collections::HashMap;
use zbus::zvariant::{OwnedValue, Value};

/// `a{sv}`: one setting (group) of a profile.
pub type Group = HashMap<String, OwnedValue>;
/// `a{sa{sv}}`: a connection profile.
pub type Settings = HashMap<String, Group>;

/// A plain value as an owned one. Only a file descriptor can fail to convert,
/// and profiles never hold one.
pub fn owned<'a>(value: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).expect("a plain D-Bus value always converts to an owned one")
}

/// The value at `group` / `key`, if any.
pub fn value<'s>(settings: &'s Settings, group: &str, key: &str) -> Option<&'s Value<'static>> {
    settings.get(group)?.get(key).map(|v| &**v)
}

/// The value inside a variant, or the value itself.
fn plain<'v>(value: &'v Value<'v>) -> &'v Value<'v> {
    match value {
        Value::Value(inner) => plain(inner),
        other => other,
    }
}

/// A text value.
pub fn text(settings: &Settings, group: &str, key: &str) -> Option<String> {
    value(settings, group, key).and_then(as_text)
}

/// A boolean value.
pub fn boolean(settings: &Settings, group: &str, key: &str) -> Option<bool> {
    match plain(value(settings, group, key)?) {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

/// An unsigned number.
pub fn uint(settings: &Settings, group: &str, key: &str) -> Option<u32> {
    value(settings, group, key).and_then(as_uint)
}

/// A signed number.
pub fn int(settings: &Settings, group: &str, key: &str) -> Option<i32> {
    match plain(value(settings, group, key)?) {
        Value::I32(n) => Some(*n),
        _ => None,
    }
}

/// A byte array (`ay`).
pub fn bytes(settings: &Settings, group: &str, key: &str) -> Option<Vec<u8>> {
    value(settings, group, key).and_then(as_bytes)
}

/// A list of texts (`as`).
pub fn texts(settings: &Settings, group: &str, key: &str) -> Vec<String> {
    match value(settings, group, key).map(plain) {
        Some(Value::Array(a)) => a.inner().iter().filter_map(as_text).collect(),
        _ => Vec::new(),
    }
}

/// A text dictionary (`a{ss}`), as the VPN `data` and `secrets`.
pub fn text_map(settings: &Settings, group: &str, key: &str) -> HashMap<String, String> {
    match value(settings, group, key).map(plain) {
        Some(Value::Dict(d)) => d.iter().filter_map(|(k, v)| Some((as_text(k)?, as_text(v)?))).collect(),
        _ => HashMap::new(),
    }
}

/// A list of dictionaries (`aa{sv}`), as `address-data` or the WireGuard peers.
pub fn dicts(settings: &Settings, group: &str, key: &str) -> Vec<HashMap<String, Value<'static>>> {
    let Some(Value::Array(a)) = value(settings, group, key).map(plain) else { return Vec::new() };
    a.inner().iter().filter_map(as_dict).collect()
}

/// A list of `au` numbers, as the legacy IPv4 `dns`.
pub fn uints(settings: &Settings, group: &str, key: &str) -> Vec<u32> {
    match value(settings, group, key).map(plain) {
        Some(Value::Array(a)) => a.inner().iter().filter_map(as_uint).collect(),
        _ => Vec::new(),
    }
}

/// A list of byte arrays (`aay`), as the legacy IPv6 `dns`.
pub fn byte_lists(settings: &Settings, group: &str, key: &str) -> Vec<Vec<u8>> {
    match value(settings, group, key).map(plain) {
        Some(Value::Array(a)) => a.inner().iter().filter_map(as_bytes).collect(),
        _ => Vec::new(),
    }
}

/// A value as text.
pub fn as_text(value: &Value<'_>) -> Option<String> {
    match plain(value) {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// A value as an unsigned number.
pub fn as_uint(value: &Value<'_>) -> Option<u32> {
    match plain(value) {
        Value::U32(n) => Some(*n),
        _ => None,
    }
}

/// A value as a byte array.
fn as_bytes(value: &Value<'_>) -> Option<Vec<u8>> {
    let Value::Array(a) = plain(value) else { return None };
    a.inner()
        .iter()
        .map(|b| match b {
            Value::U8(b) => Some(*b),
            _ => None,
        })
        .collect()
}

/// A value as a list of texts.
pub fn as_texts(value: &Value<'_>) -> Vec<String> {
    match plain(value) {
        Value::Array(a) => a.inner().iter().filter_map(as_text).collect(),
        _ => Vec::new(),
    }
}

/// A value as an `a{sv}` dictionary.
fn as_dict(value: &Value<'_>) -> Option<HashMap<String, Value<'static>>> {
    let Value::Dict(d) = plain(value) else { return None };
    let mut out = HashMap::new();
    for (k, v) in d.iter() {
        let v = plain(v).try_clone().ok()?;
        out.insert(as_text(k)?, v.try_into_owned().ok()?.into());
    }
    Some(out)
}

/// Set `group` / `key`, creating the group when needed.
pub fn set<'a>(settings: &mut Settings, group: &str, key: &str, value: impl Into<Value<'a>>) {
    settings.entry(group.to_string()).or_default().insert(key.to_string(), owned(value));
}

/// Remove `group` / `key`; a missing one is not an error.
pub fn remove(settings: &mut Settings, group: &str, key: &str) {
    if let Some(g) = settings.get_mut(group) {
        g.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_written_are_read_back() {
        let mut s = Settings::new();
        set(&mut s, "connection", "id", "Home");
        set(&mut s, "connection", "autoconnect", false);
        set(&mut s, "connection", "autoconnect-priority", -5i32);
        set(&mut s, "ipv4", "dns", vec![1u32, 2]);
        set(&mut s, "ipv6", "dns", vec![vec![1u8, 2], vec![3u8]]);
        set(&mut s, "wifi", "ssid", b"net".to_vec());
        set(&mut s, "connection", "secondaries", vec!["a".to_string()]);
        set(&mut s, "vpn", "data", HashMap::from([("remote".to_string(), "host".to_string())]));
        assert_eq!(text(&s, "connection", "id").as_deref(), Some("Home"));
        assert_eq!(boolean(&s, "connection", "autoconnect"), Some(false));
        assert_eq!(int(&s, "connection", "autoconnect-priority"), Some(-5));
        assert_eq!(uints(&s, "ipv4", "dns"), vec![1, 2]);
        assert_eq!(byte_lists(&s, "ipv6", "dns"), vec![vec![1, 2], vec![3]]);
        assert_eq!(bytes(&s, "wifi", "ssid"), Some(b"net".to_vec()));
        assert_eq!(texts(&s, "connection", "secondaries"), vec!["a".to_string()]);
        assert_eq!(text_map(&s, "vpn", "data")["remote"], "host");
    }

    #[test]
    fn a_list_of_dictionaries_is_read_back() {
        let mut s = Settings::new();
        let entry: HashMap<String, Value> = HashMap::from([("address".to_string(), Value::from("10.0.0.2")), ("prefix".to_string(), Value::from(24u32))]);
        set(&mut s, "ipv4", "address-data", vec![entry]);
        let read = dicts(&s, "ipv4", "address-data");
        assert_eq!(read.len(), 1);
        assert_eq!(as_text(&read[0]["address"]).as_deref(), Some("10.0.0.2"));
        assert_eq!(as_uint(&read[0]["prefix"]), Some(24));
    }

    #[test]
    fn wrong_types_and_missing_keys_read_as_nothing() {
        let mut s = Settings::new();
        set(&mut s, "connection", "id", "Home");
        assert_eq!(uint(&s, "connection", "id"), None);
        assert_eq!(text(&s, "connection", "missing"), None);
        assert!(texts(&s, "missing", "x").is_empty());
        remove(&mut s, "connection", "id");
        remove(&mut s, "missing", "id");
        assert_eq!(text(&s, "connection", "id"), None);
    }
}
