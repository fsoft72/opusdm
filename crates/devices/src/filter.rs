//! Which udisks2 block devices are shown to the user.

/// The facts about a drive the filter and the parser need.
#[derive(Clone, Debug, Default)]
pub struct RawDrive {
    pub path: String,
    pub vendor: String,
    pub model: String,
    pub bus: String,
    pub removable: bool,
    pub ejectable: bool,
    pub can_power_off: bool,
    pub media: String,
}

/// The facts about a block device the filter and the parser need.
#[derive(Clone, Debug, Default)]
pub struct RawBlock {
    pub path: String,
    pub dev_node: String,
    pub id_label: String,
    pub id_type: String,
    pub size: u64,
    pub hint_system: bool,
    pub hint_ignore: bool,
    /// `None` when the block has no `Filesystem` interface.
    pub mount_points: Option<Vec<String>>,
    pub drive: Option<RawDrive>,
}

/// Whether a block device belongs in the notifier: it has a filesystem, udisks2
/// does not hide it or call it a system device, and its drive is removable or
/// on a USB bus.
pub fn is_listed(block: &RawBlock) -> bool {
    if block.mount_points.is_none() || block.hint_system || block.hint_ignore { return false; }
    let Some(drive) = &block.drive else { return false };
    drive.removable || drive.bus == "usb"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick() -> RawBlock {
        RawBlock {
            path: "/org/freedesktop/UDisks2/block_devices/sdc1".into(),
            mount_points: Some(vec![]),
            drive: Some(RawDrive { bus: "usb".into(), removable: true, ..Default::default() }),
            ..Default::default()
        }
    }

    #[test]
    fn usb_stick_is_listed() {
        assert!(is_listed(&stick()));
    }

    #[test]
    fn system_and_ignored_are_not_listed() {
        let mut b = stick();
        b.hint_system = true;
        assert!(!is_listed(&b));
        let mut b = stick();
        b.hint_ignore = true;
        assert!(!is_listed(&b));
    }

    #[test]
    fn no_filesystem_or_no_drive_is_not_listed() {
        let mut b = stick();
        b.mount_points = None;
        assert!(!is_listed(&b));
        let mut b = stick();
        b.drive = None;
        assert!(!is_listed(&b));
    }

    #[test]
    fn internal_nvme_is_not_listed_but_sd_card_is() {
        let mut b = stick();
        b.drive = Some(RawDrive::default());
        assert!(!is_listed(&b));
        b.drive = Some(RawDrive { removable: true, ..Default::default() });
        assert!(is_listed(&b));
    }
}
