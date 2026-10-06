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
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let rounded = (value * 10.0).round() / 10.0;
    if unit == 0 || rounded.fract() == 0.0 {
        format!("{} {}", rounded as u64, UNITS[unit])
    } else {
        format!("{:.1} {}", rounded, UNITS[unit])
    }
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
