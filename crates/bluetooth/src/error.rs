//! Errors of the BlueZ calls: the shared [`opusdm_watch::DbusError`], with a
//! readable one line text.

/// An error reported by BlueZ or by the bus.
pub type BtError = opusdm_watch::DbusError;

/// One line for the user, with the BlueZ errors that have a terse message
/// told in words.
pub fn text(e: &BtError) -> String {
    let words = match e.name.rsplit('.').next().unwrap_or("") {
        "AuthenticationFailed" => "Pairing failed: wrong code or refused",
        "AuthenticationCanceled" => "Pairing was canceled",
        "AuthenticationRejected" => "Pairing was rejected",
        "AuthenticationTimeout" => "The device did not answer in time",
        "ConnectionAttemptFailed" => "Could not reach the device",
        "AlreadyExists" => "The device is already paired",
        "InProgress" => "Busy: another operation is in progress",
        "NotReady" => "Bluetooth is off",
        _ => return e.text(),
    };
    words.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluez_errors_get_words() {
        assert_eq!(text(&BtError::from_dbus("org.bluez.Error.AuthenticationFailed", "")), "Pairing failed: wrong code or refused");
        assert_eq!(text(&BtError::from_dbus("org.bluez.Error.Failed", "br-connection-page-timeout")), "br-connection-page-timeout");
    }
}
