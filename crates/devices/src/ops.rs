//! Compound operations built on [`Backend`].

use crate::backend::Backend;
use crate::error::DeviceError;
use crate::model::Device;

/// Eject the drive of `device`: unmount every mounted volume of that drive
/// (`all` is the full list), then eject, powering off when the drive can.
/// Stops at the first failed unmount and does not eject then.
pub fn eject(backend: &dyn Backend, device: &Device, all: &[Device]) -> Result<(), DeviceError> {
    for v in all.iter().filter(|d| d.drive_path == device.drive_path && d.mount_point.is_some()) {
        match backend.unmount(&v.path) {
            Err(e) if !e.is_already_done() => return Err(e),
            _ => {}
        }
    }
    backend.eject_drive(&device.drive_path, device.can_power_off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::FakeBackend;
    use crate::diff::tests::dev;

    fn mounted(name: &str, drive: &str) -> Device {
        let mut d = dev(name, drive);
        d.mount_point = Some(format!("/media/fabio/{name}").into());
        d
    }

    fn p(name: &str) -> String {
        format!("/org/freedesktop/UDisks2/block_devices/{name}")
    }

    #[test]
    fn eject_unmounts_every_mounted_volume_then_ejects() {
        let all = vec![mounted("sdb1", "D"), mounted("sdb2", "D"), dev("sdb3", "D"), mounted("sdc1", "E")];
        let fake = FakeBackend::default();
        eject(&fake, &all[0], &all).unwrap();
        assert_eq!(
            fake.calls(),
            vec![
                format!("unmount {}", p("sdb1")),
                format!("unmount {}", p("sdb2")),
                "eject /org/freedesktop/UDisks2/drives/D true".to_string(),
            ]
        );
    }

    #[test]
    fn failed_unmount_stops_before_eject() {
        let all = vec![mounted("sdb1", "D")];
        let fake = FakeBackend::default();
        *fake.fail_unmount.lock().unwrap() =
            Some(DeviceError::from_dbus("org.freedesktop.UDisks2.Error.DeviceBusy", "busy"));
        let err = eject(&fake, &all[0], &all).unwrap_err();
        assert_eq!(err.text(), "Device is busy");
        assert_eq!(fake.calls(), vec![format!("unmount {}", p("sdb1"))]);
    }
}
