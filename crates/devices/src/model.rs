//! Plain data for one mountable volume.

use std::path::PathBuf;

/// What kind of storage a volume lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Usb,
    Disk,
    Card,
}

/// One volume (a partition with a filesystem) on a removable drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// udisks2 object path of the block device.
    pub path: String,
    /// udisks2 object path of the drive that holds it.
    pub drive_path: String,
    /// Name shown to the user.
    pub label: String,
    pub size: u64,
    /// Device node, for example `/dev/sdc1`.
    pub dev_node: String,
    pub fs_type: String,
    pub mount_point: Option<PathBuf>,
    pub kind: Kind,
    pub can_eject: bool,
    pub can_power_off: bool,
    /// `"Vendor Model"` of the drive, may be empty.
    pub drive_name: String,
}

/// Size with decimal units, one decimal only when it is not a round number
/// (`"32 GB"`, `"1.5 TB"`, `"0 B"`).
pub fn human_size(bytes: u64) -> String {
    opusdm_ipc::size::decimal_size(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(32_000_000_000), "32 GB");
        assert_eq!(human_size(1_500_000_000_000), "1.5 TB");
        assert_eq!(human_size(16_000_898_564_096), "16 TB");
    }
}
