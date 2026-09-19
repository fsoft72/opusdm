//! XEmbed system tray (freedesktop `systemtray-spec`): a background thread
//! (`spawn`) owns the `_NET_SYSTEM_TRAY_S<n>` selection and docks icon
//! windows into small container windows it creates, per the XEmbed
//! handshake. Pure message/parsing helpers are kept free of the X11
//! connection so they can be unit tested directly, mirroring `ewmh.rs`.

/// `_NET_SYSTEM_TRAY_OPCODE` opcode: a client asks to dock its icon.
pub const SYSTEM_TRAY_REQUEST_DOCK: u32 = 0;

/// `_XEMBED` opcode sent by the embedder once a client is docked.
pub const XEMBED_EMBEDDED_NOTIFY: u32 = 0;

/// Vertical/horizontal margin reserved around each docked icon inside its
/// container window, so icons don't touch the panel edge.
pub const ICON_MARGIN: i32 = 8;

/// The square size (in px) a docked icon's container window gets, derived
/// from the panel's height.
pub fn icon_square_size(panel_height: i32) -> i32 {
    (panel_height - 2 * ICON_MARGIN).max(1)
}

/// Data words for the root `MANAGER` `ClientMessage` announcing that the
/// tray selection now has an owner.
pub fn manager_message_data(timestamp: u32, selection_atom: u32, manager_xid: u32) -> [u32; 5] {
    [timestamp, selection_atom, manager_xid, 0, 0]
}

/// Data words for the `_XEMBED` `XEMBED_EMBEDDED_NOTIFY` message sent to a
/// newly docked icon window once it has been reparented into `container_xid`.
pub fn xembed_embedded_notify_data(container_xid: u32, xembed_version: u32) -> [u32; 5] {
    [0, XEMBED_EMBEDDED_NOTIFY, 0, container_xid, xembed_version]
}

/// Extract the icon window id from a `_NET_SYSTEM_TRAY_OPCODE`
/// `ClientMessage`'s data words, if it is a dock request.
pub fn parse_dock_request(data: [u32; 5]) -> Option<u32> {
    if data[1] == SYSTEM_TRAY_REQUEST_DOCK {
        Some(data[2])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_square_size_subtracts_margin_on_both_sides() {
        assert_eq!(icon_square_size(40), 24); // 40 - 2*8
    }

    #[test]
    fn icon_square_size_never_goes_below_one() {
        assert_eq!(icon_square_size(4), 1);
    }

    #[test]
    fn manager_message_data_packs_timestamp_selection_and_manager() {
        let d = manager_message_data(12345, 99, 7);
        assert_eq!(d, [12345, 99, 7, 0, 0]);
    }

    #[test]
    fn xembed_embedded_notify_data_packs_container_and_version() {
        let d = xembed_embedded_notify_data(42, 0);
        assert_eq!(d, [0, XEMBED_EMBEDDED_NOTIFY, 0, 42, 0]);
    }

    #[test]
    fn parse_dock_request_extracts_icon_xid() {
        let data = [0, SYSTEM_TRAY_REQUEST_DOCK, 555, 0, 0];
        assert_eq!(parse_dock_request(data), Some(555));
    }

    #[test]
    fn parse_dock_request_ignores_other_opcodes() {
        let data = [0, 2 /* SYSTEM_TRAY_END_MESSAGE */, 555, 0, 0];
        assert_eq!(parse_dock_request(data), None);
    }
}
