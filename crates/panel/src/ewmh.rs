//! EWMH observer: a background thread owns an X11 connection, watches the
//! root window for `_NET_*` property changes and the client windows for title
//! and state changes, and pushes typed snapshots onto an `async-channel` the
//! GTK loop drains. Click actions open a short-lived connection each and send
//! the matching `ClientMessage` (same throw-away style as the hub's
//! `pin_geometry`).

use std::collections::HashSet;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt, EventMask, Window,
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
    pub icon: Option<IconRgba>,
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
    raw.chunks_exact(4)
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
    std::thread::Builder::new()
        .name("opusdm-panel-ewmh".into())
        .spawn(move || {
            if let Err(e) = observe(&tx) {
                eprintln!("opusdm-panel: EWMH observer stopped: {e}");
            }
        })
        .expect("spawn ewmh thread");
    rx
}

fn observe(tx: &async_channel::Sender<EwmhEvent>) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
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

    loop {
        let event = conn.wait_for_event()?;
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
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            );
        }
        if let Some(c) = read_client(conn, xid, atoms) {
            clients.push(c);
        }
    }
    let _ = tx.send_blocking(EwmhEvent::Clients(clients));
}

/// Read the panel-relevant properties of one client window. Returns `None`
/// for windows we never show (docks, the desktop, windows that ask to be
/// left out of the taskbar) or that vanished.
fn read_client(conn: &RustConnection, xid: Window, atoms: &Atoms) -> Option<ClientWin> {
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

    Some(ClientWin { xid, title, desktop, minimized, icon })
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

fn with_root<F>(f: F)
where
    F: FnOnce(&RustConnection, Window, &Atoms) -> Result<(), Box<dyn std::error::Error>>,
{
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        f(&conn, root, &atoms)?;
        conn.flush()?;
        Ok(())
    };
    if let Err(e) = run() {
        eprintln!("opusdm-panel: X action failed: {e}");
    }
}

fn send_root(
    conn: &RustConnection,
    root: Window,
    type_: u32,
    window: Window,
    data: [u32; 5],
) -> Result<(), Box<dyn std::error::Error>> {
    let ev = ClientMessageEvent::new(32, window, type_, data);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        ev,
    )?;
    Ok(())
}

/// Switch to virtual desktop `index`.
pub fn switch_desktop(index: u32) {
    with_root(|conn, root, atoms| {
        send_root(
            conn,
            root,
            atoms._NET_CURRENT_DESKTOP,
            root,
            [index, CURRENT_TIME, 0, 0, 0],
        )
    });
}

/// Raise and focus `xid` (pager source indication).
pub fn activate_window(xid: u32) {
    with_root(|conn, root, atoms| {
        send_root(conn, root, atoms._NET_ACTIVE_WINDOW, xid, [2, CURRENT_TIME, 0, 0, 0])
    });
}

/// Minimise `xid` via ICCCM `WM_CHANGE_STATE`.
pub fn iconify_window(xid: u32) {
    with_root(|conn, root, atoms| {
        send_root(conn, root, atoms.WM_CHANGE_STATE, xid, [ICONIC_STATE, 0, 0, 0, 0])
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
