//! Server side decorations: the `zxdg_decoration_manager_v1` handler and
//! the frame geometry every placement of a decorated window goes through.
//!
//! A client that asks for `server_side` gets it: its window is decorated and
//! the window manager places its frame (title bar and border) instead of
//! the client surface. A client that asks for `client_side`, unsets the
//! mode or never creates a decoration object stays client side and keeps
//! exactly the sizes and positions it had before (GTK4 apps draw their own
//! decorations).
//!
//! Ownership: the decoration state of a window is the plain flag
//! `WindowInfo::decorated`, owned by the window table and gone with the
//! window; the `zxdg_toplevel_decoration_v1` object is owned by Smithay
//! (the toplevel's user data) and no handle to it is kept here.

pub mod geometry;
pub mod render;
pub mod theme;

use smithay::{
    delegate_xdg_decoration,
    reexports::{
        wayland_protocols::xdg::{decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode, shell::server::xdg_toplevel},
        wayland_server::{protocol::wl_surface::WlSurface, Resource},
    },
    wayland::{
        compositor::with_states,
        shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface, XdgToplevelSurfaceData},
    },
};

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};
use crate::wm::WindowId;
use geometry::{client_rect, frame_rect, insets, Insets};
use render::TitleState;

/// What a backend draws for the frame of one shown decorated window.
#[derive(Debug, Clone, PartialEq)]
pub struct DecorView {
    /// The window.
    pub id: WindowId,
    /// The frame rectangle in the space.
    pub frame: Rect,
    /// The insets of the frame.
    pub insets: Insets,
    /// What its title bar shows (the title image cache key).
    pub title: TitleState,
    /// Opacity of the frame, `0.0..=1.0`: the window's own
    /// (`effective_opacity`), so a translucent window has a translucent frame.
    pub alpha: f32,
}

impl State {
    /// The insets of the toplevel `wl` as it is now (decorated, fullscreen);
    /// zero for an unknown toplevel.
    pub fn insets_of(&self, wl: &WlSurface) -> Insets {
        let Some(info) = self.windows.get(wl) else { return Insets::default() };
        insets(info.decorated, info.fullscreen, &self.decor_theme)
    }

    /// The client rectangle of the toplevel `wl` whose frame is `frame`:
    /// `frame` itself when the window has no frame (client side, or
    /// fullscreen), else `frame` minus the insets, at least 1x1 (a configure
    /// size of 0 would leave the size to the client).
    pub fn client_rect_in(&self, wl: &WlSurface, frame: Rect) -> Rect {
        let insets = self.insets_of(wl);
        if insets == Insets::default() { return frame; }
        let client = client_rect(frame, insets);
        Rect { w: client.w.max(1), h: client.h.max(1), ..client }
    }

    /// The frame rectangle of the toplevel `wl` whose client is `client`.
    pub fn frame_rect_of(&self, wl: &WlSurface, client: Rect) -> Rect {
        frame_rect(client, self.insets_of(wl))
    }

    /// The frames to draw, bottom to top: one per window of the space that
    /// has a frame (decorated, not fullscreen), with its title, its active
    /// state (`Activated`, as `windows/list`), the hovered and pressed
    /// title bar buttons (`frame_pointer`; pressed only while the pointer
    /// is on it) and its opacity.
    pub fn decor_views(&self) -> Vec<DecorView> {
        self.space.elements().filter_map(|window| self._decor_view(window)).collect()
    }

    /// The frame of `window` (see `decor_views`); `None` without one.
    fn _decor_view(&self, window: &smithay::desktop::Window) -> Option<DecorView> {
        let wl = window.toplevel()?.wl_surface();
        let info = self.windows.get(wl)?;
        let insets = self.insets_of(wl);
        if insets == Insets::default() { return None; }
        let loc = self.space.element_location(window)?;
        let size = window.geometry().size;
        let frame = frame_rect(Rect { x: loc.x, y: loc.y, w: size.w, h: size.h }, insets);
        let active = window.toplevel().is_some_and(|t| t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Activated)));
        let hovered = self.frame_pointer.hovered.filter(|(id, _)| *id == info.id).map(|(_, b)| b);
        let pressed = self.frame_pointer.pressed.filter(|(id, _)| *id == info.id).map(|(_, b)| b).filter(|b| hovered == Some(*b));
        let title = TitleState { title: _title_of(wl), active, width: frame.w, hovered, pressed };
        let alpha = f32::from(self.effective_opacity(info)) / 100.0;
        Some(DecorView { id: info.id, frame, insets, title, alpha })
    }

    /// Sets the decoration mode of `toplevel` (`ServerSide` decorates it,
    /// anything else leaves it client side), sends it once the initial
    /// configure went out and, when the decorated state of a maximized or
    /// snapped window changed, fits the client in its area again. A window
    /// in the normal state keeps its client where it is. An unknown
    /// toplevel only gets the mode (logged).
    fn _set_decoration_mode(&mut self, toplevel: &ToplevelSurface, mode: Mode, why: &str) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
        let wl = toplevel.wl_surface().clone();
        let decorated = mode == Mode::ServerSide;
        opusdm_ipc::debug_log!(LOG_TAG, "toplevel {} decoration {mode:?} ({why})", wl.id());
        let Some(info) = self.windows.get_mut(&wl) else {
            opusdm_ipc::err_log!(LOG_TAG, "decoration of {}: unknown toplevel, mode {mode:?} sent, no frame", wl.id());
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
            return;
        };
        let changed = info.decorated != decorated;
        info.decorated = decorated;
        let (maximized, fullscreen, snapped) = (info.maximized, info.fullscreen, info.snapped);
        if changed && !fullscreen {
            if maximized {
                let area = self.usable_area();
                self.refill_window(&wl, area, "maximized");
            } else if let Some(zone) = snapped {
                let area = self.snap_target(zone);
                self.refill_window(&wl, area, "snapped");
            } else if decorated {
                self._keep_title_in_area(&wl);
            }
        }
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        if changed {
            self.notify_watchers();
        }
    }
}

impl State {
    /// A shown window in the normal state that just became decorated: when
    /// its frame top (its title bar) would be above the top of the usable
    /// area, the window moves down so the frame starts there (the client
    /// keeps its place otherwise; a hidden or not yet mapped window is
    /// placed by its frame when it shows).
    fn _keep_title_in_area(&mut self, wl: &WlSurface) {
        let Some(window) = self.windows.get(wl).map(|info| info.window.clone()) else { return };
        let Some(loc) = self.space.element_location(&window) else {
            opusdm_ipc::debug_log!(LOG_TAG, "decoration of {}: not shown, placed by its frame when shown", wl.id());
            return;
        };
        let top = self.insets_of(wl).top;
        let area = self.usable_area();
        if loc.y.saturating_sub(top) >= area.y { return; }
        let y = area.y.saturating_add(top);
        opusdm_ipc::debug_log!(LOG_TAG, "decoration of {}: title above the usable area, moved down to {}", wl.id(), y);
        self.move_window(&window, (loc.x, y));
    }
}

impl XdgDecorationHandler for State {
    /// A decoration object for `toplevel`: client side until the client
    /// asks for server side (no mode asked means client side here).
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self._set_decoration_mode(&toplevel, Mode::ClientSide, "new decoration object");
    }

    /// The client prefers `mode`: `ServerSide` is granted, anything else is
    /// client side.
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        let granted = if mode == Mode::ServerSide { Mode::ServerSide } else { Mode::ClientSide };
        self._set_decoration_mode(&toplevel, granted, "asked by the client");
    }

    /// The client has no preference any more: client side.
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self._set_decoration_mode(&toplevel, Mode::ClientSide, "mode unset by the client");
    }
}

delegate_xdg_decoration!(State);

/// The title of the toplevel `wl` (`xdg_toplevel.set_title`); empty when
/// it has none (a poisoned lock is logged).
fn _title_of(wl: &WlSurface) -> String {
    with_states(wl, |states| {
        let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() else { return String::new() };
        match data.lock() {
            Ok(attrs) => attrs.title.clone().unwrap_or_default(),
            Err(_) => {
                opusdm_ipc::err_log!(LOG_TAG, "toplevel {}: data lock poisoned, no title", wl.id());
                String::new()
            }
        }
    })
}
