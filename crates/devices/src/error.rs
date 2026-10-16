//! Errors of the udisks2 calls: the shared [`opusdm_watch::DbusError`],
//! with a readable one line text.

/// An error reported by udisks2 or by the bus.
pub type DeviceError = opusdm_watch::DbusError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_get_short_texts() {
        let busy = DeviceError::from_dbus("org.freedesktop.UDisks2.Error.DeviceBusy", "target is busy");
        assert_eq!(busy.text(), "Device is busy");
        let auth = DeviceError::from_dbus("org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain", "x");
        assert_eq!(auth.text(), "Not authorized");
    }

    #[test]
    fn unknown_name_shows_the_message() {
        let e = DeviceError::from_dbus("org.freedesktop.UDisks2.Error.Failed", "wrong fs type");
        assert_eq!(e.text(), "wrong fs type");
        assert!(!e.is_already_done());
        assert!(DeviceError::from_dbus("org.freedesktop.UDisks2.Error.AlreadyMounted", "").is_already_done());
    }
}
