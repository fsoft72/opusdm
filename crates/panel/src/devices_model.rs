//! The list of removable devices as the panel sees it: pure state, no GTK.

use opusdm_devices::{Device, DeviceEvent, WatchMsg};
use std::collections::HashSet;

/// One thing a row of the drop-down can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Mount,
    Open,
    Unmount,
    Eject,
}

/// One row of the drop-down.
#[derive(Clone, Debug)]
pub struct Row {
    pub device: Device,
    pub actions: Vec<Action>,
    pub busy: bool,
}

/// Devices, the ones running an operation, and the new ones not yet announced.
#[derive(Default)]
pub struct Model {
    devices: Vec<Device>,
    busy: HashSet<String>,
    announce: Vec<Device>,
    seeded: bool,
}

impl Model {
    /// Apply one message of the watcher. A snapshot never announces anything:
    /// devices already present at start-up must not pop up.
    pub fn apply(&mut self, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(_) | WatchMsg::Unavailable(_) => {}
            WatchMsg::Snapshot(list) => {
                self.devices = list;
                self.seeded = true;
                self.announce.clear();
                self.busy.retain(|p| self.devices.iter().any(|d| &d.path == p));
            }
            WatchMsg::Event(DeviceEvent::Added(d)) => {
                if self.devices.iter().any(|x| x.path == d.path) { return; }
                if self.seeded && d.mount_point.is_none() { self.announce.push(d.clone()); }
                self.devices.push(d);
                self.devices.sort_by(|a, b| a.dev_node.cmp(&b.dev_node));
            }
            WatchMsg::Event(DeviceEvent::Changed(d)) => {
                if d.mount_point.is_some() { self.announce.retain(|a| a.path != d.path); }
                if let Some(slot) = self.devices.iter_mut().find(|x| x.path == d.path) { *slot = d; }
            }
            WatchMsg::Event(DeviceEvent::Removed(path)) => {
                self.devices.retain(|d| d.path != path);
                self.announce.retain(|d| d.path != path);
                self.busy.remove(&path);
            }
        }
    }

    /// Every device, sorted by device node.
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    /// The device at object path `path`.
    pub fn find(&self, path: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.path == path)
    }

    /// Whether there is nothing to show.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Mark a device as running (or done with) an operation.
    pub fn set_busy(&mut self, path: &str, busy: bool) {
        if busy { self.busy.insert(path.to_string()); } else { self.busy.remove(path); }
    }

    /// Whether a device runs an operation.
    pub fn is_busy(&self, path: &str) -> bool {
        self.busy.contains(path)
    }

    /// The devices that arrived since the last call and still wait for a popup.
    pub fn take_new(&mut self) -> Vec<Device> {
        std::mem::take(&mut self.announce)
    }

    /// The rows of the drop-down. Eject is offered once per drive, on its first
    /// volume, when the drive can eject or power off.
    pub fn rows(&self) -> Vec<Row> {
        let mut seen_drives: HashSet<&str> = HashSet::new();
        self.devices
            .iter()
            .map(|d| {
                let first_of_drive = seen_drives.insert(d.drive_path.as_str());
                let mut actions = if d.mount_point.is_some() {
                    vec![Action::Open, Action::Unmount]
                } else {
                    vec![Action::Mount]
                };
                if first_of_drive && (d.can_eject || d.can_power_off) { actions.push(Action::Eject); }
                Row { device: d.clone(), actions, busy: self.is_busy(&d.path) }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_devices::Kind;

    fn dev(name: &str, drive: &str, mounted: bool) -> Device {
        Device {
            path: format!("/b/{name}"),
            drive_path: format!("/d/{drive}"),
            label: name.to_string(),
            size: 1_000_000_000,
            dev_node: format!("/dev/{name}"),
            fs_type: "vfat".into(),
            mount_point: mounted.then(|| format!("/media/fabio/{name}").into()),
            kind: Kind::Usb,
            can_eject: true,
            can_power_off: true,
            drive_name: String::new(),
        }
    }

    fn added(d: Device) -> WatchMsg {
        WatchMsg::Event(DeviceEvent::Added(d))
    }

    #[test]
    fn start_up_devices_are_not_announced() {
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![dev("sdb1", "A", false)]));
        assert_eq!(m.devices().len(), 1);
        assert!(m.take_new().is_empty());
    }

    #[test]
    fn a_new_device_is_announced_once() {
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![]));
        m.apply(added(dev("sdb1", "A", false)));
        m.apply(added(dev("sdb1", "A", false)));
        assert_eq!(m.devices().len(), 1);
        assert_eq!(m.take_new().len(), 1);
        assert!(m.take_new().is_empty());
    }

    #[test]
    fn an_already_mounted_device_is_not_announced() {
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![]));
        m.apply(added(dev("sdb1", "A", true)));
        assert!(m.take_new().is_empty());
    }

    #[test]
    fn mounting_cancels_the_pending_announcement() {
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![]));
        m.apply(added(dev("sdb1", "A", false)));
        m.apply(WatchMsg::Event(DeviceEvent::Changed(dev("sdb1", "A", true))));
        assert!(m.take_new().is_empty());
        assert!(m.devices()[0].mount_point.is_some());
    }

    #[test]
    fn removal_drops_the_device_and_its_busy_flag() {
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![dev("sdb1", "A", true)]));
        m.set_busy("/b/sdb1", true);
        m.apply(WatchMsg::Event(DeviceEvent::Removed("/b/sdb1".into())));
        assert!(m.is_empty());
        assert!(!m.is_busy("/b/sdb1"));
    }

    #[test]
    fn rows_offer_eject_once_per_drive() {
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![dev("sdb1", "A", true), dev("sdb2", "A", true), dev("sdc1", "B", false)]));
        let rows = m.rows();
        assert_eq!(rows[0].actions, vec![Action::Open, Action::Unmount, Action::Eject]);
        assert_eq!(rows[1].actions, vec![Action::Open, Action::Unmount]);
        assert_eq!(rows[2].actions, vec![Action::Mount, Action::Eject]);
    }

    #[test]
    fn a_drive_that_cannot_eject_gets_no_eject() {
        let mut d = dev("sdb1", "A", false);
        d.can_eject = false;
        d.can_power_off = false;
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![d]));
        assert_eq!(m.rows()[0].actions, vec![Action::Mount]);
    }

    #[test]
    fn a_new_snapshot_replaces_the_list() {
        let mut m = Model::default();
        m.apply(WatchMsg::Snapshot(vec![dev("sdb1", "A", false)]));
        m.apply(WatchMsg::Unavailable("gone".into()));
        m.apply(WatchMsg::Snapshot(vec![dev("sdc1", "B", false)]));
        assert_eq!(m.devices().len(), 1);
        assert_eq!(m.devices()[0].dev_node, "/dev/sdc1");
    }
}
