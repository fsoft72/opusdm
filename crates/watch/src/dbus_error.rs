//! The error of a D-Bus service call, shared by the device, power and
//! network crates, with a readable one line text.

use zbus::DBusError as _;

/// An error reported by a D-Bus service or by the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbusError {
    /// D-Bus error name, empty for a transport or logic error.
    pub name: String,
    pub message: String,
    /// The network is saved but its key is not: the caller must ask for it
    /// and connect again with it. Only NetworkManager calls set it.
    pub needs_password: bool,
}

impl DbusError {
    /// Build from a D-Bus error name and message.
    pub fn from_dbus(name: &str, message: &str) -> Self {
        Self { name: name.to_string(), message: message.to_string(), needs_password: false }
    }

    /// An error with no D-Bus name (connection problems and similar).
    pub fn other(message: impl Into<String>) -> Self {
        Self { name: String::new(), message: message.into(), needs_password: false }
    }

    /// The saved network has no stored key, so a password is needed to join it.
    pub fn password_needed() -> Self {
        Self { name: String::new(), message: "A password is needed".to_string(), needs_password: true }
    }

    /// One line for the user: permission errors and a busy device get a
    /// short text, anything else shows the service message (or the error
    /// name when there is no message).
    pub fn text(&self) -> String {
        if self.is_not_authorized() { return "Not authorized".to_string(); }
        if self.name.ends_with(".DeviceBusy") { return "Device is busy".to_string(); }
        if self.message.is_empty() { return self.name.clone(); }
        self.message.clone()
    }

    /// Whether the service refused the call for lack of permission.
    pub fn is_not_authorized(&self) -> bool {
        self.name.ends_with(".PermissionDenied") || self.name.ends_with(".AccessDenied") || self.name.contains(".NotAuthorized")
    }

    /// Whether the error only says the volume is already in the wanted state
    /// (udisks2 `AlreadyMounted` / `NotMounted`).
    pub fn is_already_done(&self) -> bool {
        self.name.ends_with(".AlreadyMounted") || self.name.ends_with(".NotMounted")
    }
}

impl From<&zbus::Error> for DbusError {
    /// Keep the remote error name when there is one, also for the standard
    /// `org.freedesktop.DBus.Error.*` errors zbus wraps in `fdo::Error`.
    fn from(e: &zbus::Error) -> Self {
        match e {
            zbus::Error::MethodError(name, msg, _) => Self::from_dbus(name.as_str(), msg.as_deref().unwrap_or("")),
            zbus::Error::FDO(fdo) => Self::from(fdo.as_ref()),
            other => Self::other(other.to_string()),
        }
    }
}

impl From<zbus::Error> for DbusError {
    /// See `From<&zbus::Error>`.
    fn from(e: zbus::Error) -> Self {
        Self::from(&e)
    }
}

impl From<&zbus::fdo::Error> for DbusError {
    /// Keep the standard error name, unwrapping a plain zbus error.
    fn from(e: &zbus::fdo::Error) -> Self {
        match e {
            zbus::fdo::Error::ZBus(inner) => Self::from(inner),
            other => Self::from_dbus(other.name().as_str(), other.description().unwrap_or("")),
        }
    }
}

impl From<zbus::fdo::Error> for DbusError {
    /// See `From<&zbus::fdo::Error>`.
    fn from(e: zbus::fdo::Error) -> Self {
        Self::from(&e)
    }
}

impl std::fmt::Display for DbusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for DbusError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_get_a_short_text() {
        for name in [
            "org.freedesktop.NetworkManager.PermissionDenied",
            "org.freedesktop.DBus.Error.AccessDenied",
            "org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain",
        ] {
            assert_eq!(DbusError::from_dbus(name, "long polkit text").text(), "Not authorized", "{name}");
        }
    }

    #[test]
    fn a_busy_device_gets_a_short_text() {
        let busy = DbusError::from_dbus("org.freedesktop.UDisks2.Error.DeviceBusy", "target is busy");
        assert_eq!(busy.text(), "Device is busy");
    }

    #[test]
    fn other_errors_show_the_message_or_the_name() {
        assert_eq!(DbusError::other("no bus").text(), "no bus");
        assert_eq!(DbusError::from_dbus("org.x.Failed", "").text(), "org.x.Failed");
        assert_eq!(DbusError::from_dbus("org.x.Failed", "boom").text(), "boom");
    }

    #[test]
    fn only_password_needed_asks_for_a_password() {
        assert!(DbusError::password_needed().needs_password);
        assert!(!DbusError::other("x").needs_password);
        assert!(!DbusError::from_dbus("org.x.Failed", "boom").needs_password);
    }

    #[test]
    fn already_mounted_and_not_mounted_are_already_done() {
        assert!(DbusError::from_dbus("org.freedesktop.UDisks2.Error.AlreadyMounted", "").is_already_done());
        assert!(DbusError::from_dbus("org.freedesktop.UDisks2.Error.NotMounted", "").is_already_done());
        assert!(!DbusError::from_dbus("org.freedesktop.UDisks2.Error.Failed", "").is_already_done());
    }

    #[test]
    fn a_wrapped_fdo_error_keeps_its_name() {
        let e = DbusError::from(zbus::Error::from(zbus::fdo::Error::AccessDenied("no".into())));
        assert_eq!(e.name, "org.freedesktop.DBus.Error.AccessDenied");
        assert_eq!(e.message, "no");
        assert_eq!(e.text(), "Not authorized");
    }

    #[test]
    fn a_plain_zbus_error_has_no_name() {
        let e = DbusError::from(zbus::Error::InterfaceNotFound);
        assert!(e.name.is_empty());
        assert_eq!(e.text(), zbus::Error::InterfaceNotFound.to_string());
        let wrapped = DbusError::from(zbus::fdo::Error::ZBus(zbus::Error::InterfaceNotFound));
        assert!(wrapped.name.is_empty());
    }
}
