//! Compositor state: the Smithay protocol states, the windows and the seat.

use std::collections::HashMap;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::{
    delegate_data_device, delegate_output, delegate_seat,
    desktop::{layer_map_for_output, PopupManager, Space, Window},
    input::{Seat, SeatHandler, SeatState},
    output::Output,
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::wl_surface::WlSurface,
        DisplayHandle, Resource,
    },
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::{OutputHandler, OutputManagerState},
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::{wlr_layer::WlrLayerShellState, xdg::XdgShellState},
        shm::ShmState,
    },
};

use crate::geometry::Rect;
use crate::grabs::ResizeTrack;

/// Log tag of the compositor.
pub const LOG_TAG: &str = "opusdm-compositor";

/// Name of the seat advertised to clients.
const SEAT_NAME: &str = "seat0";

/// What the compositor keeps about a toplevel besides its `Window`.
#[derive(Debug, Clone, Default)]
pub struct WindowInfo {
    /// Geometry before maximize / fullscreen, to restore it.
    pub restore: Option<Rect>,
    /// True while the window is maximized (it fills `usable_area()`).
    pub maximized: bool,
    /// True while the window is fullscreen (it fills the whole output and is
    /// shown above the TOP layer; the render order comes in task 7).
    pub fullscreen: bool,
    /// Position the window had when it was unmapped by a null buffer; it
    /// maps there again instead of taking a new cascade position.
    pub last_position: Option<(i32, i32)>,
    /// Interactive resize in progress, followed by the commits of the window.
    pub resize: ResizeTrack,
}

/// Per-client data handed to Smithay when a client connects.
#[derive(Default)]
pub struct ClientState {
    /// Smithay's per-client compositor state.
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    /// Nothing to do when the client is set up.
    fn initialized(&self, _client_id: ClientId) {}

    /// Traces the disconnect; its windows go away through `toplevel_destroyed`.
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        opusdm_ipc::debug_log!(LOG_TAG, "client {client_id:?} disconnected: {reason:?}");
    }
}

/// The whole compositor state, owned by the event loop of the backend.
// Some protocol states are only held to keep their globals alive.
#[allow(dead_code)]
pub struct State {
    pub display_handle: DisplayHandle,
    /// Mapped windows, in stacking order (last = top).
    pub space: Space<Window>,
    /// Every toplevel, mapped or not, keyed by its `wl_surface`.
    pub windows: HashMap<WlSurface, WindowInfo>,
    /// Toplevels created but not mapped yet (no buffer committed).
    pub unmapped: Vec<Window>,
    /// The xdg popups of every client (parents are toplevels or layer surfaces).
    pub popups: PopupManager,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    /// Mapped TOP / OVERLAY layer surface with exclusive keyboard
    /// interactivity: it keeps the keyboard while it is mapped.
    pub exclusive_layer: Option<WlSurface>,
    pub shm_state: ShmState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub output_manager_state: OutputManagerState,
    pub seat: Seat<State>,
    pub output: Output,
    /// The `--startup` command while it runs.
    pub startup: Option<Child>,
    /// Index of the next window in the cascade.
    pub cascade_index: usize,
    /// Set by SIGTERM / SIGINT; the loop ends when it is true.
    pub quit: Arc<AtomicBool>,
    pub start_time: Instant,
    /// True once `refresh` has logged that the seat has no keyboard.
    pub no_keyboard_logged: bool,
}

impl State {
    /// Creates the protocol globals, the seat (without devices) and maps
    /// `output` at the origin of the space.
    pub fn new(display: &DisplayHandle, output: Output) -> State {
        let compositor_state = CompositorState::new::<State>(display);
        let xdg_shell_state = XdgShellState::new::<State>(display);
        let layer_shell_state = WlrLayerShellState::new::<State>(display);
        let shm_state = ShmState::new::<State>(display, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(display);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<State>(display);
        let seat = seat_state.new_wl_seat(display, SEAT_NAME);
        let _global = output.create_global::<State>(display);
        let mut space = Space::default();
        space.map_output(&output, (0, 0));
        State {
            display_handle: display.clone(),
            space,
            windows: HashMap::new(),
            unmapped: Vec::new(),
            popups: PopupManager::default(),
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            exclusive_layer: None,
            shm_state,
            seat_state,
            data_device_state,
            output_manager_state,
            seat,
            output,
            startup: None,
            cascade_index: 0,
            quit: Arc::new(AtomicBool::new(false)),
            start_time: Instant::now(),
            no_keyboard_logged: false,
        }
    }

    /// Geometry of the output in the space.
    pub fn output_area(&self) -> Rect {
        let Some(geo) = self.space.output_geometry(&self.output) else {
            opusdm_ipc::err_log!(LOG_TAG, "output {} is not mapped in the space", self.output.name());
            return Rect::default();
        };
        Rect { x: geo.loc.x, y: geo.loc.y, w: geo.size.w, h: geo.size.h }
    }

    /// Area where windows are placed: the output geometry minus the exclusive
    /// zones of the layer surfaces (`non_exclusive_zone` of its layer map).
    pub fn usable_area(&self) -> Rect {
        let output = self.output_area();
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        Rect { x: output.x + zone.loc.x, y: output.y + zone.loc.y, w: zone.size.w, h: zone.size.h }
    }

    /// Sends the frame callbacks of every mapped window and layer surface
    /// (and their popups).
    pub fn send_frames(&self) {
        let time = self.start_time.elapsed();
        let output = &self.output;
        for window in self.space.elements() {
            window.send_frame(output, time, Some(Duration::ZERO), |_, _| Some(output.clone()));
        }
        let map = layer_map_for_output(output);
        for layer in map.layers() {
            layer.send_frame(output, time, Some(Duration::ZERO), |_, _| Some(output.clone()));
        }
    }

    /// Moves the mapped `window` to `loc` without changing the stacking
    /// order (Smithay's `map_element` raises the element it maps).
    pub fn move_window(&mut self, window: &Window, loc: (i32, i32)) {
        let target: Point<i32, Logical> = loc.into();
        let order: Vec<(Window, Point<i32, Logical>)> = self
            .space
            .elements()
            .filter_map(|w| self.space.element_location(w).map(|p| (w.clone(), if w == window { target } else { p })))
            .collect();
        for (w, p) in order {
            self.space.map_element(w, p, false);
        }
    }

    /// Raises `window`, gives it the keyboard and the `Activated` state;
    /// `None` takes the focus away from every window. While an exclusive
    /// keyboard layer is mapped the window is only raised: the layer keeps
    /// the keyboard.
    pub fn focus_window(&mut self, window: Option<Window>) {
        if let Some(layer) = self.exclusive_layer.clone() {
            if let Some(w) = &window {
                self.space.raise_element(w, false);
            }
            opusdm_ipc::debug_log!(LOG_TAG, "focus: exclusive layer {} keeps the keyboard", layer.id());
            // A window mapped meanwhile may have been activated by `map_element`: undo it.
            self.focus_layer(&layer);
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "focus: the seat has no keyboard");
            return;
        };
        match &window {
            Some(w) => self.space.raise_element(w, true),
            None => self.space.elements().for_each(|w| {
                w.set_activated(false);
            }),
        }
        let target = window.as_ref().and_then(|w| w.toplevel()).map(|t| t.wl_surface().clone());
        keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        for toplevel in self.space.elements().filter_map(|w| w.toplevel()) {
            toplevel.send_pending_configure();
        }
    }

    /// Gives the focus to the exclusive keyboard layer when one is mapped,
    /// else to the topmost mapped window (none when the space is empty).
    pub fn focus_topmost(&mut self) {
        if let Some(layer) = self.exclusive_layer.clone() {
            self.focus_layer(&layer);
            return;
        }
        let top = self.space.elements().next_back().cloned();
        self.focus_window(top);
    }

    /// Gives the keyboard to the layer surface `surface` and takes the
    /// `Activated` state away from every window.
    pub fn focus_layer(&mut self, surface: &WlSurface) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "focus: the seat has no keyboard");
            return;
        };
        for window in self.space.elements() {
            window.set_activated(false);
        }
        keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        for toplevel in self.space.elements().filter_map(|w| w.toplevel()) {
            toplevel.send_pending_configure();
        }
    }

    /// True when the keyboard focus is `surface`, a dead surface or nothing.
    pub fn focus_is_gone_or(&self, surface: &WlSurface) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else { return true };
        keyboard.current_focus().is_none_or(|f| &f == surface || !f.alive())
    }

    /// Drops dead windows and layer surfaces from the space, the layer map and
    /// the tables (a safety net for `toplevel_destroyed` / `layer_destroyed`)
    /// and moves a dead keyboard focus to the top window.
    pub fn refresh(&mut self) {
        self.space.refresh();
        self.windows.retain(|surface, _| surface.alive());
        self.unmapped.retain(|w| w.alive());
        self.popups.cleanup();
        let before = self.usable_area();
        layer_map_for_output(&self.output).cleanup();
        self.usable_area_changed(before);
        if self.exclusive_layer.as_ref().is_some_and(|l| !l.alive()) {
            opusdm_ipc::debug_log!(LOG_TAG, "exclusive keyboard layer is dead, released");
            self.exclusive_layer = None;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            if !self.no_keyboard_logged {
                opusdm_ipc::err_log!(LOG_TAG, "refresh: the seat has no keyboard, focus is not checked");
                self.no_keyboard_logged = true;
            }
            return;
        };
        if keyboard.current_focus().is_some_and(|f| !f.alive()) {
            opusdm_ipc::debug_log!(LOG_TAG, "keyboard focus on a dead surface, refocusing");
            self.focus_topmost();
        }
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    /// The seat state.
    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    /// Moves the selection (clipboard) focus with the keyboard focus.
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(State);

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    /// The data device state.
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);

impl OutputHandler for State {}

delegate_output!(State);
