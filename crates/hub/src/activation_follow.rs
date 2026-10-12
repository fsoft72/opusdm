//! Bring forward the window a program asks to activate, X11 only.
//!
//! A program asks for its window to come forward with a `_NET_ACTIVE_WINDOW`
//! request flagged as coming from an application: a browser that opens the
//! login page of a command line tool in a tab, Telegram clicked in the tray
//! while it is already open. Openbox refuses such a request for a window on
//! another virtual desktop (and whenever its focus stealing prevention says
//! no) and only marks the window as demanding attention, so nothing seems to
//! happen and the user has to look for the window on every desktop.
//!
//! The hub listens to the requests on the root window (they are sent with
//! `SubstructureNotify`, so any client may see them). When the window has not
//! become the active one shortly after the request, the hub switches to the
//! window's desktop (Openbox does not do it even for a pager request) and
//! sends the same request flagged as coming from a pager, which Openbox
//! honours: it restores, raises and focuses the window.
//!
//! On Wayland nothing is started.

use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, Window};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// How long the window manager may take to honour the program's request by
/// itself before the hub repeats it.
const GRACE: Duration = Duration::from_millis(250);

/// Pause between two looks at `_NET_ACTIVE_WINDOW` during [`GRACE`].
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// EWMH source indication for a request from a pager (direct user action).
const SOURCE_PAGER: u32 = 2;

/// Whether a `_NET_ACTIVE_WINDOW` request with source indication `source`
/// for `target` must be repeated as a pager request: only requests of an
/// application, only for a window the window manager manages, and only when
/// the window did not become the active one (`active`) by itself.
pub fn must_follow(source: u32, target: Window, managed: bool, active: Window) -> bool {
    source == opusdm_x11::SOURCE_APPLICATION && managed && target != 0 && active != target
}

/// The virtual desktop to switch to before `target` can be activated: the
/// desktop of the window (`window_desktop`) unless it is the `current` one,
/// the window is on every desktop or its desktop is unknown.
pub fn desktop_to_show(window_desktop: Option<u32>, current: u32) -> Option<u32> {
    window_desktop.filter(|d| *d != current && *d != opusdm_x11::ALL_DESKTOPS)
}

/// Start the thread that follows activation requests. A failure is logged
/// and never stops the hub.
pub fn start() {
    if !opusdm_ipc::session::is_x11() { return; }
    let spawned = std::thread::Builder::new().name("activation-follow".to_string()).spawn(|| {
        if let Err(e) = run() {
            opusdm_ipc::err_log!("opusdm-hub", "activation follow stopped: {e}");
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start the activation follow thread: {e}");
    }
}

/// The atoms the thread needs, interned once.
struct Atoms {
    active: u32,
    client_list: u32,
    current_desktop: u32,
    wm_desktop: u32,
}

/// Listen to the root window and follow every refused request.
fn run() -> Result<(), opusdm_x11::X11Error> {
    let opusdm_x11::Session { conn, root, .. } = opusdm_x11::connect()?;
    let atoms = Atoms {
        active: conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?.reply()?.atom,
        client_list: conn.intern_atom(false, b"_NET_CLIENT_LIST")?.reply()?.atom,
        current_desktop: conn.intern_atom(false, b"_NET_CURRENT_DESKTOP")?.reply()?.atom,
        wm_desktop: conn.intern_atom(false, b"_NET_WM_DESKTOP")?.reply()?.atom,
    };
    let aux = ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY);
    conn.change_window_attributes(root, &aux)?.check()?;
    loop {
        let Event::ClientMessage(e) = conn.wait_for_event()? else { continue };
        if e.type_ != atoms.active || e.format != 32 { continue; }
        let source = e.data.as_data32()[0];
        if source != opusdm_x11::SOURCE_APPLICATION { continue; }
        follow(&conn, root, &atoms, e.window, source)?;
    }
}

/// Give the window manager [`GRACE`] to activate `target`, then show its
/// desktop and repeat the request as a pager when [`must_follow`] says so.
fn follow(conn: &RustConnection, root: Window, atoms: &Atoms, target: Window, source: u32) -> Result<(), opusdm_x11::X11Error> {
    let started = Instant::now();
    while started.elapsed() < GRACE {
        if active_window(conn, root, atoms)? == target { return Ok(()); }
        std::thread::sleep(POLL_INTERVAL);
    }
    let active = active_window(conn, root, atoms)?;
    let managed = client_list(conn, root, atoms)?.contains(&target);
    if !must_follow(source, target, managed, active) { return Ok(()); }
    opusdm_ipc::debug_log!("opusdm-hub", "activation follow: bringing 0x{target:x} forward");
    let current = cardinal(conn, root, atoms.current_desktop)?.unwrap_or(0);
    if let Some(desktop) = desktop_to_show(cardinal(conn, target, atoms.wm_desktop)?, current) {
        // Data: the desktop, time (0 = current).
        opusdm_x11::send_to_root(conn, root, root, atoms.current_desktop, [desktop, 0, 0, 0, 0])?;
    }
    // Data: source, time (0 = current), the window that has the focus now.
    opusdm_x11::send_to_root(conn, root, target, atoms.active, [SOURCE_PAGER, 0, active, 0, 0])?;
    opusdm_x11::x11_sync(conn)?;
    Ok(())
}

/// The window that has the focus according to the window manager.
fn active_window(conn: &RustConnection, root: Window, atoms: &Atoms) -> Result<Window, opusdm_x11::X11Error> {
    let reply = conn.get_property(false, root, atoms.active, AtomEnum::WINDOW, 0, 1)?.reply()?;
    Ok(reply.value32().and_then(|mut v| v.next()).unwrap_or(0))
}

/// The first value of the CARDINAL property `atom` of `window`; `None` when
/// the property or the window is gone.
fn cardinal(conn: &RustConnection, window: Window, atom: u32) -> Result<Option<u32>, opusdm_x11::X11Error> {
    let Some(reply) = opusdm_x11::gone(conn.get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)?.reply())? else { return Ok(None) };
    Ok(reply.value32().and_then(|mut v| v.next()))
}

/// The windows the window manager lists as managed clients.
fn client_list(conn: &RustConnection, root: Window, atoms: &Atoms) -> Result<Vec<Window>, opusdm_x11::X11Error> {
    let reply = conn.get_property(false, root, atoms.client_list, AtomEnum::WINDOW, 0, u32::MAX)?.reply()?;
    Ok(reply.value32().map(|v| v.collect()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_application_request_is_followed() {
        assert!(must_follow(opusdm_x11::SOURCE_APPLICATION, 0x22, true, 0x38));
        assert!(must_follow(opusdm_x11::SOURCE_APPLICATION, 0x22, true, 0));
    }

    #[test]
    fn the_desktop_of_the_window_is_shown_when_it_is_another_one() {
        assert_eq!(desktop_to_show(Some(5), 1), Some(5));
        assert_eq!(desktop_to_show(Some(1), 1), None);
        assert_eq!(desktop_to_show(Some(opusdm_x11::ALL_DESKTOPS), 1), None);
        assert_eq!(desktop_to_show(None, 1), None);
    }

    #[test]
    fn an_honoured_request_is_left_alone() {
        assert!(!must_follow(opusdm_x11::SOURCE_APPLICATION, 0x22, true, 0x22));
    }

    #[test]
    fn only_application_requests_for_managed_windows_are_followed() {
        // A pager request is the hub's own (or the panel's): never a loop.
        assert!(!must_follow(SOURCE_PAGER, 0x22, true, 0x38));
        assert!(!must_follow(0, 0x22, true, 0x38));
        assert!(!must_follow(opusdm_x11::SOURCE_APPLICATION, 0x22, false, 0x38));
        assert!(!must_follow(opusdm_x11::SOURCE_APPLICATION, 0, true, 0x38));
    }

    /// The first value of the CARDINAL or WINDOW property `name` of `window`.
    fn card(conn: &RustConnection, window: Window, name: &[u8]) -> u32 {
        let atom = conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
        let reply = conn.get_property(false, window, atom, AtomEnum::ANY, 0, 1).unwrap().reply().unwrap();
        reply.value32().and_then(|mut v| v.next()).unwrap_or(0)
    }

    /// Wait up to two seconds for `check` to hold.
    fn eventually(check: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if check() { return true; }
            std::thread::sleep(POLL_INTERVAL);
        }
        false
    }

    /// Needs an X11 session with an EWMH window manager and at least two
    /// desktops (e.g. Openbox in Xephyr). Switches the desktop for a moment
    /// and switches it back. Run with
    /// `cargo test -p opusdm-hub --lib activation_follow -- --ignored`.
    #[test]
    #[ignore]
    fn a_window_on_another_desktop_comes_forward() {
        use x11rb::protocol::xproto::{CreateWindowAux, PropMode, WindowClass};
        use x11rb::wrapper::ConnectionExt as _;
        start();
        let opusdm_x11::Session { conn, root, .. } = opusdm_x11::connect().unwrap();
        let atom = |name: &[u8]| conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
        let home = card(&conn, root, b"_NET_CURRENT_DESKTOP");
        let count = card(&conn, root, b"_NET_NUMBER_OF_DESKTOPS");
        assert!(count >= 2, "needs two desktops");
        let other = (home + 1) % count;
        let window = conn.generate_id().unwrap();
        let aux = CreateWindowAux::new().background_pixel(0);
        conn.create_window(0, window, root, 50, 50, 200, 100, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
        // WM_HINTS with the input flag and a WM_CLASS, like a real program.
        conn.change_property32(PropMode::REPLACE, window, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, &[1, 1, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING, b"opusdm-test\0OpusdmTest\0").unwrap();
        conn.map_window(window).unwrap();
        opusdm_x11::x11_sync(&conn).unwrap();
        assert!(eventually(|| client_list_of(&conn, root).contains(&window)), "the window manager adopts the window");
        opusdm_x11::send_to_root(&conn, root, window, atom(b"_NET_WM_DESKTOP"), [other, SOURCE_PAGER, 0, 0, 0]).unwrap();
        opusdm_x11::x11_sync(&conn).unwrap();
        assert!(eventually(|| card(&conn, window, b"_NET_WM_DESKTOP") == other));

        let active = card(&conn, root, b"_NET_ACTIVE_WINDOW");
        let request = [opusdm_x11::SOURCE_APPLICATION, 0, active, 0, 0];
        opusdm_x11::send_to_root(&conn, root, window, atom(b"_NET_ACTIVE_WINDOW"), request).unwrap();
        opusdm_x11::x11_sync(&conn).unwrap();
        let followed = eventually(|| card(&conn, root, b"_NET_ACTIVE_WINDOW") == window);
        let desktop = card(&conn, root, b"_NET_CURRENT_DESKTOP");

        conn.destroy_window(window).unwrap();
        let back = eventually(|| {
            opusdm_x11::send_to_root(&conn, root, root, atom(b"_NET_CURRENT_DESKTOP"), [home, 0, 0, 0, 0]).unwrap();
            opusdm_x11::x11_sync(&conn).unwrap();
            std::thread::sleep(POLL_INTERVAL);
            card(&conn, root, b"_NET_CURRENT_DESKTOP") == home
        });
        assert!(back, "the desktop of the test is shown again");
        assert!(followed, "the window became the active one (desktop {desktop})");
        assert_eq!(desktop, other, "the desktop of the window is shown");
    }

    /// `_NET_CLIENT_LIST` of `root`.
    fn client_list_of(conn: &RustConnection, root: Window) -> Vec<Window> {
        let atom = conn.intern_atom(false, b"_NET_CLIENT_LIST").unwrap().reply().unwrap().atom;
        let reply = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, u32::MAX).unwrap().reply().unwrap();
        reply.value32().map(|v| v.collect()).unwrap_or_default()
    }
}
