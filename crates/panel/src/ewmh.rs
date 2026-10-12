//! EWMH observer: a background thread owns an X11 connection, watches the
//! root window for `_NET_*` property changes and the client windows for title
//! and state changes, and pushes typed snapshots onto an `async-channel` the
//! GTK loop drains. Click actions open a short-lived connection each and send
//! the matching `ClientMessage` (same throw-away style as the hub's
//! `pin_geometry`).

use std::collections::HashSet;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, Window,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::CURRENT_TIME;

/// ICCCM `IconicState`, the argument to `WM_CHANGE_STATE` that asks the window
/// manager to minimise a window.
const ICONIC_STATE: u32 = 3;

/// "On all desktops" sentinel from `_NET_WM_DESKTOP` / `_NET_CURRENT_DESKTOP`.
const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        _NET_CURRENT_DESKTOP,
        _NET_NUMBER_OF_DESKTOPS,
        _NET_DESKTOP_NAMES,
        _NET_CLIENT_LIST,
        _NET_ACTIVE_WINDOW,
        _NET_WM_NAME,
        _NET_WM_DESKTOP,
        _NET_WM_PID,
        _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_WM_ICON,
        UTF8_STRING,
        WM_CHANGE_STATE,
    }
}

/// A managed top-level window, as the panel needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientWin {
    pub xid: u32,
    pub title: String,
    /// Virtual desktop index, or `-1` for "all desktops".
    pub desktop: i64,
    pub minimized: bool,
    /// Owner process from `_NET_WM_PID`, when the client sets it.
    pub pid: Option<u32>,
    pub icon: Option<IconRgba>,
    /// Wayland app id (the desktop file name), used for the icon when there
    /// is no `_NET_WM_ICON`. Always `None` on X11.
    pub app_id: Option<String>,
    /// Position and size of the client area in root coordinates.
    pub rect: WinRect,
}

/// A window rectangle in root (screen) coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WinRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// A window icon decoded from `_NET_WM_ICON` into tightly packed RGBA8.
#[derive(Clone, Debug, PartialEq)]
pub struct IconRgba {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// Snapshots the observer thread sends to the GTK side.
#[derive(Clone, Debug)]
pub enum EwmhEvent {
    /// Desktop count, current index and names, sent together on any change.
    Desktops { count: u32, current: u32, names: Vec<String> },
    /// Only the current desktop moved.
    CurrentDesktop(u32),
    /// The full client list was recomputed.
    Clients(Vec<ClientWin>),
    /// The active window changed (`None` when nothing is active).
    ActiveWindow(Option<u32>),
    /// Windows moved or were resized: `(xid, rect)` for every managed client.
    Geometries(Vec<(u32, WinRect)>),
}

// --------------------------------------------------------------------------
// Pure parsers
// --------------------------------------------------------------------------

/// Split a `_NET_DESKTOP_NAMES` blob (NUL-separated UTF-8) into names. A
/// trailing NUL does not produce an empty final entry.
pub fn parse_desktop_names(raw: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|b| *b == 0)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect();
    if out.last().map(|s| s.is_empty()).unwrap_or(false) {
        out.pop();
    }
    out
}

/// Decode a `_NET_CLIENT_LIST` blob (array of 32-bit little-endian window
/// ids). Trailing bytes that do not fill a whole id are ignored.
pub fn parse_window_list(raw: &[u8]) -> Vec<u32> {
    raw.as_chunks::<4>().0.iter()
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Pick the icon closest to `target` px from a `_NET_WM_ICON` value (repeating
/// `width, height, width*height` ARGB pixels) and unpack it to RGBA8.
pub fn pick_icon(words: &[u32], target: i32) -> Option<IconRgba> {
    let mut i = 0;
    let mut best: Option<(i32, usize, usize)> = None; // (score, start, count)
    while i + 2 <= words.len() {
        let w = words[i] as i32;
        let h = words[i + 1] as i32;
        let count = (w as i64 * h as i64) as usize;
        if w <= 0 || h <= 0 || i + 2 + count > words.len() {
            break;
        }
        let score = (w - target).abs();
        if best.map(|(bs, _, _)| score < bs).unwrap_or(true) {
            best = Some((score, i + 2, count));
        }
        i += 2 + count;
    }
    let (_, start, count) = best?;
    let w = words[start - 2] as i32;
    let h = words[start - 1] as i32;
    let mut rgba = Vec::with_capacity(count * 4);
    for &px in &words[start..start + count] {
        let a = (px >> 24) & 0xff;
        let r = (px >> 16) & 0xff;
        let g = (px >> 8) & 0xff;
        let b = px & 0xff;
        rgba.extend_from_slice(&[r as u8, g as u8, b as u8, a as u8]);
    }
    Some(IconRgba { width: w, height: h, rgba })
}

// --------------------------------------------------------------------------
// Observer thread
// --------------------------------------------------------------------------

/// Start the observer thread. Returns the receiver of snapshots; the thread
/// runs until the X connection drops or the receiver is closed.
pub fn spawn() -> async_channel::Receiver<EwmhEvent> {
    let (tx, rx) = async_channel::unbounded::<EwmhEvent>();
    if !opusdm_ipc::session::is_x11() {
        crate::wl_windows::spawn(tx);
        return rx;
    }
    std::thread::Builder::new()
        .name("opusdm-panel-ewmh".into())
        .spawn(move || {
            if let Err(e) = observe(&tx) {
                opusdm_ipc::err_log!("opusdm-panel", "EWMH observer stopped: {e}");
            }
        })
        .expect("spawn ewmh thread");
    rx
}

fn observe(tx: &async_channel::Sender<EwmhEvent>) -> Result<(), Box<dyn std::error::Error>> {
    let opusdm_x11::Session { conn, root, .. } = opusdm_x11::connect()?;
    let atoms = Atoms::new(&conn)?.reply()?;

    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new()
            .event_mask(EventMask::PROPERTY_CHANGE | EventMask::SUBSTRUCTURE_NOTIFY),
    )?;
    conn.flush()?;

    let mut watched: HashSet<Window> = HashSet::new();
    emit_desktops(&conn, root, &atoms, tx);
    emit_clients(&conn, root, &atoms, &mut watched, tx);
    emit_active(&conn, root, &atoms, tx);

    let mut pending: Option<Event> = None;
    loop {
        let event = match pending.take() {
            Some(ev) => ev,
            None => conn.wait_for_event()?,
        };
        if let Event::ConfigureNotify(e) = &event {
            if !watched.contains(&e.window) {
                continue;
            }
            // A window drag sends a burst: collapse it into one snapshot.
            while let Some(next) = conn.poll_for_event()? {
                if !matches!(next, Event::ConfigureNotify(_)) {
                    pending = Some(next);
                    break;
                }
            }
            emit_geometries(&conn, root, &atoms, &watched, tx);
            continue;
        }
        let Event::PropertyNotify(e) = event else { continue };
        if e.window == root {
            if e.atom == atoms._NET_CURRENT_DESKTOP {
                if let Some(n) = get_card(&conn, root, atoms._NET_CURRENT_DESKTOP) {
                    let _ = tx.send_blocking(EwmhEvent::CurrentDesktop(n));
                }
            } else if e.atom == atoms._NET_NUMBER_OF_DESKTOPS
                || e.atom == atoms._NET_DESKTOP_NAMES
            {
                emit_desktops(&conn, root, &atoms, tx);
            } else if e.atom == atoms._NET_CLIENT_LIST {
                emit_clients(&conn, root, &atoms, &mut watched, tx);
            } else if e.atom == atoms._NET_ACTIVE_WINDOW {
                emit_active(&conn, root, &atoms, tx);
            }
        } else if e.atom == atoms._NET_WM_NAME
            || e.atom == u32::from(AtomEnum::WM_NAME)
            || e.atom == atoms._NET_WM_STATE
            || e.atom == atoms._NET_WM_DESKTOP
        {
            emit_clients(&conn, root, &atoms, &mut watched, tx);
        }
    }
}

fn emit_desktops(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    tx: &async_channel::Sender<EwmhEvent>,
) {
    let count = get_card(conn, root, atoms._NET_NUMBER_OF_DESKTOPS).unwrap_or(1);
    let current = get_card(conn, root, atoms._NET_CURRENT_DESKTOP).unwrap_or(0);
    let names = get_bytes(conn, root, atoms._NET_DESKTOP_NAMES)
        .map(|b| parse_desktop_names(&b))
        .unwrap_or_default();
    let _ = tx.send_blocking(EwmhEvent::Desktops { count, current, names });
}

/// Send the current rectangle of every watched, still existing client.
fn emit_geometries(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    watched: &HashSet<Window>,
    tx: &async_channel::Sender<EwmhEvent>,
) {
    let Some(raw) = get_bytes(conn, root, atoms._NET_CLIENT_LIST) else { return };
    let rects: Vec<(u32, WinRect)> = parse_window_list(&raw)
        .into_iter()
        .filter(|xid| watched.contains(xid))
        .filter_map(|xid| read_rect(conn, root, xid).map(|r| (xid, r)))
        .collect();
    let _ = tx.send_blocking(EwmhEvent::Geometries(rects));
}

/// Read the client area of `xid` in root coordinates; `None` if it vanished.
fn read_rect(conn: &RustConnection, root: Window, xid: Window) -> Option<WinRect> {
    let geo = conn.get_geometry(xid).ok()?.reply().ok()?;
    let pos = conn.translate_coordinates(xid, root, 0, 0).ok()?.reply().ok()?;
    Some(WinRect {
        x: i32::from(pos.dst_x),
        y: i32::from(pos.dst_y),
        w: u32::from(geo.width),
        h: u32::from(geo.height),
    })
}

fn emit_active(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    tx: &async_channel::Sender<EwmhEvent>,
) {
    let active = get_card(conn, root, atoms._NET_ACTIVE_WINDOW).filter(|w| *w != 0);
    let _ = tx.send_blocking(EwmhEvent::ActiveWindow(active));
}

fn emit_clients(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    watched: &mut HashSet<Window>,
    tx: &async_channel::Sender<EwmhEvent>,
) {
    let Some(raw) = get_bytes(conn, root, atoms._NET_CLIENT_LIST) else {
        return;
    };
    let ids = parse_window_list(&raw);
    let mut clients = Vec::with_capacity(ids.len());
    for xid in ids {
        if watched.insert(xid) {
            let _ = conn.change_window_attributes(
                xid,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
            );
        }
        if let Some(c) = read_client(conn, root, xid, atoms) {
            clients.push(c);
        }
    }
    let _ = tx.send_blocking(EwmhEvent::Clients(clients));
}

/// Read the panel-relevant properties of one client window. Returns `None`
/// for windows we never show (docks, the desktop, windows that ask to be
/// left out of the taskbar) or that vanished.
fn read_client(conn: &RustConnection, root: Window, xid: Window, atoms: &Atoms) -> Option<ClientWin> {
    // Skip docks and the desktop.
    if let Some(types) = get_atoms(conn, xid, atoms._NET_WM_WINDOW_TYPE) {
        if types.contains(&atoms._NET_WM_WINDOW_TYPE_DOCK)
            || types.contains(&atoms._NET_WM_WINDOW_TYPE_DESKTOP)
        {
            return None;
        }
    }

    let title = get_string(conn, xid, atoms._NET_WM_NAME)
        .filter(|s| !s.is_empty())
        .or_else(|| get_string(conn, xid, AtomEnum::WM_NAME.into()))
        .unwrap_or_default();

    let desktop = match get_card(conn, xid, atoms._NET_WM_DESKTOP) {
        Some(ALL_DESKTOPS) => -1,
        Some(n) => n as i64,
        None => -1,
    };

    let states = get_atoms(conn, xid, atoms._NET_WM_STATE).unwrap_or_default();
    if states.contains(&atoms._NET_WM_STATE_SKIP_TASKBAR) {
        return None;
    }
    let minimized = states.contains(&atoms._NET_WM_STATE_HIDDEN);

    let icon = get_card_array(conn, xid, atoms._NET_WM_ICON)
        .and_then(|words| pick_icon(&words, 24));

    let pid = get_card(conn, xid, atoms._NET_WM_PID).filter(|p| *p != 0);

    let rect = read_rect(conn, root, xid).unwrap_or_default();

    Some(ClientWin { xid, title, desktop, minimized, pid, icon, app_id: None, rect })
}

// --------------------------------------------------------------------------
// Property helpers
// --------------------------------------------------------------------------

fn get_bytes(conn: &RustConnection, win: Window, prop: u32) -> Option<Vec<u8>> {
    let r = conn
        .get_property(false, win, prop, AtomEnum::ANY, 0, u32::MAX)
        .ok()?
        .reply()
        .ok()?;
    if r.value.is_empty() {
        None
    } else {
        Some(r.value)
    }
}

fn get_card(conn: &RustConnection, win: Window, prop: u32) -> Option<u32> {
    let r = conn
        .get_property(false, win, prop, AtomEnum::ANY, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    r.value32().and_then(|mut it| it.next())
}

fn get_card_array(conn: &RustConnection, win: Window, prop: u32) -> Option<Vec<u32>> {
    let r = conn
        .get_property(false, win, prop, AtomEnum::ANY, 0, u32::MAX)
        .ok()?
        .reply()
        .ok()?;
    r.value32().map(|it| it.collect())
}

fn get_atoms(conn: &RustConnection, win: Window, prop: u32) -> Option<Vec<u32>> {
    get_card_array(conn, win, prop)
}

fn get_string(conn: &RustConnection, win: Window, prop: u32) -> Option<String> {
    let bytes = get_bytes(conn, win, prop)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// --------------------------------------------------------------------------
// Click actions (short-lived connection each)
// --------------------------------------------------------------------------

/// Run `f` on the shared connection; a failure is logged, never raised.
/// On Wayland the actions go through `crate::wl_windows` instead.
fn with_root(f: impl FnOnce(&opusdm_ui::X11) -> Result<(), opusdm_ui::X11Error>) {
    if let Err(e) = opusdm_ui::with_x11(f) {
        opusdm_ipc::err_log!("opusdm-panel", "X action failed: {e}");
    }
}

/// Switch to virtual desktop `index`.
pub fn switch_desktop(index: u32) {
    if crate::wl_windows::switch_desktop(index) { return; }
    with_root(|x11| x11.send_to_root(x11.root(), "_NET_CURRENT_DESKTOP", [index, CURRENT_TIME, 0, 0, 0]));
}

/// Raise and focus `xid` (pager source indication).
pub fn activate_window(xid: u32) {
    if crate::wl_windows::activate_window(xid) { return; }
    with_root(|x11| x11.send_to_root(xid, "_NET_ACTIVE_WINDOW", [2, CURRENT_TIME, 0, 0, 0]));
}

/// Minimise `xid` via ICCCM `WM_CHANGE_STATE`.
pub fn iconify_window(xid: u32) {
    if crate::wl_windows::iconify_window(xid) { return; }
    with_root(|x11| x11.send_to_root(xid, "WM_CHANGE_STATE", [ICONIC_STATE, 0, 0, 0, 0]));
}

/// Ask `xid` to close via `_NET_CLOSE_WINDOW`, like its close button (pager
/// source indication; the window manager sends `WM_DELETE_WINDOW`).
pub fn close_window(xid: u32) {
    if crate::wl_windows::close_window(xid) { return; }
    with_root(|x11| x11.send_to_root(xid, "_NET_CLOSE_WINDOW", [CURRENT_TIME, 2, 0, 0, 0]));
}

/// Move `xid` to virtual desktop `index` (pager source indication).
pub fn move_window_to_desktop(xid: u32, index: u32) {
    if crate::wl_windows::move_window_to_desktop(xid, index) { return; }
    with_root(|x11| x11.send_to_root(xid, "_NET_WM_DESKTOP", [index, 2, 0, 0, 0]));
}

/// True when `pid` is a process the panel may kill: never 0/1 and never the
/// panel's own process (`own`).
pub fn is_killable(pid: u32, own: u32) -> bool {
    pid > 1 && pid != own
}

/// Send `SIGKILL` to `pid`. A refused or failed kill is logged, never raised.
pub fn force_kill(pid: u32) {
    if !is_killable(pid, std::process::id()) {
        opusdm_ipc::err_log!("opusdm-panel", "refusing to kill pid {pid}");
        return;
    }
    // SAFETY: `kill` takes plain integers and has no memory-safety contract.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } != 0 {
        opusdm_ipc::err_log!("opusdm-panel", "kill -9 {pid} failed: {}", std::io::Error::last_os_error());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn killable_excludes_init_zero_and_self() {
        assert!(!is_killable(0, 10));
        assert!(!is_killable(1, 10));
        assert!(!is_killable(10, 10));
        assert!(is_killable(11, 10));
    }

    #[test]
    fn desktop_names_split_on_nul_without_trailing_empty() {
        let raw = b"one\0two\0three\0";
        assert_eq!(parse_desktop_names(raw), vec!["one", "two", "three"]);
    }

    #[test]
    fn desktop_names_keep_interior_empty() {
        let raw = b"one\0\0three\0";
        assert_eq!(parse_desktop_names(raw), vec!["one", "", "three"]);
    }

    #[test]
    fn window_list_reads_le_u32s_and_ignores_a_short_tail() {
        let raw = [0x21, 0x04, 0, 0, 0x40, 0, 0, 0, 0xAA];
        assert_eq!(parse_window_list(&raw), vec![0x421, 0x40]);
    }

    #[test]
    fn pick_icon_prefers_the_size_closest_to_target() {
        // Two icons: 1x1 red, 2x1 blue+green.
        let mut words = vec![1, 1, 0xFFFF_0000];
        words.extend_from_slice(&[2, 1, 0xFF00_00FF, 0xFF00_FF00]);
        let icon = pick_icon(&words, 2).unwrap();
        assert_eq!((icon.width, icon.height), (2, 1));
        // First pixel unpacked from 0xFF0000FF -> R0 G0 B255 A255.
        assert_eq!(&icon.rgba[0..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn pick_icon_none_when_empty_or_truncated() {
        assert!(pick_icon(&[], 16).is_none());
        assert!(pick_icon(&[4, 4, 1, 2, 3], 16).is_none()); // says 16 px, has 3
    }
}
