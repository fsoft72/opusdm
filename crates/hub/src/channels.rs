//! Helpers for the `mpsc` channels between the hub's threads.

use std::sync::mpsc::Sender;

/// Send `msg` on `tx`. When the receiving side is gone the message is lost:
/// log it, naming it with `what` (e.g. "settings save"), instead of dropping
/// it silently. Returns whether the message was delivered.
pub fn send_or_log<T>(tx: &Sender<T>, msg: T, what: &str) -> bool {
    if tx.send(msg).is_ok() {
        return true;
    }
    opusdm_ipc::err_log!("hub", "{what} lost: its receiver is gone");
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn a_delivered_message_reports_true() {
        let (tx, rx) = channel();
        assert!(send_or_log(&tx, 7, "test message"));
        assert_eq!(rx.recv().unwrap(), 7);
    }

    #[test]
    fn a_message_to_a_closed_channel_reports_false_without_panicking() {
        let (tx, rx) = channel::<u32>();
        drop(rx);
        assert!(!send_or_log(&tx, 7, "test message"));
    }
}
