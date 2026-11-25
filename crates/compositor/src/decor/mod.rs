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
pub mod theme;

use smithay::{
    delegate_xdg_decoration,
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
        wayland_server::{protocol::wl_surface::WlSurface, Resource},
    },
    wayland::shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface},
};

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};
use geometry::{client_rect, frame_rect, insets, Insets};

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
