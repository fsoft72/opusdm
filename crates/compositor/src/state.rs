//! Compositor state: the Smithay protocol states, the windows and the seat.

use std::collections::{HashMap, HashSet};
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::with_renderer_surface_state},
    delegate_data_device, delegate_output, delegate_seat,
    desktop::{layer_map_for_output, LayerSurface, PopupManager, Space, Window},
    input::{
        keyboard::{KeyboardHandle, Keycode},
        pointer::CursorImageStatus,
        Seat, SeatHandler, SeatState,
    },
    output::Output,
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::wl_surface::WlSurface,
        DisplayHandle, Resource,
    },
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        dmabuf::{DmabufState, ImportNotifier},
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
        xdg_activation::XdgActivationState,
    },
};

use crate::config::CompositorConfig;
use crate::geometry::Rect;
use crate::grabs::ResizeTrack;
use crate::hotkeys::HotkeyTable;
use crate::ipc::IpcServer;
use crate::menu::theme::MenuTheme;
use crate::menu::WindowMenu;
use crate::protocols::foreign_toplevel::ForeignToplevels;
use crate::protocols::workspace::Workspaces;
use crate::wm::desktops::Desktops;
use crate::wm::{WindowId, HIDDEN_FRAME_INTERVAL};

/// Log tag of the compositor.
pub const LOG_TAG: &str = "opusdm-compositor";

/// Name of the seat advertised to clients.
const SEAT_NAME: &str = "seat0";

/// What the compositor keeps about a toplevel: its `Window` and the state
/// of the window manager.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// Stable id of the window, never reused in a run.
    pub id: WindowId,
    /// The Smithay window, kept here for the whole life of the toplevel
    /// (the space holds only the shown windows).
    pub window: Window,
    /// 0-based index of the desktop the window is on.
    pub desktop: u32,
    /// True while the window is minimized (hidden until it is activated).
    pub minimized: bool,
    /// Geometry before maximize / fullscreen, to restore it.
    pub restore: Option<Rect>,
    /// True while the window is maximized (it fills `usable_area()`).
    pub maximized: bool,
    /// True while the window is fullscreen (it fills the whole output and is
    /// shown above the TOP layer when it is the topmost window).
    pub fullscreen: bool,
    /// Position the window had when it left the space (unmapped by a null
    /// buffer, minimized or left on another desktop); it maps there again
    /// instead of taking a new cascade position.
    pub last_position: Option<(i32, i32)>,
    /// Interactive resize in progress, followed by the commits of the window.
    pub resize: ResizeTrack,
    /// Pid of the client process, read once from the socket credentials
    /// when the toplevel is created (`None` when unknown or 0).
    pub pid: Option<u32>,
    /// An xdg-activation request came while the toplevel was not mapped:
    /// it is activated when it maps (the flag goes away with the window).
    pub pending_activation: bool,
}

impl WindowInfo {
    /// Info of a new toplevel `window` with the id `id`, on the desktop `desktop`.
    pub fn new(id: WindowId, window: Window, desktop: u32) -> WindowInfo {
        WindowInfo {
            id,
            window,
            desktop,
            minimized: false,
            restore: None,
            maximized: false,
            fullscreen: false,
            last_position: None,
            resize: ResizeTrack::default(),
            pid: None,
            pending_activation: false,
        }
    }
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
    /// Shown windows, in stacking order (last = top): minimized windows and
    /// windows of other desktops are out of the space (see `sync_space`).
    pub space: Space<Window>,
    /// Every toplevel, mapped or not, shown or hidden, keyed by its
    /// `wl_surface`: the owner of the `Window` of every toplevel.
    pub windows: HashMap<WlSurface, WindowInfo>,
    /// Stacking order of every toplevel (last = top), shown or not.
    pub stack: Vec<WindowId>,
    /// The active toplevel: the last toplevel that took the keyboard, the
    /// only one with the `Activated` state. A layer surface taking the
    /// keyboard (a panel click, an exclusive layer) leaves it as it is; it
    /// changes only when another toplevel gets the keyboard, and is cleared
    /// when its window is hidden, unmapped or destroyed.
    pub active_window: Option<WindowId>,
    /// The virtual desktops.
    pub desktops: Desktops,
    /// Id the next toplevel gets (ids start at 1, never reused).
    pub next_window_id: u32,
    /// When the hidden windows got their last frame callbacks.
    pub last_hidden_frame: Instant,
    /// Toplevels created but not mapped yet (no buffer committed).
    pub unmapped: Vec<Window>,
    /// The xdg popups of every client (parents are toplevels or layer surfaces).
    pub popups: PopupManager,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    /// `xdg_activation_v1`: the activation tokens (see `activation.rs`).
    pub xdg_activation_state: XdgActivationState,
    /// Mapped TOP / OVERLAY layer surface with exclusive keyboard
    /// interactivity: it keeps the keyboard while it is mapped.
    pub exclusive_layer: Option<WlSurface>,
    /// Layer surfaces hidden by a null buffer (their role kept): out of the
    /// layer map until their next commit, which maps them again.
    pub hidden_layers: Vec<LayerSurface>,
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
    /// Cursor image asked by the client with the pointer focus (drawn by
    /// the nested backend; the headless backend ignores it).
    pub cursor_status: CursorImageStatus,
    /// `linux-dmabuf` state; its global exists only in the nested backend.
    pub dmabuf_state: DmabufState,
    /// Dmabufs imported by clients and not yet checked by the renderer:
    /// the nested backend answers each notifier on its next frame.
    pub pending_dmabufs: Vec<(Dmabuf, ImportNotifier)>,
    /// The private IPC server (unbound until the backend binds it).
    pub ipc: IpcServer,
    /// True when the IPC accepts synthetic input (`debug/key`): only with
    /// `--headless`, the test backend.
    pub test_input: bool,
    /// The `zwlr_foreign_toplevel_manager_v1` global, its managers and handles.
    pub foreign_toplevels: ForeignToplevels,
    /// The `ext_workspace_manager_v1` global and its managers.
    pub workspaces: Workspaces,
    /// Built-in bindings (`compositor.json`) and the hotkeys of the hub,
    /// matched by `keyboard_key` before the focused client.
    pub hotkeys: HotkeyTable,
    /// Keys whose press matched a binding: their release (and any repeat
    /// press) is swallowed too, so the client never sees half a key.
    pub swallowed_keys: HashSet<Keycode>,
    /// The open window menu (Alt+Space), owned here only; it names its
    /// window by id and is closed when the window goes (see `menu`).
    pub menu: Option<WindowMenu>,
    /// Colours of the window menu, read once at startup from OpusDMTheme.
    pub menu_theme: MenuTheme,
    /// Pointer buttons whose press the window menu took: their release is
    /// swallowed too, so no client sees half a click.
    pub swallowed_buttons: HashSet<u32>,
}

impl State {
    /// Creates the protocol globals, the seat (without devices), `desktops`
    /// virtual desktops (clamped) and maps `output` at the origin of the space.
    pub fn new(display: &DisplayHandle, output: Output, desktops: u32) -> State {
        let compositor_state = CompositorState::new::<State>(display);
        let xdg_shell_state = XdgShellState::new::<State>(display);
        let layer_shell_state = WlrLayerShellState::new::<State>(display);
        let xdg_activation_state = XdgActivationState::new::<State>(display);
        let shm_state = ShmState::new::<State>(display, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(display);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<State>(display);
        let seat = seat_state.new_wl_seat(display, SEAT_NAME);
        let _global = output.create_global::<State>(display);
        let foreign_toplevels = ForeignToplevels::new(display);
        let workspaces = Workspaces::new(display);
        let mut space = Space::default();
        space.map_output(&output, (0, 0));
        State {
            display_handle: display.clone(),
            space,
            windows: HashMap::new(),
            stack: Vec::new(),
            active_window: None,
            desktops: Desktops::new(desktops),
            next_window_id: 1,
            last_hidden_frame: Instant::now(),
            unmapped: Vec::new(),
            popups: PopupManager::default(),
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            xdg_activation_state,
            exclusive_layer: None,
            hidden_layers: Vec::new(),
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
            cursor_status: CursorImageStatus::default_named(),
            dmabuf_state: DmabufState::new(),
            pending_dmabufs: Vec::new(),
            ipc: IpcServer::default(),
            test_input: false,
            foreign_toplevels,
            workspaces,
            hotkeys: HotkeyTable::new(&CompositorConfig::default()),
            swallowed_keys: HashSet::new(),
            menu: None,
            menu_theme: MenuTheme::load(),
            swallowed_buttons: HashSet::new(),
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

    /// Sends the frame callbacks of every shown window and layer surface
    /// (and their popups), and every `HIDDEN_FRAME_INTERVAL` those of the
    /// hidden windows that have a buffer (minimized or on another desktop),
    /// so their clients do not stall.
    pub fn send_frames(&mut self) {
        let time = self.start_time.elapsed();
        let output = &self.output;
        for window in self.space.elements() {
            window.send_frame(output, time, Some(Duration::ZERO), |_, _| Some(output.clone()));
        }
        if self.last_hidden_frame.elapsed() >= HIDDEN_FRAME_INTERVAL {
            self.last_hidden_frame = Instant::now();
            let hidden = self.windows.values().filter(|i| self.space.element_location(&i.window).is_none() && has_buffer(&i.window));
            for info in hidden {
                info.window.send_frame(output, time, Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
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
                self.raise_in_stack(w);
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
        if let Some(w) = &window {
            self.raise_in_stack(w);
        }
        match &window {
            Some(w) => {
                self.space.raise_element(w, true);
                self.active_window = _window_id_of(&self.windows, w);
                if self.active_window.is_none() {
                    opusdm_ipc::err_log!(LOG_TAG, "focus: the focused window is not in the table, no active window");
                }
            }
            None => {
                self.active_window = None;
                self.space.elements().for_each(|w| {
                    w.set_activated(false);
                });
            }
        }
        let target = window.as_ref().and_then(|w| w.toplevel()).map(|t| t.wl_surface().clone());
        if let Some(target) = &target {
            self._end_foreign_popup_grab(&keyboard, target);
        }
        keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        for toplevel in self.space.elements().filter_map(|w| w.toplevel()) {
            toplevel.send_pending_configure();
        }
    }

    /// Ends the popup grab (an open menu) rooted at a surface other than
    /// `target` before `target` takes the keyboard: Smithay's popup
    /// keyboard grab ignores focus changes while it is active and gives the
    /// keyboard back to its root when it ends, so the keyboard would stay
    /// with the menu while `target` is shown active. The popups of the root
    /// get `popup_done`. A grab rooted at `target` itself (its own menu) is
    /// left alone; no grab: nothing.
    fn _end_foreign_popup_grab(&mut self, keyboard: &KeyboardHandle<State>, target: &WlSurface) {
        let Some(root) = keyboard.grab_start_data().and_then(|d| d.focus).filter(|r| r != target) else { return };
        opusdm_ipc::debug_log!(LOG_TAG, "focus: {} ends the popup grab rooted at {}", target.id(), root.id());
        self.dismiss_popups_and_grabs(&root);
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

    /// Gives the keyboard to the layer surface `surface`. The active window
    /// keeps the `Activated` state (a layer never becomes the active window);
    /// every other window loses it (`map_element` may have given it to a
    /// window mapped under an exclusive layer).
    pub fn focus_layer(&mut self, surface: &WlSurface) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "focus: the seat has no keyboard");
            return;
        };
        self.apply_active_window();
        keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        for toplevel in self.space.elements().filter_map(|w| w.toplevel()) {
            toplevel.send_pending_configure();
        }
    }

    /// Gives the `Activated` state to the active window (`active_window`)
    /// and takes it away from every other window of the space; the caller
    /// sends the configures.
    pub fn apply_active_window(&self) {
        for window in self.space.elements() {
            let id = _window_id_of(&self.windows, window);
            window.set_activated(id.is_some() && id == self.active_window);
        }
    }

    /// Clears `active_window` when it is `id` (its window was hidden,
    /// unmapped or destroyed).
    pub fn forget_active_window(&mut self, id: WindowId) {
        if self.active_window != Some(id) { return; }
        self.active_window = None;
        opusdm_ipc::debug_log!(LOG_TAG, "window {} is no longer the active window", id.0);
    }

    /// True when the keyboard focus is `surface`, a dead surface or nothing.
    pub fn focus_is_gone_or(&self, surface: &WlSurface) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else { return true };
        keyboard.current_focus().is_none_or(|f| &f == surface || !f.alive())
    }

    /// Drops dead windows and layer surfaces from the space, the layer map and
    /// the tables (a safety net for `toplevel_destroyed` / `layer_destroyed`),
    /// closes a window menu whose window is gone or hidden and moves a dead
    /// keyboard focus to the top window.
    pub fn refresh(&mut self) {
        self.space.refresh();
        self.windows.retain(|surface, _| surface.alive());
        let windows = &self.windows;
        self.stack.retain(|id| windows.values().any(|i| i.id == *id));
        if let Some(id) = self.active_window.filter(|id| !windows.values().any(|i| i.id == *id)) {
            opusdm_ipc::debug_log!(LOG_TAG, "active window {} is dead, cleared", id.0);
            self.active_window = None;
        }
        self.unmapped.retain(|w| w.alive());
        self.popups.cleanup();
        let before = self.usable_area();
        layer_map_for_output(&self.output).cleanup();
        self.usable_area_changed(before);
        let hidden = self.hidden_layers.len();
        self.hidden_layers.retain(|l| l.alive());
        if self.hidden_layers.len() != hidden {
            opusdm_ipc::debug_log!(LOG_TAG, "{} dead hidden layer surface(s) dropped", hidden - self.hidden_layers.len());
        }
        if self.exclusive_layer.as_ref().is_some_and(|l| !l.alive()) {
            opusdm_ipc::debug_log!(LOG_TAG, "exclusive keyboard layer is dead, released");
            self.exclusive_layer = None;
        }
        self.check_window_menu();
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

/// Id of the toplevel `window` in `windows`, when it is in the table.
fn _window_id_of(windows: &HashMap<WlSurface, WindowInfo>, window: &Window) -> Option<WindowId> {
    windows.get(window.toplevel()?.wl_surface()).map(|info| info.id)
}

/// Pid of the client that owns `surface`, from its socket credentials;
/// `None` (logged) when the client or its credentials are gone, or the pid
/// is not positive (a client in another pid namespace reads 0).
pub fn client_pid(dh: &DisplayHandle, surface: &WlSurface) -> Option<u32> {
    let client = match dh.get_client(surface.id()) {
        Ok(client) => client,
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "pid of {}: no client: {e}", surface.id());
            return None;
        }
    };
    let credentials = match client.get_credentials(dh) {
        Ok(credentials) => credentials,
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "pid of {}: no credentials: {e}", surface.id());
            return None;
        }
    };
    match u32::try_from(credentials.pid) {
        Ok(pid) if pid > 0 => Some(pid),
        _ => {
            opusdm_ipc::err_log!(LOG_TAG, "pid of {}: credentials give pid {}, shown as unknown", surface.id(), credentials.pid);
            None
        }
    }
}

/// True when the toplevel surface of `window` has a buffer attached.
pub fn has_buffer(window: &Window) -> bool {
    let Some(toplevel) = window.toplevel() else { return false };
    with_renderer_surface_state(toplevel.wl_surface(), |s| s.buffer().is_some()).unwrap_or(false)
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

    /// Stores the cursor image the focused client asked for.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
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
