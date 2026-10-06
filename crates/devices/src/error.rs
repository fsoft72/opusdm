//! Errors of the udisks2 calls, with a readable one line text.

/// An error reported by udisks2 or by the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceError {
    /// D-Bus error name, empty for a transport error.
    pub name: String,
    pub message: String,
}

impl DeviceError {
    /// Build from a D-Bus error name and message.
    pub fn from_dbus(name: &str, message: &str) -> Self {
        Self { name: name.to_string(), message: message.to_string() }
    }

    /// An error with no D-Bus name (connection problems and similar).
    pub fn other(message: impl Into<String>) -> Self {
        Self { name: String::new(), message: message.into() }
    }

    /// Convert a zbus error, keeping the remote error name when there is one.
    pub fn from_zbus(e: &zbus::Error) -> Self {
        match e {
            zbus::Error::MethodError(name, msg, _) => {
                Self::from_dbus(name.as_str(), msg.as_deref().unwrap_or(""))
            }
            other => Self::other(other.to_string()),
        }
    }

    /// One line for the user: `DeviceBusy` and `NotAuthorized*` get short
    /// texts, anything else shows the udisks2 message.
    pub fn text(&self) -> String {
        if self.name.ends_with(".DeviceBusy") { return "Device is busy".to_string(); }
        if self.name.contains(".NotAuthorized") { return "Not authorized".to_string(); }
        if self.message.is_empty() { return self.name.clone(); }
        self.message.clone()
    }

    /// Whether the error only says the volume is already in the wanted state.
    pub fn is_already_done(&self) -> bool {
        self.name.ends_with(".AlreadyMounted") || self.name.ends_with(".NotMounted")
    }
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for DeviceError {}

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
