//! Errors of the UPower and power-profiles-daemon calls: the shared
//! [`opusdm_watch::DbusError`], with a readable one line text.

/// An error reported by a power service or by the bus.
pub type PowerError = opusdm_watch::DbusError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_get_a_short_text() {
        let e = PowerError::from_dbus("org.freedesktop.DBus.Error.AccessDenied", "long polkit text");
        assert_eq!(e.text(), "Not authorized");
    }

    #[test]
    fn other_errors_show_the_message_or_the_name() {
        assert_eq!(PowerError::other("no bus").text(), "no bus");
        assert_eq!(PowerError::from_dbus("org.x.Failed", "").text(), "org.x.Failed");
        assert_eq!(PowerError::from_dbus("org.x.Failed", "boom").text(), "boom");
    }

    #[test]
    fn a_refused_property_write_says_not_authorized() {
        // `set_property` fails with an `fdo::Error`; its name must survive.
        let e = PowerError::from(zbus::fdo::Error::AccessDenied("polkit".into()));
        assert_eq!(e.text(), "Not authorized");
    }
}
