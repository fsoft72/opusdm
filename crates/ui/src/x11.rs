//! Small X11 helpers shared by the processes that talk to the window manager
//! over a short-lived `x11rb` connection.

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::ConnectionExt;

/// Flush `conn` and wait until the X server has processed everything sent on
/// it. Call it before dropping a short-lived connection: dropping it right
/// after a plain flush can lose the requests the server has not read yet, so
/// the window manager never sees the `ClientMessage`s or configure requests.
pub fn x11_sync(conn: &impl Connection) -> Result<(), ReplyError> {
    conn.get_input_focus()?.reply()?;
    Ok(())
}
