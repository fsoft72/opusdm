//! The rfkill switch of Bluetooth: read from `/sys/class/rfkill`, the soft
//! block cleared by writing an event to `/dev/rfkill` (the device is
//! writable by the user of the seat, no password needed).

use crate::error::BtError;
use std::io::Write;
use std::path::Path;

/// Folder of the rfkill switches.
const SYS_RFKILL: &str = "/sys/class/rfkill";
/// The rfkill control device.
const DEV_RFKILL: &str = "/dev/rfkill";
/// `RFKILL_TYPE_BLUETOOTH`.
const TYPE_BLUETOOTH: u8 = 2;
/// `RFKILL_OP_CHANGE_ALL`: change every switch of the type.
const OP_CHANGE_ALL: u8 = 3;

/// The state of the Bluetooth switches: blocked when any of them is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rfkill {
    /// Blocked by software (`rfkill block`, airplane mode).
    pub soft: bool,
    /// Blocked by a hardware switch: cannot be cleared from here.
    pub hard: bool,
}

/// Read the Bluetooth switches; no switch reads as not blocked.
pub fn read() -> Rfkill {
    read_from(Path::new(SYS_RFKILL))
}

/// [`read`] from another folder, for tests.
fn read_from(dir: &Path) -> Rfkill {
    let mut out = Rfkill::default();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let p = entry.path();
        let field = |name: &str| std::fs::read_to_string(p.join(name)).unwrap_or_default().trim().to_string();
        if field("type") != "bluetooth" { continue; }
        out.soft |= field("soft") == "1";
        out.hard |= field("hard") == "1";
    }
    out
}

/// The `struct rfkill_event` that clears the soft block of every Bluetooth
/// switch: idx (u32), type, op, soft, hard.
pub fn encode_unblock() -> [u8; 8] {
    [0, 0, 0, 0, TYPE_BLUETOOTH, OP_CHANGE_ALL, 0, 0]
}

/// Clear the soft block of every Bluetooth switch.
pub fn unblock() -> Result<(), BtError> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(DEV_RFKILL)
        .map_err(|e| BtError::other(format!("Cannot open {DEV_RFKILL}: {e}")))?;
    f.write_all(&encode_unblock()).map_err(|e| BtError::other(format!("Cannot unblock Bluetooth: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfkill_unblock_event_bytes() {
        assert_eq!(encode_unblock(), [0, 0, 0, 0, 2, 3, 0, 0]);
    }

    #[test]
    fn only_bluetooth_switches_count() {
        let dir = std::env::temp_dir().join(format!("opusdm-rfkill-{}", std::process::id()));
        for (name, kind, soft, hard) in [("rfkill0", "bluetooth", "1", "0"), ("rfkill1", "wlan", "0", "1")] {
            let d = dir.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
            std::fs::write(d.join("soft"), soft).unwrap();
            std::fs::write(d.join("hard"), hard).unwrap();
        }
        assert_eq!(read_from(&dir), Rfkill { soft: true, hard: false });
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(read_from(&dir), Rfkill::default());
    }
}
