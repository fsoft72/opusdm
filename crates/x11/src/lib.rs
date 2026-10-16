//! X11 plumbing shared by every crate that talks to the X server or the window
//! manager, with no GTK: the connection, the EWMH client messages, frame
//! extents and the constants of the protocols.
//!
//! Window manager requests (geometry, `_NET_WM_STATE`, restacking) all go
//! through one lazily opened connection per process, with every atom interned
//! once: [`with_x11`] hands it to a closure and waits for a round trip when the
//! closure returns, so the requests have reached the server (and the window
//! manager) by then. Opening a connection per call cost a connect, a handful
//! of `InternAtom` round trips and a close each time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{self, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode, StackMode, Window};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// Error type of the X11 helpers.
pub type X11Error = Box<dyn std::error::Error>;

/// `_NET_WM_STATE` action: remove the listed property.
pub const NET_WM_STATE_REMOVE: u32 = 0;

/// `_NET_WM_STATE` action: add the listed property.
pub const NET_WM_STATE_ADD: u32 = 1;

/// `_NET_WM_DESKTOP` value meaning "on all virtual desktops".
pub const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

/// EWMH source indication for client messages sent by a normal application.
pub const SOURCE_APPLICATION: u32 = 1;

/// `_NET_MOVERESIZE_WINDOW` flags: keep the window gravity, all four of x, y,
/// width and height are supplied, and the request comes from the application.
pub const MOVERESIZE_ALL: u32 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12);

/// Frame extents of a window (`_NET_FRAME_EXTENTS`): left, right, top, bottom.
pub type Extents = [i32; 4];

/// A new connection to the X server and what every user of it needs at once.
pub struct Session {
    pub conn: RustConnection,
    pub screen_num: usize,
    /// The root window of the default screen.
    pub root: Window,
}

/// Connect to the X server named by `DISPLAY`. Refused on a Wayland session,
/// where `DISPLAY` names XWayland: the EWMH requests would reach nobody and
/// the X clients seen there are only a part of the windows.
pub fn connect() -> Result<Session, X11Error> {
    if !opusdm_ipc::session::is_x11() { return Err("not an X11 session".into()); }
    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    Ok(Session { conn, screen_num, root })
}

/// Send a 32 bit `ClientMessage` of type `message` about `window` to `root`,
/// the way a client talks to an EWMH window manager.
pub fn send_to_root(conn: &impl Connection, root: Window, window: Window, message: u32, data: [u32; 5]) -> Result<(), X11Error> {
    let event = ClientMessageEvent::new(32, window, message, data);
    conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)?;
    Ok(())
}

/// The frame extents in a `_NET_FRAME_EXTENTS` reply; zero when the property
/// is absent or malformed.
pub fn extents_of(reply: Option<xproto::GetPropertyReply>) -> Extents {
    let values: Vec<u32> = reply.and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default();
    match values[..] {
        [l, r, t, b] => [l as i32, r as i32, t as i32, b as i32],
        _ => [0; 4],
    }
}

/// Turn "the window is gone" (an X error reply) into `None` and keep real
/// connection failures.
pub fn gone<T>(result: Result<T, ReplyError>) -> Result<Option<T>, X11Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The raw value of property `prop` of `window`, of any type. `None` when
/// the property is absent or empty, or the window is gone.
pub fn property_bytes(conn: &impl Connection, window: Window, prop: u32) -> Option<Vec<u8>> {
    let reply = conn.get_property(false, window, prop, AtomEnum::ANY, 0, u32::MAX).ok()?.reply().ok()?;
    if reply.value.is_empty() { return None; }
    Some(reply.value)
}

/// The first 32 bit value of property `prop` of `window` (a `CARDINAL`,
/// `WINDOW` or `ATOM`). `None` when it is absent or not 32 bit.
pub fn property_card(conn: &impl Connection, window: Window, prop: u32) -> Option<u32> {
    let reply = conn.get_property(false, window, prop, AtomEnum::ANY, 0, 1).ok()?.reply().ok()?;
    reply.value32().and_then(|mut values| values.next())
}

/// Every 32 bit value of property `prop` of `window` (a list of cardinals,
/// windows or atoms). `None` when it is absent or not 32 bit.
pub fn property_cards(conn: &impl Connection, window: Window, prop: u32) -> Option<Vec<u32>> {
    let reply = conn.get_property(false, window, prop, AtomEnum::ANY, 0, u32::MAX).ok()?.reply().ok()?;
    reply.value32().map(|values| values.collect())
}

/// Property `prop` of `window` as text, invalid UTF-8 replaced. `None` when
/// it is absent or empty.
pub fn property_string(conn: &impl Connection, window: Window, prop: u32) -> Option<String> {
    property_bytes(conn, window, prop).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Delays, in milliseconds, at which a window is placed again after it was
/// mapped: the window manager runs its own placement policy after the map and
/// may move the window, or drop state messages sent before it adopted the
/// window, so the geometry and states are asserted again a few times.
pub const PLACEMENT_RETRY_MS: [u64; 3] = [80, 400, 1000];

/// The desktop `delta` steps from `current` among `count` desktops, wrapping
/// around at both ends. `None` when there are no desktops.
pub fn wrapped_desktop(current: u32, count: u32, delta: i32) -> Option<u32> {
    if count == 0 {
        return None;
    }
    let target = (i64::from(current) + i64::from(delta)).rem_euclid(i64::from(count));
    u32::try_from(target).ok()
}

/// Flush `conn` and wait until the X server has processed everything sent on
/// it. Needed before dropping a connection, and after sending requests the
/// caller wants the window manager to have seen.
pub fn x11_sync(conn: &impl Connection) -> Result<(), ReplyError> {
    conn.get_input_focus()?.reply()?;
    Ok(())
}

/// The process-wide connection and what was learned from it.
pub struct X11 {
    conn: RustConnection,
    root: u32,
    atoms: Mutex<HashMap<&'static str, u32>>,
}

static SHARED: Mutex<Option<Arc<X11>>> = Mutex::new(None);

/// Run `f` on the shared connection, opening it on first use, and wait for a
/// round trip afterwards. A failure drops the connection so the next call
/// reconnects.
pub fn with_x11<T>(f: impl FnOnce(&X11) -> Result<T, X11Error>) -> Result<T, X11Error> {
    let x11 = shared()?;
    let result = f(&x11).and_then(|value| {
        x11_sync(&x11.conn)?;
        Ok(value)
    });
    if result.is_err() {
        let mut slot = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|current| Arc::ptr_eq(current, &x11)) {
            *slot = None;
        }
    }
    result
}

/// The shared connection, opened if this is the first use.
fn shared() -> Result<Arc<X11>, X11Error> {
    let mut slot = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(x11) = slot.as_ref() {
        return Ok(x11.clone());
    }
    let Session { conn, root, .. } = connect()?;
    let x11 = Arc::new(X11 { conn, root, atoms: Mutex::new(HashMap::new()) });
    *slot = Some(x11.clone());
    Ok(x11)
}

impl X11 {
    /// The raw connection, for requests the helpers do not cover.
    pub fn conn(&self) -> &RustConnection {
        &self.conn
    }

    /// The root window of the default screen.
    pub fn root(&self) -> u32 {
        self.root
    }

    /// Current height of the default screen in pixels, read from the root
    /// window geometry on every call so it follows xrandr resizes (the
    /// connection setup keeps the size the screen had when it was opened).
    pub fn screen_height(&self) -> Result<i32, X11Error> {
        let geometry = self.conn.get_geometry(self.root)?.reply()?;
        Ok(i32::from(geometry.height))
    }

    /// The atom called `name`, interned on first use.
    pub fn atom(&self, name: &'static str) -> Result<u32, X11Error> {
        if let Some(&atom) = self.atoms.lock().unwrap_or_else(|e| e.into_inner()).get(name) {
            return Ok(atom);
        }
        let atom = self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        self.atoms.lock().unwrap_or_else(|e| e.into_inner()).insert(name, atom);
        Ok(atom)
    }

    /// Send a `ClientMessage` of type `message` about `window` to the root
    /// window, the way a client talks to an EWMH window manager.
    pub fn send_to_root(&self, window: u32, message: &'static str, data: [u32; 5]) -> Result<(), X11Error> {
        send_to_root(&self.conn, self.root, window, self.atom(message)?, data)
    }

    /// Ask the window manager to add each of the `_NET_WM_STATE_*` atoms in
    /// `states` to `window`, one message per state.
    pub fn add_wm_states(&self, window: u32, states: &[&'static str]) -> Result<(), X11Error> {
        for state in states {
            let state = self.atom(state)?;
            self.send_to_root(window, "_NET_WM_STATE", [NET_WM_STATE_ADD, state, 0, 0, 0])?;
        }
        Ok(())
    }

    /// Put `window` on every virtual desktop: `_NET_WM_DESKTOP` set to all
    /// desktops, as a property (read when the window manager adopts the
    /// window) and as an EWMH client message (the only form honoured once the
    /// window is mapped), plus a `_NET_WM_STATE_STICKY` message. Callers that
    /// write `_NET_WM_STATE` before mapping should list STICKY there too.
    /// Safe to call both before and after mapping.
    pub fn stick_to_all_desktops(&self, window: u32) -> Result<(), X11Error> {
        let wm_desktop = self.atom("_NET_WM_DESKTOP")?;
        self.conn.change_property32(PropMode::REPLACE, window, wm_desktop, AtomEnum::CARDINAL, &[ALL_DESKTOPS])?;
        self.send_to_root(window, "_NET_WM_DESKTOP", [ALL_DESKTOPS, SOURCE_APPLICATION, 0, 0, 0])?;
        self.add_wm_states(window, &["_NET_WM_STATE_STICKY"])
    }

    /// Undo [`X11::stick_to_all_desktops`] on a mapped `window`: move it to
    /// the current virtual desktop only and drop `_NET_WM_STATE_STICKY`.
    pub fn unstick_to_current_desktop(&self, window: u32) -> Result<(), X11Error> {
        let current = self.atom("_NET_CURRENT_DESKTOP")?;
        let reply = self.conn.get_property(false, self.root, current, AtomEnum::CARDINAL, 0, 1)?.reply()?;
        let desktop = reply.value32().and_then(|mut v| v.next()).unwrap_or(0);
        self.send_to_root(window, "_NET_WM_DESKTOP", [desktop, SOURCE_APPLICATION, 0, 0, 0])?;
        let sticky = self.atom("_NET_WM_STATE_STICKY")?;
        self.send_to_root(window, "_NET_WM_STATE", [NET_WM_STATE_REMOVE, sticky, 0, 0, 0])
    }

    /// The first value of the CARDINAL property `name` on the root window.
    fn root_cardinal(&self, name: &'static str) -> Result<Option<u32>, X11Error> {
        let atom = self.atom(name)?;
        let reply = self.conn.get_property(false, self.root, atom, AtomEnum::CARDINAL, 0, 1)?.reply()?;
        Ok(reply.value32().and_then(|mut v| v.next()))
    }

    /// Switch the window manager to the virtual desktop `delta` steps away
    /// from the current one, wrapping at both ends (`-1` previous, `1`
    /// next). Does nothing when the window manager reports no desktops.
    pub fn switch_desktop(&self, delta: i32) -> Result<(), X11Error> {
        let current = self.root_cardinal("_NET_CURRENT_DESKTOP")?.unwrap_or(0);
        let count = self.root_cardinal("_NET_NUMBER_OF_DESKTOPS")?.unwrap_or(0);
        let Some(target) = wrapped_desktop(current, count, delta) else { return Ok(()) };
        // Data: the new desktop and the source indication; 0 = current time.
        self.send_to_root(self.root, "_NET_CURRENT_DESKTOP", [target, 0, 0, 0, 0])
    }

    /// Ask the window manager to offer `count` virtual desktops
    /// (`_NET_NUMBER_OF_DESKTOPS` on the root window). A `count` of 0 is ignored.
    pub fn set_desktop_count(&self, count: u32) -> Result<(), X11Error> {
        if count == 0 { return Ok(()); }
        self.send_to_root(self.root, "_NET_NUMBER_OF_DESKTOPS", [count, 0, 0, 0, 0])
    }

    /// Write the `_NET_WM_WINDOW_TYPE` of `window` (the atom called
    /// `window_type`) and its `_NET_WM_STATE` (the atoms called `states`) as
    /// properties. A window manager reads them when it adopts the window;
    /// once the window is mapped, change states with [`X11::add_wm_states`].
    pub fn set_type_and_states(&self, window: u32, window_type: &'static str, states: &[&'static str]) -> Result<(), X11Error> {
        let type_atom = self.atom(window_type)?;
        let state_atoms = states.iter().map(|name| self.atom(name)).collect::<Result<Vec<u32>, _>>()?;
        self.conn.change_property32(PropMode::REPLACE, window, self.atom("_NET_WM_WINDOW_TYPE")?, AtomEnum::ATOM, &[type_atom])?;
        self.conn.change_property32(PropMode::REPLACE, window, self.atom("_NET_WM_STATE")?, AtomEnum::ATOM, &state_atoms)?;
        Ok(())
    }

    /// Fix `window` at the given geometry: user-specified position and size
    /// hints (so the window manager does not re-place it) with the minimum
    /// and maximum size equal to the size (so it cannot be resized), then a
    /// `ConfigureWindow` there.
    pub fn set_fixed_geometry(&self, window: u32, x: i32, y: i32, w: u32, h: u32) -> Result<(), X11Error> {
        let (width, height) = (i32::try_from(w)?, i32::try_from(h)?);
        let mut hints = WmSizeHints::new();
        hints.position = Some((WmSizeHintsSpecification::UserSpecified, x, y));
        hints.size = Some((WmSizeHintsSpecification::UserSpecified, width, height));
        hints.min_size = Some((width, height));
        hints.max_size = Some((width, height));
        hints.set_normal_hints(&self.conn, window)?.check()?;
        self.configure(window, x, y, w, h)
    }

    /// A plain `ConfigureWindow` of `window` to the given geometry.
    pub fn configure(&self, window: u32, x: i32, y: i32, w: u32, h: u32) -> Result<(), X11Error> {
        self.conn.configure_window(window, &xproto::ConfigureWindowAux::new().x(x).y(y).width(w).height(h))?;
        Ok(())
    }

    /// Put `window` at the given geometry twice: a plain `ConfigureWindow`,
    /// and a `_NET_MOVERESIZE_WINDOW` message. Once the window manager has
    /// adopted a window it ignores the position of a `ConfigureWindow`; only
    /// the EWMH message moves the frame itself.
    pub fn pin_geometry(&self, window: u32, x: i32, y: i32, w: u32, h: u32) -> Result<(), X11Error> {
        self.configure(window, x, y, w, h)?;
        self.move_resize(window, MOVERESIZE_ALL, x, y, w, h)
    }

    /// Send `_NET_MOVERESIZE_WINDOW` for `window` with the given `flags`.
    pub fn move_resize(&self, window: u32, flags: u32, x: i32, y: i32, w: u32, h: u32) -> Result<(), X11Error> {
        self.send_to_root(window, "_NET_MOVERESIZE_WINDOW", [flags, x as u32, y as u32, w, h])
    }

    /// Ask the window manager to keep `window` above every other one and
    /// restack it on top now.
    pub fn raise_above(&self, window: u32) -> Result<(), X11Error> {
        self.add_wm_states(window, &["_NET_WM_STATE_ABOVE"])?;
        self.conn.configure_window(window, &xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_desktop_wraps_at_both_ends() {
        assert_eq!(wrapped_desktop(1, 4, 1), Some(2));
        assert_eq!(wrapped_desktop(1, 4, -1), Some(0));
        assert_eq!(wrapped_desktop(3, 4, 1), Some(0));
        assert_eq!(wrapped_desktop(0, 4, -1), Some(3));
        assert_eq!(wrapped_desktop(0, 1, 1), Some(0));
        assert_eq!(wrapped_desktop(0, 0, 1), None);
    }

    /// Needs a running X server (`DISPLAY`), e.g. `Xephyr :99`; run with
    /// `cargo test -p opusdm-ui -- --ignored`.
    #[test]
    #[ignore]
    fn shared_connection_interns_once_and_moves_a_window() {
        let window = with_x11(|x11| {
            let conn = x11.conn();
            let id = conn.generate_id()?;
            conn.create_window(0, id, x11.root(), 0, 0, 50, 50, 0, xproto::WindowClass::INPUT_OUTPUT, 0, &xproto::CreateWindowAux::new())?;
            Ok(id)
        })
        .unwrap();
        with_x11(|x11| x11.pin_geometry(window, 10, 20, 100, 80)).unwrap();
        with_x11(|x11| {
            assert_eq!(x11.atom("_NET_WM_STATE")?, x11.atom("_NET_WM_STATE")?);
            let geometry = x11.conn().get_geometry(window)?.reply()?;
            assert_eq!((geometry.x, geometry.y, geometry.width, geometry.height), (10, 20, 100, 80));
            Ok(())
        })
        .unwrap();
        assert!(Arc::ptr_eq(&shared().unwrap(), &shared().unwrap()), "one connection per process");
    }

    /// Needs a running X server, like the test above.
    #[test]
    #[ignore]
    fn type_states_and_fixed_geometry_are_written_and_read_back() {
        with_x11(|x11| {
            let conn = x11.conn();
            let id = conn.generate_id()?;
            conn.create_window(0, id, x11.root(), 0, 0, 50, 50, 0, xproto::WindowClass::INPUT_OUTPUT, 0, &xproto::CreateWindowAux::new())?;
            x11.set_type_and_states(id, "_NET_WM_WINDOW_TYPE_DOCK", &["_NET_WM_STATE_ABOVE", "_NET_WM_STATE_STICKY"])?;
            x11.set_fixed_geometry(id, 7, 9, 120, 40)?;
            x11_sync(conn)?;
            let win_type = property_card(conn, id, x11.atom("_NET_WM_WINDOW_TYPE")?);
            assert_eq!(win_type, Some(x11.atom("_NET_WM_WINDOW_TYPE_DOCK")?));
            let states = property_cards(conn, id, x11.atom("_NET_WM_STATE")?);
            assert_eq!(states, Some(vec![x11.atom("_NET_WM_STATE_ABOVE")?, x11.atom("_NET_WM_STATE_STICKY")?]));
            let hints = WmSizeHints::get_normal_hints(conn, id)?.reply()?.expect("size hints");
            assert_eq!((hints.min_size, hints.max_size), (Some((120, 40)), Some((120, 40))));
            let geometry = conn.get_geometry(id)?.reply()?;
            assert_eq!((geometry.x, geometry.y, geometry.width, geometry.height), (7, 9, 120, 40));
            assert_eq!(property_string(conn, id, x11.atom("_NET_WM_NAME")?), None);
            conn.destroy_window(id)?;
            Ok(())
        })
        .unwrap();
    }

    /// Needs a running X server, like the test above.
    #[test]
    #[ignore]
    fn screen_height_is_the_live_root_height() {
        with_x11(|x11| {
            let root = x11.conn().get_geometry(x11.root())?.reply()?;
            assert_eq!(x11.screen_height()?, i32::from(root.height));
            Ok(())
        })
        .unwrap();
    }
}
