//! Errors of the NetworkManager calls: the shared
//! [`opusdm_watch::DbusError`], with a readable one line text.

/// An error reported by NetworkManager or by the bus.
pub type NetError = opusdm_watch::DbusError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_get_a_short_text() {
        let e = NetError::from_dbus("org.freedesktop.NetworkManager.PermissionDenied", "long polkit text");
        assert_eq!(e.text(), "Not authorized");
    }

    #[test]
    fn only_password_needed_asks_for_a_password() {
        assert!(NetError::password_needed().needs_password);
        assert!(!NetError::other("x").needs_password);
        assert!(!NetError::from_dbus("org.x.Failed", "boom").needs_password);
    }

    #[test]
    fn other_errors_show_the_message_or_the_name() {
        assert_eq!(NetError::other("no bus").text(), "no bus");
        assert_eq!(NetError::from_dbus("org.x.Failed", "").text(), "org.x.Failed");
        assert_eq!(NetError::from_dbus("org.x.Failed", "boom").text(), "boom");
    }
}
