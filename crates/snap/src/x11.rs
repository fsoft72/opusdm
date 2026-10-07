//! X11 access of the snapper: reading the pointer, the active window, the
//! monitors and the work area, and placing windows. The geometry helpers are
//! pure and unit tested.

use crate::machine::WindowId;
use crate::workarea::{monitor_area, Strut};
use crate::zones::{Rect, Zone};
use std::cell::Cell;
use std::error::Error;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::randr;
use opusdm_x11::{extents_of, gone, send_to_root, Extents, MOVERESIZE_ALL, NET_WM_STATE_ADD, NET_WM_STATE_REMOVE, SOURCE_APPLICATION};
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt, Window};
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

x11rb::atom_manager! {
    /// The atoms the snapper uses.
    pub Atoms: AtomsCookie {
        _NET_ACTIVE_WINDOW,
        _NET_FRAME_EXTENTS,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_WM_WINDOW_TYPE_SPLASH,
        _NET_WORKAREA,
        _NET_CLIENT_LIST,
        _NET_WM_STRUT,
        _NET_WM_STRUT_PARTIAL,
        _NET_CURRENT_DESKTOP,
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_MOVERESIZE_WINDOW,
        _NET_WM_MOVERESIZE,
    }
}

/// `_NET_WM_MOVERESIZE` direction: move the window with the pointer.
const MOVERESIZE_MOVE: u32 = 8;

/// Keysym of the Escape key.
const KEYSYM_ESCAPE: u32 = 0xff1b;

/// Escape keycode on a standard PC keyboard, used when the map has none.
const FALLBACK_ESCAPE_KEYCODE: u8 = 9;

/// XTEST event types.
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;

/// Most client windows scanned for struts.
const STRUT_SCAN_LIMIT: u32 = 1024;

/// Time given to Openbox to cancel its move loop after Escape.
const CANCEL_DRAG_WAIT: Duration = Duration::from_millis(120);

/// The part of `a` that is also in `b`, or `None` when they do not overlap.
pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let (x, y) = (a.x.max(b.x), a.y.max(b.y));
    let (right, bottom) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    (right > x && bottom > y).then(|| Rect::new(x, y, right - x, bottom - y))
}

/// The usable area of the monitor under `pointer`: the monitor itself minus
/// what the panels and docks reserve on it. With no strut at all the screen's
/// `workarea` is used instead (it also holds the margins the window manager
/// itself configures); with struts only the bands that touch the monitor
/// count. `None` without monitors.
pub fn area_for(pointer: (i32, i32), monitors: &[Rect], struts: &[Strut], screen: (i32, i32), workarea: Option<Rect>) -> Option<Rect> {
    let monitor = monitors.iter().find(|m| m.contains(pointer)).or_else(|| monitors.first()).copied()?;
    if !struts.is_empty() { return Some(monitor_area(monitor, struts, screen)); }
    Some(workarea.and_then(|w| intersect(monitor, w)).unwrap_or(monitor))
}

/// The frame rectangle of a window whose client area is `client`.
pub fn frame_from_client(client: Rect, extents: Extents) -> Rect {
    Rect::new(client.x - extents[0], client.y - extents[2], client.w + extents[0] + extents[1], client.h + extents[2] + extents[3])
}

/// `rect` moved by the distance the pointer travelled from `from` to `to`.
pub fn shifted(rect: Rect, from: (i32, i32), to: (i32, i32)) -> Rect {
    Rect::new(rect.x + to.0 - from.0, rect.y + to.1 - from.1, rect.w, rect.h)
}

/// What `_NET_MOVERESIZE_WINDOW` expects to put the frame at `frame`: the
/// frame origin and the CLIENT size, never below 1x1.
pub fn client_geometry(frame: Rect, extents: Extents) -> (i32, i32, u32, u32) {
    let w = (frame.w - extents[0] - extents[1]).max(1) as u32;
    let h = (frame.h - extents[2] - extents[3]).max(1) as u32;
    (frame.x, frame.y, w, h)
}

/// The monitors, the struts of the docks and the work area, read when a drag
/// may start.
#[derive(Debug, Clone)]
pub struct Environment {
    pub monitors: Vec<Rect>,
    /// What the panels and docks reserve, one entry per window that reserves.
    pub struts: Vec<Strut>,
    /// Size of the whole screen, the struts are relative to its edges.
    pub screen: (i32, i32),
    /// `_NET_WORKAREA` of the current desktop, the fallback without struts.
    pub workarea: Option<Rect>,
}

impl Environment {
    /// The usable area under `pointer`; an empty rectangle (which arms no
    /// zone) when there is no monitor.
    pub fn area(&self, pointer: (i32, i32)) -> Rect {
        area_for(pointer, &self.monitors, &self.struts, self.screen, self.workarea).unwrap_or_default()
    }
}

/// A connection to the X server and what the snapper learned from it.
pub struct Display {
    pub(crate) conn: RustConnection,
    pub(crate) root: Window,
    pub(crate) screen_num: usize,
    atoms: Atoms,
    escape_keycode: u8,
    /// The last window checked by [`Display::snappable`] and the answer.
    checked: Cell<Option<(Window, bool)>>,
}

impl Display {
    /// Connect to the X server and intern the atoms.
    pub fn connect() -> Result<Display, Box<dyn Error>> {
        let opusdm_x11::Session { conn, screen_num, root } = opusdm_x11::connect()?;
        let atoms = Atoms::new(&conn)?.reply()?;
        let escape_keycode = Self::find_escape_keycode(&conn)?;
        Ok(Display { conn, root, screen_num, atoms, escape_keycode, checked: Cell::new(None) })
    }

    /// The keycode of Escape on the current keyboard map.
    fn find_escape_keycode(conn: &RustConnection) -> Result<u8, Box<dyn Error>> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let mapping = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        let per_key = usize::from(mapping.keysyms_per_keycode);
        if per_key == 0 { return Ok(FALLBACK_ESCAPE_KEYCODE); }
        let index = mapping.keysyms.chunks(per_key).position(|keysyms| keysyms.contains(&KEYSYM_ESCAPE));
        Ok(index.map_or(FALLBACK_ESCAPE_KEYCODE, |i| min + i as u8))
    }

    /// Pointer position in root coordinates and whether the left button is down.
    pub fn pointer(&self) -> Result<((i32, i32), bool), Box<dyn Error>> {
        let reply = self.conn.query_pointer(self.root)?.reply()?;
        let button1 = u16::from(reply.mask) & u16::from(xproto::KeyButMask::BUTTON1) != 0;
        Ok(((i32::from(reply.root_x), i32::from(reply.root_y)), button1))
    }

    /// Whether a compositing manager owns the screen: without one a
    /// translucent window is drawn opaque.
    pub fn compositor_running(&self) -> Result<bool, Box<dyn Error>> {
        let name = format!("_NET_WM_CM_S{}", self.screen_num);
        let atom = self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        Ok(self.conn.get_selection_owner(atom)?.reply()?.owner != 0)
    }

    /// The monitors (the whole root window when RandR is missing) and the work
    /// area of the current virtual desktop.
    pub fn environment(&self) -> Result<Environment, Box<dyn Error>> {
        let mut monitors: Vec<Rect> = randr::get_monitors(&self.conn, self.root, true)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| {
                reply
                    .monitors
                    .iter()
                    .map(|m| Rect::new(i32::from(m.x), i32::from(m.y), i32::from(m.width), i32::from(m.height)))
                    .filter(|r| r.w > 0 && r.h > 0)
                    .collect()
            })
            .unwrap_or_default();
        let root = &self.conn.setup().roots[self.screen_num];
        let screen = (i32::from(root.width_in_pixels), i32::from(root.height_in_pixels));
        if monitors.is_empty() {
            monitors.push(Rect::new(0, 0, screen.0, screen.1));
        }
        Ok(Environment { monitors, struts: self.struts()?, screen, workarea: self.workarea()? })
    }

    /// The struts of every managed window that reserves space at a screen
    /// edge (`_NET_WM_STRUT_PARTIAL`, or the older `_NET_WM_STRUT`).
    fn struts(&self) -> Result<Vec<Strut>, Box<dyn Error>> {
        let list = self.conn.get_property(false, self.root, self.atoms._NET_CLIENT_LIST, AtomEnum::WINDOW, 0, STRUT_SCAN_LIMIT)?.reply()?;
        let windows: Vec<Window> = list.value32().map(|v| v.collect()).unwrap_or_default();
        let mut requests = Vec::with_capacity(windows.len());
        for window in windows {
            let partial = self.conn.get_property(false, window, self.atoms._NET_WM_STRUT_PARTIAL, AtomEnum::CARDINAL, 0, 12)?;
            let plain = self.conn.get_property(false, window, self.atoms._NET_WM_STRUT, AtomEnum::CARDINAL, 0, 4)?;
            requests.push((partial, plain));
        }
        let mut struts = Vec::new();
        for (partial, plain) in requests {
            let read = |reply: Option<xproto::GetPropertyReply>| reply.and_then(|r| r.value32().map(|v| v.collect::<Vec<u32>>())).and_then(|v| Strut::parse(&v));
            let strut = read(gone(partial.reply())?).or_else(|| read(gone(plain.reply()).ok().flatten()));
            struts.extend(strut);
        }
        Ok(struts)
    }

    /// `_NET_WORKAREA` of the current virtual desktop.
    fn workarea(&self) -> Result<Option<Rect>, Box<dyn Error>> {
        let desktop = self.conn.get_property(false, self.root, self.atoms._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL, 0, 1)?.reply()?;
        let desktop = desktop.value32().and_then(|mut v| v.next()).unwrap_or(0) as usize;
        let reply = self.conn.get_property(false, self.root, self.atoms._NET_WORKAREA, AtomEnum::CARDINAL, 0, (desktop as u32 + 1) * 4)?.reply()?;
        let values: Vec<u32> = reply.value32().map(|v| v.collect()).unwrap_or_default();
        Ok(values.get(desktop * 4..desktop * 4 + 4).map(|v| Rect::new(v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32)))
    }

    /// The active window and its frame rectangle; `None` when there is none
    /// or it must not be snapped (dock, desktop, splash, fixed size).
    pub fn active_frame(&self) -> Result<Option<(WindowId, Rect)>, Box<dyn Error>> {
        let reply = self.conn.get_property(false, self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW, 0, 1)?.reply()?;
        let Some(window) = reply.value32().and_then(|mut v| v.next()).filter(|w| *w != 0) else { return Ok(None) };
        if !self.snappable(window)? { return Ok(None); }
        Ok(self.frame_rect(window)?.map(|rect| (window, rect)))
    }

    /// Whether `window` may be snapped. The answer is remembered for the last
    /// window asked about.
    fn snappable(&self, window: Window) -> Result<bool, Box<dyn Error>> {
        if let Some((checked, answer)) = self.checked.get() {
            if checked == window { return Ok(answer); }
        }
        let answer = self.check_snappable(window)?;
        self.checked.set(Some((window, answer)));
        Ok(answer)
    }

    /// The uncached part of [`Display::snappable`].
    fn check_snappable(&self, window: Window) -> Result<bool, Box<dyn Error>> {
        let kind = self.conn.get_property(false, window, self.atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM, 0, 8)?;
        let Some(kind) = gone(kind.reply())? else { return Ok(false) };
        let excluded = [self.atoms._NET_WM_WINDOW_TYPE_DOCK, self.atoms._NET_WM_WINDOW_TYPE_DESKTOP, self.atoms._NET_WM_WINDOW_TYPE_SPLASH];
        if kind.value32().is_some_and(|mut types| types.any(|t| excluded.contains(&t))) { return Ok(false); }
        let hints = WmSizeHints::get_normal_hints(&self.conn, window)?;
        let Some(hints) = gone(hints.reply())?.flatten() else { return Ok(true) };
        Ok(!(hints.min_size.is_some() && hints.min_size == hints.max_size))
    }

    /// The frame rectangle of `window` in root coordinates; `None` when the
    /// window is gone.
    pub fn frame_rect(&self, window: Window) -> Result<Option<Rect>, Box<dyn Error>> {
        let geometry = self.conn.get_geometry(window)?;
        let origin = self.conn.translate_coordinates(window, self.root, 0, 0)?;
        let extents = self.conn.get_property(false, window, self.atoms._NET_FRAME_EXTENTS, AtomEnum::CARDINAL, 0, 4)?;
        let (Some(geometry), Some(origin)) = (gone(geometry.reply())?, gone(origin.reply())?) else { return Ok(None) };
        let client = Rect::new(i32::from(origin.dst_x), i32::from(origin.dst_y), i32::from(geometry.width), i32::from(geometry.height));
        Ok(Some(frame_from_client(client, extents_of(gone(extents.reply())?))))
    }

    /// Send a client message about `window` to the root window, as an EWMH
    /// client does.
    fn send_to_root(&self, window: Window, kind: u32, data: [u32; 5]) -> Result<(), Box<dyn Error>> {
        send_to_root(&self.conn, self.root, window, kind, data)
    }

    /// Add or remove the maximized state of `window`.
    fn set_maximized(&self, window: Window, on: bool) -> Result<(), Box<dyn Error>> {
        let action = if on { NET_WM_STATE_ADD } else { NET_WM_STATE_REMOVE };
        let data = [action, self.atoms._NET_WM_STATE_MAXIMIZED_VERT, self.atoms._NET_WM_STATE_MAXIMIZED_HORZ, SOURCE_APPLICATION, 0];
        self.send_to_root(window, self.atoms._NET_WM_STATE, data)
    }

    /// Put the frame of `window` at `frame`.
    fn move_resize_frame(&self, window: Window, frame: Rect) -> Result<(), Box<dyn Error>> {
        let reply = gone(self.conn.get_property(false, window, self.atoms._NET_FRAME_EXTENTS, AtomEnum::CARDINAL, 0, 4)?.reply())?;
        let (x, y, w, h) = client_geometry(frame, extents_of(reply));
        self.send_to_root(window, self.atoms._NET_MOVERESIZE_WINDOW, [MOVERESIZE_ALL, x as u32, y as u32, w, h])
    }

    /// Snap `window` to `zone`: maximized for [`Zone::Maximize`], otherwise
    /// unmaximized and put at `rect`.
    pub fn place(&self, window: Window, zone: Zone, rect: Rect) -> Result<(), Box<dyn Error>> {
        if zone == Zone::Maximize {
            self.set_maximized(window, true)?;
        } else {
            self.set_maximized(window, false)?;
            self.move_resize_frame(window, rect)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    /// Give `window` its pre-snap size at `rect` while the user is dragging it.
    /// `rect` was computed for the pointer at `decided_at`.
    ///
    /// Openbox keeps applying the geometry it had when the drag started, so
    /// the move is cancelled first (Escape through XTEST), the window is
    /// resized, and a fresh move is started under the pointer with
    /// `_NET_WM_MOVERESIZE`. The pointer moves while Openbox cancels, so the
    /// rectangle is shifted by that travel, to keep the pointer where it was
    /// inside the window. If the button is already up the window is only
    /// resized.
    pub fn restore(&self, window: Window, rect: Rect, decided_at: (i32, i32)) -> Result<(), Box<dyn Error>> {
        self.set_maximized(window, false)?;
        let dragging = self.pointer()?.1;
        if dragging {
            self.press_escape()?;
            std::thread::sleep(CANCEL_DRAG_WAIT);
        }
        let ((x, y), still_down) = self.pointer()?;
        let rect = shifted(rect, decided_at, (x, y));
        self.move_resize_frame(window, rect)?;
        if dragging && still_down {
            self.send_to_root(window, self.atoms._NET_WM_MOVERESIZE, [x as u32, y as u32, MOVERESIZE_MOVE, 1, SOURCE_APPLICATION])?;
        }
        self.conn.flush()?;
        Ok(())
    }

    /// Press and release Escape through the XTEST extension.
    fn press_escape(&self) -> Result<(), Box<dyn Error>> {
        xtest::fake_input(&self.conn, KEY_PRESS, self.escape_keycode, 0, self.root, 0, 0, 0)?;
        xtest::fake_input(&self.conn, KEY_RELEASE, self.escape_keycode, 0, self.root, 0, 0, 0)?;
        self.conn.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_clips_to_the_overlap() {
        let monitor = Rect::new(0, 0, 1920, 1080);
        let workarea = Rect::new(0, 30, 1920, 1050);
        assert_eq!(intersect(monitor, workarea), Some(workarea));
        assert_eq!(intersect(Rect::new(0, 0, 10, 10), Rect::new(10, 0, 10, 10)), None);
    }

    const SCREEN: (i32, i32) = (3200, 1080);

    #[test]
    fn the_area_is_the_monitor_under_the_pointer_minus_the_panel() {
        let monitors = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1280, 1024)];
        let workarea = Some(Rect::new(0, 30, 3200, 1050));
        assert_eq!(area_for((100, 500), &monitors, &[], SCREEN, workarea), Some(Rect::new(0, 30, 1920, 1050)));
        assert_eq!(area_for((2000, 500), &monitors, &[], SCREEN, workarea), Some(Rect::new(1920, 30, 1280, 994)));
    }

    #[test]
    fn with_struts_a_panel_only_shrinks_its_own_monitor() {
        let monitors = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1280, 1024)];
        let panel = Strut::parse(&[0, 0, 30, 0, 0, 0, 0, 0, 0, 1919, 0, 0]).unwrap();
        // The union work area would shrink the second monitor too.
        let workarea = Some(Rect::new(0, 30, 3200, 1050));
        assert_eq!(area_for((100, 500), &monitors, &[panel], SCREEN, workarea), Some(Rect::new(0, 30, 1920, 1050)));
        assert_eq!(area_for((2000, 500), &monitors, &[panel], SCREEN, workarea), Some(monitors[1]));
    }

    #[test]
    fn the_area_falls_back_to_the_monitor_without_a_work_area() {
        let monitors = [Rect::new(0, 0, 1920, 1080)];
        assert_eq!(area_for((5, 5), &monitors, &[], SCREEN, None), Some(monitors[0]));
        assert_eq!(area_for((5, 5), &monitors, &[], SCREEN, Some(Rect::new(5000, 0, 10, 10))), Some(monitors[0]));
        assert_eq!(area_for((5, 5), &[], &[], SCREEN, None), None);
    }

    #[test]
    fn a_pointer_outside_every_monitor_uses_the_first() {
        let monitors = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1280, 1024)];
        assert_eq!(area_for((-50, 5), &monitors, &[], SCREEN, None), Some(monitors[0]));
    }

    #[test]
    fn frame_and_client_geometry_are_inverse() {
        let extents = [2, 2, 19, 2];
        let client = Rect::new(302, 269, 484, 316);
        let frame = frame_from_client(client, extents);
        assert_eq!(frame, Rect::new(300, 250, 488, 337));
        assert_eq!(client_geometry(frame, extents), (300, 250, 484, 316));
    }

    #[test]
    fn client_geometry_matches_the_spec_example() {
        assert_eq!(client_geometry(Rect::new(0, 30, 960, 1050), [1, 1, 24, 1]), (0, 30, 958, 1025));
    }

    #[test]
    fn the_rectangle_follows_the_pointer_travel() {
        let rect = Rect::new(100, 50, 600, 400);
        assert_eq!(shifted(rect, (10, 10), (110, 40)), Rect::new(200, 80, 600, 400));
        assert_eq!(shifted(rect, (10, 10), (10, 10)), rect);
    }

    #[test]
    fn client_geometry_never_goes_below_one_pixel() {
        assert_eq!(client_geometry(Rect::new(0, 0, 3, 3), [2, 2, 20, 2]), (0, 0, 1, 1));
    }
}
