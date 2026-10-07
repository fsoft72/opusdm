//! Errors of the sound server calls, with a readable one line text.

/// An error of the sound server or of the connection to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// No server, a lost connection, or a call that got no answer in time.
    ConnectionFailed(String),
    /// The device is gone (unplugged in the meantime).
    NoSuchDevice,
    /// The server answered that it did not do it.
    Rejected(String),
}

impl AudioError {
    /// One line for the user.
    pub fn text(&self) -> String {
        match self {
            AudioError::ConnectionFailed(why) => format!("Cannot reach the sound server: {why}"),
            AudioError::NoSuchDevice => "The device is gone".to_string(),
            AudioError::Rejected(why) => format!("The sound server refused the change: {why}"),
        }
    }
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_variant_has_a_one_line_text() {
        assert_eq!(AudioError::NoSuchDevice.text(), "The device is gone");
        assert_eq!(
            AudioError::ConnectionFailed("timeout".into()).text(),
            "Cannot reach the sound server: timeout"
        );
        assert_eq!(
            AudioError::Rejected("no entity".into()).text(),
            "The sound server refused the change: no entity"
        );
        for e in [AudioError::NoSuchDevice, AudioError::ConnectionFailed("x".into()), AudioError::Rejected("x".into())] {
            assert!(!e.text().contains('\n'));
        }
    }
}
