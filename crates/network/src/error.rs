//! Errors of the NetworkManager calls, with a readable one line text.

/// An error reported by NetworkManager or by the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetError {
    /// D-Bus error name, empty for a transport or logic error.
    pub name: String,
    pub message: String,
}

impl NetError {
    /// Build from a D-Bus error name and message.
    pub fn from_dbus(name: &str, message: &str) -> Self {
        Self { name: name.to_string(), message: message.to_string() }
    }

    /// An error with no D-Bus name (connection problems, failed joins).
    pub fn other(message: impl Into<String>) -> Self {
        Self { name: String::new(), message: message.into() }
    }

    /// Convert a zbus error, keeping the remote error name when there is one.
    pub fn from_zbus(e: &zbus::Error) -> Self {
        match e {
            zbus::Error::MethodError(name, msg, _) => Self::from_dbus(name.as_str(), msg.as_deref().unwrap_or("")),
            other => Self::other(other.to_string()),
        }
    }

    /// One line for the user: permission errors get a short text, anything
    /// else shows the NetworkManager message.
    pub fn text(&self) -> String {
        if self.name.ends_with(".PermissionDenied") || self.name.ends_with(".NotAuthorized") {
            return "Not authorized".to_string();
        }
        if self.message.is_empty() && !self.name.is_empty() { return self.name.clone(); }
        self.message.clone()
    }
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for NetError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_get_a_short_text() {
        let e = NetError::from_dbus("org.freedesktop.NetworkManager.PermissionDenied", "long polkit text");
        assert_eq!(e.text(), "Not authorized");
    }

    #[test]
    fn other_errors_show_the_message_or_the_name() {
        assert_eq!(NetError::other("no bus").text(), "no bus");
        assert_eq!(NetError::from_dbus("org.x.Failed", "").text(), "org.x.Failed");
        assert_eq!(NetError::from_dbus("org.x.Failed", "boom").text(), "boom");
    }
}
