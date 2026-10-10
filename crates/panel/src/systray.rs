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

/// How many times a docked icon is resized back to fill its container before
/// the tray gives up and only centres it, so a client that insists on its
/// own size can't start an endless resize ping-pong.
pub const MAX_FORCED_RESIZES: u32 = 5;

/// The geometry `(x, y, width, height)` to give a docked icon now at
/// `icon` (x, y, width, height inside its container) in a container of
/// `container` size, or `None` when it is already right. The icon is made
/// to fill the container (GTK3 icons resize themselves back to 16 px after
/// docking and then draw in the top left corner); once `forced` reaches
/// `MAX_FORCED_RESIZES` it keeps its size and is centred instead.
pub fn icon_fit(container: (u32, u32), icon: (i32, i32, u32, u32), forced: u32) -> Option<(i32, i32, u32, u32)> {
    let (cw, ch) = container;
    let (x, y, w, h) = icon;
    let target = if forced < MAX_FORCED_RESIZES {
        (0, 0, cw, ch)
    } else {
        ((cw as i32 - w as i32) / 2, (ch as i32 - h as i32) / 2, w, h)
    };
    (target != (x, y, w, h)).then_some(target)
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
    CreateWindowAux, EventMask, PropMode, SetMode, StackMode, Window, WindowClass,
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
        _NET_SYSTEM_TRAY_ICON_SIZE,
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

/// One docked icon: the container window it was reparented into, that
/// container's current size and how many times the icon was resized back
/// to fill it (see `icon_fit`).
struct Docked {
    container: Window,
    size: (u32, u32),
    forced: u32,
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
            // XEmbed does not exist on Wayland: the SNI tray is the only one.
            if opusdm_ipc::session::is_x11() {
                if let Err(e) = run(panel_height, cmd_rx, &thread_wake, evt_tx) {
                    opusdm_ipc::err_log!("opusdm-panel", "system tray stopped: {e}");
                }
            }
            // Also covers a tray that never started: nobody should wait for it.
            crate::tray_ready::mark_ready(crate::tray_ready::TrayKind::XEmbed);
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
    let opusdm_x11::Session { conn, screen_num, root } = opusdm_x11::connect()?;
    let atoms = Atoms::new(&conn)?.reply()?;

    let selection_name = format!("_NET_SYSTEM_TRAY_S{screen_num}");
    let selection_atom = conn.intern_atom(false, selection_name.as_bytes())?.reply()?.atom;

    let owner = conn.get_selection_owner(selection_atom)?.reply()?.owner;
    if owner != 0 {
        opusdm_ipc::err_log!("opusdm-panel", "system tray already running, tray disabled");
        crate::tray_ready::mark_ready(crate::tray_ready::TrayKind::XEmbed);
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
    // Tells GTK3 (and other recent clients) what size to draw at, instead
    // of drawing a default 16 px icon in the corner of the container.
    conn.change_property32(
        PropMode::REPLACE,
        manager,
        atoms._NET_SYSTEM_TRAY_ICON_SIZE,
        x11rb::protocol::xproto::AtomEnum::CARDINAL,
        &[icon_square_size(panel_height).max(1) as u32],
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
    // Icons can dock from now on.
    crate::tray_ready::mark_ready(crate::tray_ready::TrayKind::XEmbed);

    let icon_size = icon_square_size(panel_height);
    let mut containers: HashMap<u32, Docked> = HashMap::new();

    event_loop(&conn, root, &atoms, manager, icon_size, &mut containers, &cmd_rx, wake, &tx)
}

#[allow(clippy::too_many_arguments)]
fn event_loop(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    manager: Window,
    icon_size: i32,
    containers: &mut HashMap<u32, Docked>,
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
                    undock_icon(conn, root, containers, e.window, false, tx);
                }
                Event::UnmapNotify(e) if containers.contains_key(&e.window) => {
                    undock_icon(conn, root, containers, e.window, true, tx);
                }
                Event::ReparentNotify(e)
                    if containers.get(&e.window).is_some_and(|d| d.container != e.parent) =>
                {
                    undock_icon(conn, root, containers, e.window, false, tx);
                }
                Event::ConfigureNotify(e) if e.event == e.window && containers.contains_key(&e.window) => {
                    let icon = (i32::from(e.x), i32::from(e.y), u32::from(e.width), u32::from(e.height));
                    fit_icon(conn, containers, e.window, icon)?;
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
///
/// A repeated request for an icon that is already docked is ignored: a
/// second container would stay mapped at the first one's old place, on top
/// of whatever moved there. The icon is reparented *before* we select its
/// structure events: reparenting a mapped window unmaps and remaps it, and
/// seeing that `UnmapNotify` would undock the icon we just docked. The
/// selection is a checked request, so an icon that died before docking
/// fails here instead of leaving a container that never goes away.
fn dock_icon(
    conn: &RustConnection,
    atoms: &Atoms,
    containers: &mut HashMap<u32, Docked>,
    root: Window,
    icon_size: i32,
    icon_xid: u32,
    tx: &async_channel::Sender<TrayEvent>,
) {
    if containers.contains_key(&icon_xid) || containers.values().any(|d| d.container == icon_xid) {
        return;
    }
    let Ok(container) = conn.generate_id() else { return };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
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
            // Without a background the parts the icon leaves transparent
            // (icons with a ParentRelative background, like GTK3's) show
            // whatever was on screen there before, which looks like another
            // icon drawn over this one.
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(crate::window::PANEL_BACKGROUND),
        )?;
        // Save-set: if the panel exits or crashes, the X server moves the
        // icon back to the root window instead of destroying it with its
        // container, so the app survives and can dock again.
        conn.change_save_set(SetMode::INSERT, icon_xid)?;
        conn.reparent_window(icon_xid, container, 0, 0)?.check()?;
        conn.change_window_attributes(
            icon_xid,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )?
        .check()?;
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
        Ok(())
    };
    match run() {
        Ok(()) => {
            let side = icon_size.max(1) as u32;
            containers.insert(icon_xid, Docked { container, size: (side, side), forced: 0 });
            let _ = tx.send_blocking(TrayEvent::IconDocked(icon_xid));
        }
        Err(e) => {
            let _ = conn.destroy_window(container);
            let _ = conn.flush();
            opusdm_ipc::err_log!("opusdm-panel", "cannot dock tray icon {icon_xid}: {e}");
        }
    }
}

/// Destroy `icon_xid`'s container (if any tracked) and report its removal.
/// When the icon is `still_inside` the container (it was only unmapped) it
/// is moved back to `root` first: destroying a window destroys its
/// children, and the icon window belongs to the app.
fn undock_icon(
    conn: &RustConnection,
    root: Window,
    containers: &mut HashMap<u32, Docked>,
    icon_xid: u32,
    still_inside: bool,
    tx: &async_channel::Sender<TrayEvent>,
) {
    if let Some(Docked { container, .. }) = containers.remove(&icon_xid) {
        if still_inside {
            let _ = conn.reparent_window(icon_xid, root, 0, 0);
            let _ = conn.change_save_set(SetMode::DELETE, icon_xid);
        }
        let _ = conn.destroy_window(container);
        let _ = conn.flush();
        let _ = tx.send_blocking(TrayEvent::IconRemoved(icon_xid));
    }
}

/// Move/resize a docked icon's container window and raise it, so it stays
/// above the panel after the panel itself was raised; a new size is passed
/// on to the icon. A no-op if `xid` isn't currently docked (e.g. it was
/// removed just before the command arrived).
fn place_icon(
    conn: &RustConnection,
    containers: &mut HashMap<u32, Docked>,
    xid: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(docked) = containers.get_mut(&xid) else { return Ok(()) };
    let size = (w.max(1) as u32, h.max(1) as u32);
    let resized = docked.size != size;
    docked.size = size;
    conn.configure_window(
        docked.container,
        &ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(w.max(1) as u32)
            .height(h.max(1) as u32)
            .stack_mode(StackMode::ABOVE),
    )?;
    conn.flush()?;
    if resized {
        // The icon's own ConfigureNotify will not come for a container
        // change, so ask for its geometry here.
        if let Some(geo) = opusdm_x11::gone(conn.get_geometry(xid)?.reply())? {
            let icon = (i32::from(geo.x), i32::from(geo.y), u32::from(geo.width), u32::from(geo.height));
            fit_icon(conn, containers, xid, icon)?;
        }
    }
    Ok(())
}

/// Give the docked icon `xid`, now at `icon` inside its container, the
/// geometry `icon_fit` asks for, counting the forced resizes.
fn fit_icon(
    conn: &RustConnection,
    containers: &mut HashMap<u32, Docked>,
    xid: u32,
    icon: (i32, i32, u32, u32),
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(docked) = containers.get_mut(&xid) else { return Ok(()) };
    let Some((x, y, w, h)) = icon_fit(docked.size, icon, docked.forced) else { return Ok(()) };
    if (w, h) == docked.size {
        docked.forced += 1;
    }
    conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y).width(w).height(h))?;
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
    fn icon_fit_grows_a_small_icon_to_the_container() {
        assert_eq!(icon_fit((24, 24), (0, 0, 16, 16), 0), Some((0, 0, 24, 24)));
    }

    #[test]
    fn icon_fit_leaves_a_fitting_icon_alone() {
        assert_eq!(icon_fit((24, 24), (0, 0, 24, 24), 0), None);
        assert_eq!(icon_fit((24, 24), (4, 4, 16, 16), MAX_FORCED_RESIZES), None);
    }

    #[test]
    fn icon_fit_centres_an_icon_that_keeps_resizing_back() {
        assert_eq!(icon_fit((24, 24), (0, 0, 16, 16), MAX_FORCED_RESIZES), Some((4, 4, 16, 16)));
        // A bigger icon is centred too (negative offsets crop it evenly).
        assert_eq!(icon_fit((24, 24), (0, 0, 32, 32), MAX_FORCED_RESIZES), Some((-4, -4, 32, 32)));
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
