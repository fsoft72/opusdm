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

// --------------------------------------------------------------------------
// Protocol thread
// --------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::mpsc;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux, ConnectionExt,
    CreateWindowAux, EventMask, PropMode, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_FROM_PARENT, CURRENT_TIME};

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        MANAGER,
        _NET_SYSTEM_TRAY_OPCODE,
        _NET_SYSTEM_TRAY_ORIENTATION,
        _XEMBED,
    }
}

/// Snapshots the tray thread sends to the GTK side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// A new icon was docked, appended to the end of the display order.
    IconDocked(u32),
    /// A docked icon's owner withdrew it or exited.
    IconRemoved(u32),
}

/// Commands the GTK side sends to the tray thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayCmd {
    /// Move/resize a docked icon's container window to this root-relative rect.
    PlaceIcon { xid: u32, x: i32, y: i32, w: i32, h: i32 },
}

/// Sender half of the command channel. Sending also wakes the tray thread,
/// which sleeps in `poll(2)` on the X connection and this eventfd instead of
/// waking up on a timer.
#[derive(Clone)]
pub struct TraySender {
    tx: mpsc::Sender<TrayCmd>,
    wake: Arc<OwnedFd>,
}

impl TraySender {
    /// Queue `cmd` and wake the tray thread. Fails only when the thread has
    /// ended (the command channel is closed).
    pub fn send(&self, cmd: TrayCmd) -> Result<(), mpsc::SendError<TrayCmd>> {
        self.tx.send(cmd)?;
        let one: u64 = 1;
        // A failed write only means a wake-up is already pending (counter full).
        let _ = unsafe { libc::write(self.wake.as_raw_fd(), (&one as *const u64).cast(), 8) };
        Ok(())
    }
}

/// A non-blocking eventfd the tray thread can `poll(2)` on.
fn new_wake_fd() -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Block until `x_fd` has data to read or `wake` was signalled, then clear
/// the wake signal.
fn wait_for_work(x_fd: RawFd, wake: &OwnedFd) {
    let mut fds = [
        libc::pollfd { fd: x_fd, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
    ];
    // EINTR just means "look again", which the caller's loop does anyway.
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
    let mut counter: u64 = 0;
    let _ = unsafe { libc::read(wake.as_raw_fd(), (&mut counter as *mut u64).cast(), 8) };
}

/// Start the tray thread. Returns a command sender and event receiver; if no
/// X11 connection is available, or the tray selection is already owned by
/// another process, the thread logs once and exits - the channels stay
/// open but nothing is ever sent or received.
pub fn spawn(panel_height: i32) -> (TraySender, async_channel::Receiver<TrayEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<TrayCmd>();
    let (evt_tx, evt_rx) = async_channel::unbounded::<TrayEvent>();
    let wake = Arc::new(new_wake_fd().expect("create tray wake eventfd"));
    let thread_wake = wake.clone();
    std::thread::Builder::new()
        .name("opusdm-panel-systray".into())
        .spawn(move || {
            if let Err(e) = run(panel_height, cmd_rx, &thread_wake, evt_tx) {
                eprintln!("opusdm-panel: system tray stopped: {e}");
            }
        })
        .expect("spawn systray thread");
    (TraySender { tx: cmd_tx, wake }, evt_rx)
}

fn run(
    panel_height: i32,
    cmd_rx: mpsc::Receiver<TrayCmd>,
    wake: &OwnedFd,
    tx: async_channel::Sender<TrayEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let atoms = Atoms::new(&conn)?.reply()?;

    let selection_name = format!("_NET_SYSTEM_TRAY_S{screen_num}");
    let selection_atom = conn.intern_atom(false, selection_name.as_bytes())?.reply()?.atom;

    let owner = conn.get_selection_owner(selection_atom)?.reply()?.owner;
    if owner != 0 {
        eprintln!("opusdm-panel: system tray already running, tray disabled");
        return Ok(());
    }

    let manager = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        manager,
        root,
        -1,
        -1,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        COPY_FROM_PARENT,
        &CreateWindowAux::new(),
    )?;
    conn.set_selection_owner(manager, selection_atom, CURRENT_TIME)?;
    conn.change_property32(
        PropMode::REPLACE,
        manager,
        atoms._NET_SYSTEM_TRAY_ORIENTATION,
        x11rb::protocol::xproto::AtomEnum::CARDINAL,
        &[0], // horizontal
    )?;
    let ev = ClientMessageEvent::new(
        32,
        root,
        atoms.MANAGER,
        manager_message_data(CURRENT_TIME, selection_atom, manager),
    );
    conn.send_event(false, root, EventMask::STRUCTURE_NOTIFY, ev)?;
    conn.change_window_attributes(
        manager,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )?;
    conn.flush()?;

    let icon_size = icon_square_size(panel_height);
    let mut containers: HashMap<u32, u32> = HashMap::new();

    event_loop(&conn, root, &atoms, manager, icon_size, &mut containers, &cmd_rx, wake, &tx)
}

#[allow(clippy::too_many_arguments)]
fn event_loop(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    manager: Window,
    icon_size: i32,
    containers: &mut HashMap<u32, u32>,
    cmd_rx: &mpsc::Receiver<TrayCmd>,
    wake: &OwnedFd,
    tx: &async_channel::Sender<TrayEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let x_fd = conn.stream().as_raw_fd();
    loop {
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::ClientMessage(e)
                    if e.window == manager && e.type_ == atoms._NET_SYSTEM_TRAY_OPCODE =>
                {
                    if let Some(icon_xid) = parse_dock_request(e.data.as_data32()) {
                        dock_icon(conn, atoms, containers, root, icon_size, icon_xid, tx);
                    }
                }
                Event::DestroyNotify(e) if containers.contains_key(&e.window) => {
                    undock_icon(conn, containers, e.window, tx);
                }
                Event::UnmapNotify(e) if containers.contains_key(&e.window) => {
                    undock_icon(conn, containers, e.window, tx);
                }
                Event::ReparentNotify(e)
                    if containers.get(&e.window).is_some_and(|c| *c != e.parent) =>
                {
                    undock_icon(conn, containers, e.window, tx);
                }
                _ => {}
            }
        }
        loop {
            match cmd_rx.try_recv() {
                Ok(TrayCmd::PlaceIcon { xid, x, y, w, h }) => place_icon(conn, containers, xid, x, y, w, h)?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        // Everything pending was handled above, so sleep until the X server
        // or a sender has something new (no timer: an idle desktop wakes
        // this thread zero times).
        wait_for_work(x_fd, wake);
    }
}

/// Handle a `SYSTEM_TRAY_REQUEST_DOCK`: create a small container window,
/// reparent the client's icon into it, complete the XEmbed handshake, and
/// map both. Errors for one icon are logged and swallowed so a single
/// misbehaving client can't take down the tray thread.
fn dock_icon(
    conn: &RustConnection,
    atoms: &Atoms,
    containers: &mut HashMap<u32, u32>,
    root: Window,
    icon_size: i32,
    icon_xid: u32,
    tx: &async_channel::Sender<TrayEvent>,
) {
    let run = || -> Result<Window, Box<dyn std::error::Error>> {
        let container = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            container,
            root,
            0,
            0,
            icon_size.max(1) as u16,
            icon_size.max(1) as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().override_redirect(1),
        )?;
        conn.change_window_attributes(
            icon_xid,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )?;
        conn.reparent_window(icon_xid, container, 0, 0)?;
        conn.configure_window(
            icon_xid,
            &ConfigureWindowAux::new()
                .width(icon_size.max(1) as u32)
                .height(icon_size.max(1) as u32),
        )?;
        let ev = ClientMessageEvent::new(
            32,
            icon_xid,
            atoms._XEMBED,
            xembed_embedded_notify_data(container, 0),
        );
        conn.send_event(false, icon_xid, EventMask::NO_EVENT, ev)?;
        conn.map_window(icon_xid)?;
        conn.map_window(container)?;
        conn.flush()?;
        Ok(container)
    };
    match run() {
        Ok(container) => {
            containers.insert(icon_xid, container);
            let _ = tx.send_blocking(TrayEvent::IconDocked(icon_xid));
        }
        Err(e) => eprintln!("opusdm-panel: cannot dock tray icon {icon_xid}: {e}"),
    }
}

/// Destroy `icon_xid`'s container (if any tracked) and report its removal.
fn undock_icon(
    conn: &RustConnection,
    containers: &mut HashMap<u32, u32>,
    icon_xid: u32,
    tx: &async_channel::Sender<TrayEvent>,
) {
    if let Some(container) = containers.remove(&icon_xid) {
        let _ = conn.destroy_window(container);
        let _ = conn.flush();
        let _ = tx.send_blocking(TrayEvent::IconRemoved(icon_xid));
    }
}

/// Move/resize a docked icon's container window. A no-op if `xid` isn't
/// currently docked (e.g. it was removed just before the command arrived).
fn place_icon(
    conn: &RustConnection,
    containers: &HashMap<u32, u32>,
    xid: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(&container) = containers.get(&xid) else { return Ok(()) };
    conn.configure_window(
        container,
        &ConfigureWindowAux::new().x(x).y(y).width(w as u32).height(h as u32),
    )?;
    conn.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command sent from another thread wakes a thread sleeping in
    /// `wait_for_work`, and the wake signal is cleared afterwards.
    #[test]
    fn sending_a_command_wakes_the_sleeping_tray_thread() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<TrayCmd>();
        let wake = Arc::new(new_wake_fd().unwrap());
        let sender = TraySender { tx: cmd_tx, wake: wake.clone() };
        // A socket nobody writes to stands in for the quiet X connection.
        let (quiet, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let thread_wake = wake.clone();
        std::thread::spawn(move || {
            wait_for_work(quiet.as_raw_fd(), &thread_wake);
            let _ = done_tx.send(cmd_rx.try_recv());
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(done_rx.try_recv().is_err(), "must still be asleep with nothing to do");
        sender.send(TrayCmd::PlaceIcon { xid: 1, x: 2, y: 3, w: 4, h: 5 }).unwrap();
        let got = done_rx.recv_timeout(std::time::Duration::from_secs(2)).expect("tray thread woke up");
        assert_eq!(got, Ok(TrayCmd::PlaceIcon { xid: 1, x: 2, y: 3, w: 4, h: 5 }));
        let mut counter: u64 = 0;
        let read = unsafe { libc::read(wake.as_raw_fd(), (&mut counter as *mut u64).cast(), 8) };
        assert_eq!(read, -1, "the wake signal was consumed");
    }

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
