//! Turns the udisks2 `GetManagedObjects` reply into [`Device`]s.

use crate::filter::{is_listed, RawBlock, RawDrive};
use crate::model::{human_size, Device, Kind};
use std::collections::HashMap;
use std::path::PathBuf;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// `a{oa{sa{sv}}}`: object path, interface name, property name, value.
pub type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

const BLOCK_IFACE: &str = "org.freedesktop.UDisks2.Block";
const FS_IFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE_IFACE: &str = "org.freedesktop.UDisks2.Drive";
const BLOCK_PREFIX: &str = "/org/freedesktop/UDisks2/block_devices/";

type Props = HashMap<String, OwnedValue>;

/// A byte array that ends with a NUL, as a string.
fn bytes_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Property `key` as a string (empty when missing or of another type).
fn get_str(props: &Props, key: &str) -> String {
    let Some(v) = props.get(key) else { return String::new() };
    if let Ok(s) = String::try_from(v.clone()) { return s; }
    OwnedObjectPath::try_from(v.clone()).map(|p| p.to_string()).unwrap_or_default()
}

/// Property `key` as a bool (false when missing).
fn get_bool(props: &Props, key: &str) -> bool {
    props.get(key).and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(false)
}

/// Property `key` as a u64 (0 when missing).
fn get_u64(props: &Props, key: &str) -> u64 {
    props.get(key).and_then(|v| u64::try_from(v.clone()).ok()).unwrap_or(0)
}

/// Property `key` as a NUL terminated byte string.
fn get_bytes(props: &Props, key: &str) -> String {
    let Some(v) = props.get(key) else { return String::new() };
    Vec::<u8>::try_from(v.clone()).map(|b| bytes_to_string(&b)).unwrap_or_default()
}

/// Property `key` as a list of NUL terminated byte strings.
fn get_byte_list(props: &Props, key: &str) -> Vec<String> {
    let Some(v) = props.get(key) else { return Vec::new() };
    Vec::<Vec<u8>>::try_from(v.clone())
        .map(|l| l.iter().map(|b| bytes_to_string(b)).collect())
        .unwrap_or_default()
}

/// The drive at `path`, when the tree has it.
fn raw_drive(objects: &ManagedObjects, path: &str) -> Option<RawDrive> {
    if path == "/" { return None; }
    let (p, ifaces) = objects.iter().find(|(p, _)| p.as_str() == path)?;
    let d = ifaces.get(DRIVE_IFACE)?;
    Some(RawDrive {
        path: p.to_string(),
        vendor: get_str(d, "Vendor").trim().to_string(),
        model: get_str(d, "Model").trim().to_string(),
        bus: get_str(d, "ConnectionBus"),
        removable: get_bool(d, "Removable"),
        ejectable: get_bool(d, "Ejectable"),
        can_power_off: get_bool(d, "CanPowerOff"),
        media: get_str(d, "Media"),
    })
}

/// Every block device of the tree with the facts the filter needs.
fn raw_blocks(objects: &ManagedObjects) -> Vec<RawBlock> {
    objects
        .iter()
        .filter(|(p, _)| p.as_str().starts_with(BLOCK_PREFIX))
        .filter_map(|(p, ifaces)| {
            let b = ifaces.get(BLOCK_IFACE)?;
            let mount_points = ifaces.get(FS_IFACE).map(|fs| get_byte_list(fs, "MountPoints"));
            Some(RawBlock {
                path: p.to_string(),
                dev_node: get_bytes(b, "Device"),
                id_label: get_str(b, "IdLabel"),
                id_type: get_str(b, "IdType"),
                size: get_u64(b, "Size"),
                hint_system: get_bool(b, "HintSystem"),
                hint_ignore: get_bool(b, "HintIgnore"),
                mount_points,
                drive: raw_drive(objects, &get_str(b, "Drive")),
            })
        })
        .collect()
}

/// Name for a volume: its label, else the drive name, else its size.
fn display_label(block: &RawBlock, drive_name: &str) -> String {
    if !block.id_label.is_empty() { return block.id_label.clone(); }
    if !drive_name.is_empty() { return drive_name.to_string(); }
    format!("{} Volume", human_size(block.size))
}

/// Convert one listed block device into a [`Device`].
fn to_device(block: RawBlock) -> Device {
    let drive = block.drive.clone().unwrap_or_default();
    let drive_name = format!("{} {}", drive.vendor, drive.model).trim().to_string();
    let kind = if drive.bus == "usb" {
        Kind::Usb
    } else if drive.media.starts_with("flash_sd") || drive.media.starts_with("flash_mmc") {
        Kind::Card
    } else {
        Kind::Disk
    };
    Device {
        label: display_label(&block, &drive_name),
        mount_point: block.mount_points.as_ref().and_then(|m| m.first()).map(PathBuf::from),
        path: block.path,
        drive_path: drive.path,
        size: block.size,
        dev_node: block.dev_node,
        fs_type: block.id_type,
        kind,
        can_eject: drive.ejectable,
        can_power_off: drive.can_power_off,
        drive_name,
    }
}

/// Every volume of the tree that belongs in the notifier, sorted by device node.
pub fn devices(objects: &ManagedObjects) -> Vec<Device> {
    let mut out: Vec<Device> = raw_blocks(objects).into_iter().filter(is_listed).map(to_device).collect();
    out.sort_by(|a, b| a.dev_node.cmp(&b.dev_node));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn ov<T: Into<Value<'static>>>(v: T) -> OwnedValue {
        OwnedValue::try_from(v.into()).unwrap()
    }

    fn path(s: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(s).unwrap()
    }

    fn nul(s: &str) -> Vec<u8> {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        b
    }

    /// One USB drive and one partition on it.
    fn tree(label: &str, vendor: &str, model: &str, mounted: Option<&str>) -> ManagedObjects {
        let drive_path = "/org/freedesktop/UDisks2/drives/D1";
        let mut drive: Props = HashMap::new();
        drive.insert("Vendor".into(), ov(vendor.to_string()));
        drive.insert("Model".into(), ov(model.to_string()));
        drive.insert("ConnectionBus".into(), ov("usb".to_string()));
        drive.insert("Removable".into(), ov(true));
        drive.insert("Ejectable".into(), ov(false));
        drive.insert("CanPowerOff".into(), ov(true));
        let mut block: Props = HashMap::new();
        block.insert("Device".into(), ov(nul("/dev/sdc1")));
        block.insert("IdLabel".into(), ov(label.to_string()));
        block.insert("IdType".into(), ov("xfs".to_string()));
        block.insert("Size".into(), ov(16_000_898_564_096u64));
        block.insert("HintSystem".into(), ov(false));
        block.insert("HintIgnore".into(), ov(false));
        block.insert("Drive".into(), ov(path(drive_path)));
        let mut fs: Props = HashMap::new();
        let points: Vec<Vec<u8>> = mounted.map(|m| vec![nul(m)]).unwrap_or_default();
        fs.insert("MountPoints".into(), ov(points));
        let mut objects = ManagedObjects::new();
        objects.insert(path(drive_path), HashMap::from([(DRIVE_IFACE.to_string(), drive)]));
        objects.insert(
            path("/org/freedesktop/UDisks2/block_devices/sdc1"),
            HashMap::from([(BLOCK_IFACE.to_string(), block), (FS_IFACE.to_string(), fs)]),
        );
        objects
    }

    #[test]
    fn mounted_usb_partition_becomes_a_device() {
        let list = devices(&tree("SEAGATE", "Seagate", "Expansion HDD", Some("/media/fabio/SEAGATE")));
        assert_eq!(list.len(), 1);
        let d = &list[0];
        assert_eq!(d.label, "SEAGATE");
        assert_eq!(d.dev_node, "/dev/sdc1");
        assert_eq!(d.mount_point, Some(PathBuf::from("/media/fabio/SEAGATE")));
        assert_eq!(d.kind, Kind::Usb);
        assert!(d.can_power_off);
        assert!(!d.can_eject);
    }

    #[test]
    fn missing_label_falls_back_to_drive_name_then_size() {
        let list = devices(&tree("", "Seagate", "Expansion HDD", None));
        assert_eq!(list[0].label, "Seagate Expansion HDD");
        assert_eq!(list[0].mount_point, None);
        let list = devices(&tree("", "", "", None));
        assert_eq!(list[0].label, "16 TB Volume");
    }

    #[test]
    fn system_partitions_are_left_out() {
        let mut objects = tree("SEAGATE", "Seagate", "Expansion HDD", None);
        let block = objects
            .get_mut(&path("/org/freedesktop/UDisks2/block_devices/sdc1"))
            .unwrap()
            .get_mut(BLOCK_IFACE)
            .unwrap();
        block.insert("HintSystem".into(), ov(true));
        assert!(devices(&objects).is_empty());
    }
}
