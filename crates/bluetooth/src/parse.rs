//! Turn the answer of `org.freedesktop.DBus.ObjectManager.GetManagedObjects`
//! of BlueZ into a [`BtState`].

use crate::model::{Adapter, BtState, Device, DeviceKind};
use crate::rfkill::Rfkill;
use std::collections::HashMap;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// The properties of one interface.
pub type Props = HashMap<String, OwnedValue>;
/// Every object of BlueZ with its interfaces and their properties.
pub type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

pub const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
pub const DEVICE_IFACE: &str = "org.bluez.Device1";
pub const BATTERY_IFACE: &str = "org.bluez.Battery1";

/// The state of the first adapter (by path) and of its devices.
pub fn state(objects: &ManagedObjects, rfkill: Rfkill) -> BtState {
    let mut paths: Vec<&OwnedObjectPath> = objects.keys().collect();
    paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let adapter = paths.iter().find_map(|p| objects[*p].get(ADAPTER_IFACE).map(|props| adapter_from(p.as_str(), props)));
    let Some(adapter) = adapter else { return BtState { adapter: None, devices: Vec::new(), rfkill } };
    let devices = paths
        .iter()
        .filter_map(|p| {
            let ifaces = &objects[*p];
            let props = ifaces.get(DEVICE_IFACE)?;
            if string(props, "Adapter") != adapter.path { return None; }
            let mut d = device_from(p.as_str(), props);
            d.battery = ifaces.get(BATTERY_IFACE).and_then(|b| b.get("Percentage")).and_then(|v| u8::try_from(v).ok());
            Some(d)
        })
        .collect();
    BtState { adapter: Some(adapter), devices, rfkill }
}

/// The adapter at `path`.
fn adapter_from(path: &str, props: &Props) -> Adapter {
    let alias = string(props, "Alias");
    Adapter {
        path: path.to_string(),
        address: string(props, "Address"),
        alias: if alias.is_empty() { string(props, "Name") } else { alias },
        powered: boolean(props, "Powered"),
        discoverable: boolean(props, "Discoverable"),
        pairable: boolean(props, "Pairable"),
        discovering: boolean(props, "Discovering"),
    }
}

/// The device at `path`, without its battery.
fn device_from(path: &str, props: &Props) -> Device {
    let address = string(props, "Address");
    let name = [string(props, "Alias"), string(props, "Name")].into_iter().find(|n| !n.is_empty()).unwrap_or_else(|| address.clone());
    Device {
        path: path.to_string(),
        address,
        name,
        kind: DeviceKind::from_icon(&string(props, "Icon")),
        paired: boolean(props, "Paired"),
        trusted: boolean(props, "Trusted"),
        blocked: boolean(props, "Blocked"),
        connected: boolean(props, "Connected"),
        battery: None,
    }
}

/// A string or object path property, empty when missing.
fn string(props: &Props, key: &str) -> String {
    match props.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => s.to_string(),
        Some(Value::ObjectPath(p)) => p.to_string(),
        _ => String::new(),
    }
}

/// A boolean property, false when missing.
fn boolean(props: &Props, key: &str) -> bool {
    props.get(key).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use zbus::zvariant::ObjectPath;

    /// Properties from pairs.
    pub fn props(pairs: Vec<(&str, Value)>) -> Props {
        pairs.into_iter().map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap())).collect()
    }

    /// An object path value.
    fn path(p: &'static str) -> Value<'static> {
        Value::from(ObjectPath::try_from(p).unwrap())
    }

    /// One adapter `hci0` with a paired keyboard (battery 80) and a found
    /// phone, plus a device of `hci1`.
    pub fn objects() -> ManagedObjects {
        let mut o = ManagedObjects::new();
        let mut add = |p: &'static str, ifaces: Vec<(&str, Props)>| {
            o.insert(OwnedObjectPath::try_from(p).unwrap(), ifaces.into_iter().map(|(i, pr)| (i.to_string(), pr)).collect());
        };
        add("/org/bluez/hci0", vec![(ADAPTER_IFACE, props(vec![("Address", Value::from("AA")), ("Alias", Value::from("office")), ("Powered", Value::from(true))]))]);
        add("/org/bluez/hci1", vec![(ADAPTER_IFACE, props(vec![("Address", Value::from("BB"))]))]);
        add(
            "/org/bluez/hci0/dev_01",
            vec![
                (DEVICE_IFACE, props(vec![("Adapter", path("/org/bluez/hci0")), ("Address", Value::from("01")), ("Alias", Value::from("Keys")), ("Icon", Value::from("input-keyboard")), ("Paired", Value::from(true)), ("Connected", Value::from(true))])),
                (BATTERY_IFACE, props(vec![("Percentage", Value::from(80u8))])),
            ],
        );
        add("/org/bluez/hci0/dev_02", vec![(DEVICE_IFACE, props(vec![("Adapter", path("/org/bluez/hci0")), ("Address", Value::from("02")), ("Name", Value::from("Pixel"))]))]);
        add("/org/bluez/hci0/dev_03", vec![(DEVICE_IFACE, props(vec![("Adapter", path("/org/bluez/hci0")), ("Address", Value::from("03"))]))]);
        add("/org/bluez/hci1/dev_09", vec![(DEVICE_IFACE, props(vec![("Adapter", path("/org/bluez/hci1")), ("Address", Value::from("09"))]))]);
        o
    }

    #[test]
    fn parse_reads_adapter_devices_and_battery() {
        let s = state(&objects(), Rfkill::default());
        let a = s.adapter.clone().unwrap();
        assert_eq!((a.path.as_str(), a.alias.as_str(), a.powered), ("/org/bluez/hci0", "office", true));
        let k = s.device("/org/bluez/hci0/dev_01").unwrap();
        assert_eq!((k.name.as_str(), k.kind, k.paired, k.connected, k.battery), ("Keys", DeviceKind::Keyboard, true, true, Some(80)));
    }

    #[test]
    fn device_name_falls_back_to_name_then_address() {
        let s = state(&objects(), Rfkill::default());
        assert_eq!(s.device_name("/org/bluez/hci0/dev_02"), "Pixel");
        assert_eq!(s.device_name("/org/bluez/hci0/dev_03"), "03");
    }

    #[test]
    fn devices_of_other_adapters_are_skipped() {
        let s = state(&objects(), Rfkill::default());
        assert_eq!(s.devices.len(), 3);
        assert!(s.device("/org/bluez/hci1/dev_09").is_none());
    }

    #[test]
    fn no_adapter_means_no_devices() {
        let s = state(&ManagedObjects::new(), Rfkill { soft: true, hard: false });
        assert!(s.adapter.is_none() && s.devices.is_empty() && s.rfkill.soft);
    }
}
