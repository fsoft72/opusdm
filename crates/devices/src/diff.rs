//! Differences between two device lists.

use crate::model::Device;

/// One change of the device list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceEvent {
    Added(Device),
    /// The object path of the device that went away.
    Removed(String),
    Changed(Device),
}

/// Events that turn `old` into `new`. `Changed` only when the mount point,
/// the label or the size differ.
pub fn diff(old: &[Device], new: &[Device]) -> Vec<DeviceEvent> {
    let mut events = Vec::new();
    for d in old.iter().filter(|o| !new.iter().any(|n| n.path == o.path)) {
        events.push(DeviceEvent::Removed(d.path.clone()));
    }
    for n in new {
        match old.iter().find(|o| o.path == n.path) {
            None => events.push(DeviceEvent::Added(n.clone())),
            Some(o) if o.mount_point != n.mount_point || o.label != n.label || o.size != n.size => {
                events.push(DeviceEvent::Changed(n.clone()))
            }
            Some(_) => {}
        }
    }
    events
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::Kind;

    /// A device for tests: unmounted USB volume `name` on drive `drive`.
    pub(crate) fn dev(name: &str, drive: &str) -> Device {
        Device {
            path: format!("/org/freedesktop/UDisks2/block_devices/{name}"),
            drive_path: format!("/org/freedesktop/UDisks2/drives/{drive}"),
            label: name.to_string(),
            size: 1_000_000_000,
            dev_node: format!("/dev/{name}"),
            fs_type: "vfat".into(),
            mount_point: None,
            kind: Kind::Usb,
            can_eject: true,
            can_power_off: true,
            drive_name: String::new(),
        }
    }

    #[test]
    fn added_and_removed() {
        let a = dev("sdb1", "D");
        assert_eq!(diff(&[], &[a.clone()]), vec![DeviceEvent::Added(a.clone())]);
        assert_eq!(diff(&[a.clone()], &[]), vec![DeviceEvent::Removed(a.path)]);
    }

    #[test]
    fn unchanged_gives_nothing_and_mount_gives_changed() {
        let a = dev("sdb1", "D");
        assert!(diff(&[a.clone()], &[a.clone()]).is_empty());
        let mut m = a.clone();
        m.mount_point = Some("/media/fabio/X".into());
        assert_eq!(diff(&[a], &[m.clone()]), vec![DeviceEvent::Changed(m)]);
    }
}
