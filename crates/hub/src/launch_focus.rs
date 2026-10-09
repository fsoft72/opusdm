//! Give the focus to the window of a program the hub launches (an `exec:`
//! hotkey, a Run step), X11 only.
//!
//! The window manager (Openbox) refuses the focus to a new window it cannot
//! tie to a recent user action: the key press of a global hotkey goes to the
//! hub's grab, so the new window looks unrequested. Two layers fix that:
//!
//! 1. Startup notification: before the spawn a `new:` message announces the
//!    launch with the current X server time and the program gets the same id
//!    in `DESKTOP_STARTUP_ID` (GTK, Qt and KDE apps pick it up). The window
//!    manager then knows the launch came from the user.
//! 2. Fallback: for a few seconds a thread watches `_NET_CLIENT_LIST`; the
//!    first new window that surely belongs to the launch (same startup id,
//!    same pid, or a `WM_CLASS` equal to the program name) is activated with
//!    a `_NET_ACTIVE_WINDOW` request flagged as coming from a pager, which
//!    the window manager honours, unless it already has the focus.
//!
//! Without an X connection (Wayland, tests without `DISPLAY`) nothing
//! happens and the program starts as before.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_FROM_PARENT;

/// Environment variable that hands the startup id to the launched program.
pub const STARTUP_ID_ENV: &str = "DESKTOP_STARTUP_ID";

/// Bytes of a startup notification message carried by one `ClientMessage`.
const CHUNK_LEN: usize = 20;

/// How long the fallback watches for the launched program's window.
const WATCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause between two looks at `_NET_CLIENT_LIST`.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a matched window may take to get the focus by itself before the
/// fallback activates it.
const FOCUS_GRACE: Duration = Duration::from_millis(300);

/// EWMH source indication for a request from a pager (direct user action).
const SOURCE_PAGER: u32 = 2;

/// Sequence number that keeps the startup ids of one hub unique.
static NEXT_SEQ: AtomicU64 = AtomicU64::new(0);

/// What is known about a window that appeared after a launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowInfo {
    /// `_NET_STARTUP_ID`, when the program set it.
    pub startup_id: Option<String>,
    /// `_NET_WM_PID`, when the program set it.
    pub pid: Option<u32>,
    /// The two strings of `WM_CLASS` (instance, class).
    pub wm_class: Vec<String>,
}

/// One announced launch: its startup id and the windows that existed before
/// it, so the fallback only looks at new ones.
pub struct Launch {
    conn: RustConnection,
    root: Window,
    source: Window,
    id: String,
    bin: String,
    known: HashSet<Window>,
}

/// The startup id of the `seq`-th launch of `bin` by process `pid` at X
/// server time `time`. The `_TIME` suffix is how the receiver learns the
/// time of the user action.
pub fn startup_id(pid: u32, seq: u64, bin: &str, time: u32) -> String {
    let name: String = bin.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    format!("opusdm-{pid}-{seq}-{name}_TIME{time}")
}

/// `value` as a quoted startup notification value: `"` and `\` are escaped.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The `new:` message announcing the launch `id` of `bin` on `screen`.
pub fn new_message(id: &str, bin: &str, screen: usize) -> String {
    let (id, bin) = (quote(id), quote(bin));
    format!("new: ID={id} NAME={bin} BIN={bin} SCREEN={screen}")
}

/// The `remove:` message that ends the launch `id`.
pub fn remove_message(id: &str) -> String {
    format!("remove: ID={}", quote(id))
}

/// `message` with its terminating NUL, cut into zero padded pieces of one
/// `ClientMessage` each.
pub fn chunks(message: &str) -> Vec<[u8; CHUNK_LEN]> {
    let mut bytes = message.as_bytes().to_vec();
    bytes.push(0);
    bytes
        .chunks(CHUNK_LEN)
        .map(|piece| {
            let mut chunk = [0u8; CHUNK_LEN];
            chunk[..piece.len()].copy_from_slice(piece);
            chunk
        })
        .collect()
}

/// Whether `window` surely belongs to the launch `id` of `bin` as process
/// `pid`: same startup id, same pid, or a `WM_CLASS` string equal to the
/// program name (ignoring case, like Openbox does).
pub fn belongs_to_launch(window: &WindowInfo, id: &str, pid: u32, bin: &str) -> bool {
    if window.startup_id.as_deref() == Some(id) || window.pid == Some(pid) {
        return true;
    }
    !bin.is_empty() && window.wm_class.iter().any(|c| c.eq_ignore_ascii_case(bin))
}

/// The file name of `program`, the name the window manager matches against
/// `WM_CLASS`.
fn bin_name(program: &str) -> String {
    Path::new(program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| program.to_string())
}

/// Announce the launch of `program` and remember the existing windows.
/// `None` when there is no X server or it cannot be talked to: the caller
/// then starts the program without a startup id.
pub fn begin(program: &str) -> Option<Launch> {
    match try_begin(program) {
        Ok(launch) => Some(launch),
        Err(e) => {
            if std::env::var_os("DISPLAY").is_some() {
                opusdm_ipc::err_log!("hub", "launch focus: cannot announce '{program}': {e}");
            }
            None
        }
    }
}

/// [`begin`] with the error.
fn try_begin(program: &str) -> Result<Launch, opusdm_x11::X11Error> {
    let opusdm_x11::Session { conn, screen_num, root } = opusdm_x11::connect()?;
    let source = conn.generate_id()?;
    let aux = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);
    conn.create_window(0, source, root, -100, -100, 1, 1, 0, WindowClass::INPUT_ONLY, COPY_FROM_PARENT, &aux)?;
    let time = server_time(&conn, source)?;
    let bin = bin_name(program);
    let id = startup_id(std::process::id(), NEXT_SEQ.fetch_add(1, Ordering::Relaxed), &bin, time);
    let known = client_list(&conn, root)?.into_iter().collect();
    let launch = Launch { conn, root, source, id, bin, known };
    launch.broadcast(&new_message(&launch.id, &launch.bin, screen_num))?;
    Ok(launch)
}

/// The current X server time: the timestamp of the `PropertyNotify` caused
/// by touching a property of `window`.
fn server_time(conn: &RustConnection, window: Window) -> Result<u32, opusdm_x11::X11Error> {
    let atom = conn.intern_atom(false, b"_OPUSDM_TIMESTAMP")?.reply()?.atom;
    conn.change_property8(PropMode::APPEND, window, atom, AtomEnum::STRING, &[])?;
    conn.flush()?;
    loop {
        if let Event::PropertyNotify(ev) = conn.wait_for_event()? {
            if ev.window == window && ev.atom == atom {
                return Ok(ev.time);
            }
        }
    }
}

/// The windows the window manager lists as managed clients.
fn client_list(conn: &RustConnection, root: Window) -> Result<Vec<Window>, opusdm_x11::X11Error> {
    let atom = conn.intern_atom(false, b"_NET_CLIENT_LIST")?.reply()?.atom;
    let reply = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, u32::MAX)?.reply()?;
    Ok(reply.value32().map(|v| v.collect()).unwrap_or_default())
}

/// The startup id, pid and class of `window`; empty fields when it is gone.
fn window_info(conn: &RustConnection, window: Window) -> Result<WindowInfo, opusdm_x11::X11Error> {
    let startup_atom = conn.intern_atom(false, b"_NET_STARTUP_ID")?.reply()?.atom;
    let pid_atom = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
    let startup = opusdm_x11::gone(conn.get_property(false, window, startup_atom, AtomEnum::ANY, 0, 1024)?.reply())?;
    let pid = opusdm_x11::gone(conn.get_property(false, window, pid_atom, AtomEnum::CARDINAL, 0, 1)?.reply())?;
    let class = opusdm_x11::gone(conn.get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)?.reply())?;
    let text = |r: Option<x11rb::protocol::xproto::GetPropertyReply>| r.filter(|r| !r.value.is_empty()).map(|r| r.value);
    Ok(WindowInfo {
        startup_id: text(startup).map(|v| String::from_utf8_lossy(&v).trim_end_matches('\0').to_string()),
        pid: pid.and_then(|r| r.value32().and_then(|mut v| v.next())),
        wm_class: text(class)
            .map(|v| v.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
            .unwrap_or_default(),
    })
}

/// The window that has the focus according to the window manager.
fn active_window(conn: &RustConnection, root: Window) -> Result<Window, opusdm_x11::X11Error> {
    let atom = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?.reply()?.atom;
    let reply = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut v| v.next()).unwrap_or(0))
}

impl Launch {
    /// The id to pass to the program in [`STARTUP_ID_ENV`].
    pub fn startup_id(&self) -> &str {
        &self.id
    }

    /// The program was started as `pid`: watch for its window on a thread
    /// of its own and end the startup sequence when done.
    pub fn watch(self, pid: u32) {
        let spawned = std::thread::Builder::new().name("opusdm-launch-focus".into()).spawn(move || {
            if let Err(e) = self.focus_new_window(pid) {
                opusdm_ipc::err_log!("hub", "launch focus: watching '{}' failed: {e}", self.bin);
            }
            let _ = self.end();
        });
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("hub", "launch focus: cannot start the watcher thread: {e}");
        }
    }

    /// The program could not be started: end the startup sequence at once.
    pub fn abort(self) {
        let _ = self.end();
    }

    /// Wait for the first new window of the launch and make sure it ends up
    /// with the focus.
    fn focus_new_window(&self, pid: u32) -> Result<(), opusdm_x11::X11Error> {
        let started = Instant::now();
        let mut seen = self.known.clone();
        while started.elapsed() < WATCH_TIMEOUT {
            for window in client_list(&self.conn, self.root)? {
                if !seen.insert(window) {
                    continue;
                }
                if belongs_to_launch(&window_info(&self.conn, window)?, &self.id, pid, &self.bin) {
                    return self.activate_unless_focused(window);
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }

    /// Give `window` a moment to receive the focus by itself, else ask the
    /// window manager to activate it as a user (pager) request.
    fn activate_unless_focused(&self, window: Window) -> Result<(), opusdm_x11::X11Error> {
        let started = Instant::now();
        while started.elapsed() < FOCUS_GRACE {
            if active_window(&self.conn, self.root)? == window {
                return Ok(());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        let current = active_window(&self.conn, self.root)?;
        let time = server_time(&self.conn, self.source)?;
        let atom = self.conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?.reply()?.atom;
        opusdm_x11::send_to_root(&self.conn, self.root, window, atom, [SOURCE_PAGER, time, current, 0, 0])?;
        opusdm_x11::x11_sync(&self.conn)?;
        Ok(())
    }

    /// Send the `remove:` message (harmless when the program already did)
    /// and drop the helper window.
    fn end(&self) -> Result<(), opusdm_x11::X11Error> {
        self.broadcast(&remove_message(&self.id))?;
        self.conn.destroy_window(self.source)?;
        opusdm_x11::x11_sync(&self.conn)?;
        Ok(())
    }

    /// Send `message` to the root window as a startup notification: a
    /// `_NET_STARTUP_INFO_BEGIN` piece followed by `_NET_STARTUP_INFO` ones.
    fn broadcast(&self, message: &str) -> Result<(), opusdm_x11::X11Error> {
        let begin = self.conn.intern_atom(false, b"_NET_STARTUP_INFO_BEGIN")?.reply()?.atom;
        let more = self.conn.intern_atom(false, b"_NET_STARTUP_INFO")?.reply()?.atom;
        for (i, chunk) in chunks(message).into_iter().enumerate() {
            let kind = if i == 0 { begin } else { more };
            let event = ClientMessageEvent::new(8, self.source, kind, chunk);
            self.conn.send_event(false, self.root, EventMask::PROPERTY_CHANGE, event)?;
        }
        opusdm_x11::x11_sync(&self.conn)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_id_carries_the_time_and_a_clean_program_name() {
        assert_eq!(startup_id(42, 3, "konsole", 7759483), "opusdm-42-3-konsole_TIME7759483");
        assert_eq!(startup_id(1, 0, "my app\"x", 5), "opusdm-1-0-myappx_TIME5");
    }

    #[test]
    fn messages_quote_their_values() {
        assert_eq!(new_message("id-1", "kon\"so\\le", 0), r#"new: ID="id-1" NAME="kon\"so\\le" BIN="kon\"so\\le" SCREEN=0"#);
        assert_eq!(remove_message("id-1"), r#"remove: ID="id-1""#);
    }

    #[test]
    fn chunks_end_with_a_nul_and_are_zero_padded() {
        let pieces = chunks("abcdefghijklmnopqrstuvwxyz");
        assert_eq!(pieces.len(), 2);
        assert_eq!(&pieces[0], b"abcdefghijklmnopqrst");
        assert_eq!(&pieces[1][..7], b"uvwxyz\0");
        assert!(pieces[1][7..].iter().all(|b| *b == 0));
        // Exactly 20 bytes of text still need a piece for the NUL.
        assert_eq!(chunks("abcdefghijklmnopqrst").len(), 2);
    }

    #[test]
    fn a_window_belongs_to_the_launch_by_id_pid_or_class() {
        let by_id = WindowInfo { startup_id: Some("id-1".into()), ..Default::default() };
        let by_pid = WindowInfo { pid: Some(77), ..Default::default() };
        let by_class = WindowInfo { pid: Some(5), wm_class: vec!["konsole".into(), "Konsole".into()], ..Default::default() };
        let other = WindowInfo { startup_id: Some("id-2".into()), pid: Some(5), wm_class: vec!["brave".into()] };
        assert!(belongs_to_launch(&by_id, "id-1", 77, "xterm"));
        assert!(belongs_to_launch(&by_pid, "id-1", 77, "xterm"));
        assert!(belongs_to_launch(&by_class, "id-1", 77, "KONSOLE"));
        assert!(!belongs_to_launch(&other, "id-1", 77, "konsole"));
        assert!(!belongs_to_launch(&WindowInfo::default(), "id-1", 77, ""));
    }

    #[test]
    fn the_program_name_is_the_file_name() {
        assert_eq!(bin_name("/usr/bin/konsole"), "konsole");
        assert_eq!(bin_name("xterm"), "xterm");
    }
}
