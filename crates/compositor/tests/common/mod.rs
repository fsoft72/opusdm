//! Test harness: starts `opusdm-compositor --headless` and drives it with a
//! smithay-client-toolkit client connected to its socket.
//!
//! Every `Compositor` has its own `XDG_RUNTIME_DIR` and the client connects to
//! the socket path directly, never through the process environment, so the
//! tests run in parallel.

// Each test binary uses only a part of the harness.
#![allow(dead_code)]

pub mod ipc;

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Surface},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm, delegate_xdg_popup,
    delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        xdg::{
            popup::{Popup, PopupConfigure, PopupHandler},
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgPositioner, XdgShell, XdgSurface as _,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use tempfile::TempDir;
use wayland_client::{
    backend::ObjectId,
    globals::registry_queue_init,
    protocol::{wl_callback, wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface::WlSurface},
    event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup::XdgPopup,
    xdg_positioner::{Anchor as PositionerAnchor, ConstraintAdjustment, Gravity},
    xdg_surface::XdgSurface,
    xdg_toplevel::XdgToplevel,
};

pub use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer};

/// How long `Compositor::start` waits for the socket name.
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a surface waits for its first configure.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long `TestClient::roundtrip` waits for the answer to its sync: a
/// hung compositor fails the test instead of hanging `cargo test`.
const ROUNDTRIP_TIMEOUT: Duration = Duration::from_secs(5);

/// Size of the output the harness asks for.
const OUTPUT_SIZE: &str = "1920x1080";

/// A rectangle in surface or output coordinates (the binary's `Rect` is not
/// visible to integration tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Linux input codes (evdev) of the modifier keys: Ctrl, Shift, Alt and
/// Super, left and right. `keys_received` does not count them.
const MODIFIER_KEYCODES: [u32; 8] = [29, 97, 42, 54, 56, 100, 125, 126];

/// A running headless compositor, killed when dropped.
pub struct Compositor {
    /// Path of the Wayland socket.
    pub socket: PathBuf,
    /// Path of the private IPC socket (`OPUSDM_COMPOSITOR_SOCKET`).
    pub ipc_socket: PathBuf,
    child: Child,
    _runtime: TempDir,
}

impl Compositor {
    /// Starts the compositor in a fresh `XDG_RUNTIME_DIR` (also its `HOME`
    /// and `XDG_CONFIG_HOME`, so the owner's `~/.config/opusdm` is never read
    /// or written) and waits (at most `START_TIMEOUT`) for the
    /// `WAYLAND_DISPLAY=<name>` and `OPUSDM_COMPOSITOR_SOCKET=<path>` lines
    /// on its stdout.
    /// The rest of its stdout (a `--startup` child may print there) is
    /// drained for the whole run, so a writer never gets EPIPE / SIGPIPE.
    pub fn start() -> Compositor {
        let runtime = tempfile::tempdir().expect("temp XDG_RUNTIME_DIR");
        let mut child = Command::new(env!("CARGO_BIN_EXE_opusdm-compositor"))
            .args(["--headless", "--size", OUTPUT_SIZE])
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("HOME", runtime.path())
            .env("XDG_CONFIG_HOME", runtime.path().join("config"))
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn opusdm-compositor");
        let stdout = child.stdout.take().expect("compositor stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            for _ in 0..2 {
                let mut line = String::new();
                let read = reader.read_line(&mut line);
                let _ = tx.send(read.map(|_| line));
            }
            // Ends when the compositor (and every child holding the pipe) is gone.
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });
        let name = _read_announce(&mut child, &rx, "WAYLAND_DISPLAY=");
        let ipc = _read_announce(&mut child, &rx, "OPUSDM_COMPOSITOR_SOCKET=");
        let socket = runtime.path().join(name);
        Compositor { socket, ipc_socket: PathBuf::from(ipc), child, _runtime: runtime }
    }

    /// True while the compositor process has not exited.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Number of file descriptors the compositor process has open now
    /// (entries of `/proc/<pid>/fd`).
    pub fn open_fds(&self) -> usize {
        std::fs::read_dir(format!("/proc/{}/fd", self.child.id())).expect("read the compositor fd table").count()
    }

    /// Sends the signal `sig` to the compositor process.
    pub fn signal(&self, sig: i32) {
        let pid = i32::try_from(self.child.id()).expect("pid fits in i32");
        // SAFETY: kill(2) on the pid of our own child, no memory involved.
        let rc = unsafe { libc::kill(pid, sig) };
        assert_eq!(rc, 0, "kill({pid}, {sig}) failed");
    }

    /// Waits at most `timeout` for the process to exit; returns its exit code
    /// (`None` on timeout or when a signal killed it).
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }
}

/// The value of the next stdout line of `child`, which must start with
/// `prefix`; kills `child` and panics on a timeout or another line.
fn _read_announce(child: &mut Child, rx: &mpsc::Receiver<std::io::Result<String>>, prefix: &str) -> String {
    let line = match rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(line)) => line,
        other => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("compositor did not print {prefix}: {other:?}");
        }
    };
    let Some(value) = line.trim().strip_prefix(prefix) else {
        let _ = child.kill();
        let status = child.wait();
        panic!("compositor did not print {prefix} (got {line:?}, exit {status:?})");
    };
    value.to_string()
}

impl Drop for Compositor {
    /// Kills the compositor and reaps it.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Opens a client connection to `comp` through its socket path.
pub fn connect(comp: &Compositor) -> Connection {
    connect_path(&comp.socket)
}

/// Opens a client connection to the Wayland socket at `socket`.
pub fn connect_path(socket: &Path) -> Connection {
    let stream = UnixStream::connect(socket).expect("connect to the compositor socket");
    Connection::from_socket(stream).expect("wayland connection")
}

/// Anything that has a `wl_surface` (toplevels now, popups and layer surfaces later).
pub trait HasSurface {
    /// The surface of the object.
    fn wl_surface(&self) -> &WlSurface;
}

/// A mapped xdg toplevel of a `TestClient`.
pub struct TestWindow {
    window: Window,
}

impl TestWindow {
    /// The `xdg_toplevel` of the window, to send requests on it.
    pub fn xdg(&self) -> &XdgToplevel {
        self.window.xdg_toplevel()
    }
}

impl HasSurface for TestWindow {
    /// The surface of the toplevel.
    fn wl_surface(&self) -> &WlSurface {
        self.window.wl_surface()
    }
}

/// A surface that can be the parent of an xdg popup (toplevels and layer surfaces).
pub trait PopupParent {
    /// The `xdg_surface` the popup is created against; `None` for a parent
    /// that adopts the popup through its own request (a layer surface's `get_popup`).
    fn parent_xdg_surface(&self) -> Option<&XdgSurface>;

    /// Adopts `popup` before its first commit; nothing for an xdg parent.
    fn adopt_popup(&self, _popup: &XdgPopup) {}
}

impl PopupParent for TestWindow {
    /// The `xdg_surface` of the toplevel.
    fn parent_xdg_surface(&self) -> Option<&XdgSurface> {
        Some(self.window.xdg_surface())
    }
}

/// A mapped layer surface of a `TestClient`.
pub struct TestLayer {
    layer: LayerSurface,
}

impl HasSurface for TestLayer {
    /// The surface of the layer surface.
    fn wl_surface(&self) -> &WlSurface {
        self.layer.wl_surface()
    }
}

impl PopupParent for TestLayer {
    /// A layer surface has no `xdg_surface`: it adopts the popup.
    fn parent_xdg_surface(&self) -> Option<&XdgSurface> {
        None
    }

    /// Sends `zwlr_layer_surface_v1.get_popup` for `popup`.
    fn adopt_popup(&self, popup: &XdgPopup) {
        self.layer.get_popup(popup);
    }
}

/// A mapped xdg popup of a `TestClient`.
pub struct TestPopup {
    popup: Popup,
}

impl HasSurface for TestPopup {
    /// The surface of the popup.
    fn wl_surface(&self) -> &WlSurface {
        self.popup.wl_surface()
    }
}

/// A toplevel as a `zwlr_foreign_toplevel_manager_v1` client sees it after
/// the last `done` of its handle.
#[derive(Debug, Clone)]
pub struct ForeignToplevel {
    /// Last `title` event (empty before one came).
    pub title: String,
    /// Last `app_id` event (empty before one came).
    pub app_id: String,
    /// Values of the last `state` event (0 maximized, 1 minimized, 2 activated, 3 fullscreen).
    pub states: Vec<u32>,
    /// Outputs entered and not left.
    pub outputs: usize,
    /// The handle, to send requests on it.
    pub handle: ZwlrForeignToplevelHandleV1,
}

/// A foreign-toplevel handle of a `TestClient`: the events since the last
/// `done` and what the last `done` published.
struct ForeignEntry {
    /// The handle.
    handle: ZwlrForeignToplevelHandleV1,
    /// Last `title` received, published on `done`.
    title: String,
    /// Last `app_id` received, published on `done`.
    app_id: String,
    /// Last `state` received, published on `done`.
    states: Vec<u32>,
    /// Ids of the outputs entered and not left.
    outputs: Vec<ObjectId>,
    /// The toplevel as of the last `done` (`None` before the first one).
    published: Option<ForeignToplevel>,
}

/// A workspace as an `ext_workspace_manager_v1` client sees it after the
/// last `done` of the manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestWorkspace {
    /// Last `name` event (empty before one came).
    pub name: String,
    /// The last `state` event had the `active` bit.
    pub active: bool,
    /// Values of the last `coordinates` event.
    pub coordinates: Vec<u32>,
}

/// A workspace handle of a `TestClient` and its fields as received so far.
struct WorkspaceEntry {
    /// The handle.
    handle: ExtWorkspaceHandleV1,
    /// The fields received since the handle came.
    fields: TestWorkspace,
}

/// What the client knows about one of its surfaces (any role).
struct SurfaceInfo {
    /// Size the test asked for, used when a configure leaves the size to the client.
    requested: (i32, i32),
    /// Size of the last configure answered (the requested size when it was 0x0).
    last_configure: (i32, i32),
    /// True once the first configure was answered with a buffer.
    configured: bool,
    /// Buffer attached last; kept alive while the compositor may use it.
    buffer: Option<Buffer>,
    /// Position (relative to the parent) and size of the last popup configure.
    popup_rect: Rect,
    /// Size sent by the last layer surface configure (0 = the client decides).
    layer_size: (u32, u32),
}

impl SurfaceInfo {
    /// Info of a new surface the test wants of size `requested`.
    fn new(requested: (i32, i32)) -> SurfaceInfo {
        SurfaceInfo { requested, last_configure: (0, 0), configured: false, buffer: None, popup_rect: Rect::default(), layer_size: (0, 0) }
    }
}

/// Client side state handed to the event queue.
struct ClientApp {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    /// Surfaces keyed by the `wl_surface` id.
    surfaces: HashMap<ObjectId, SurfaceInfo>,
    /// Surfaces that got a frame callback since their last `wait_frame` request.
    frames: Vec<ObjectId>,
    /// Surface that has the keyboard of this client (`wl_keyboard` enter / leave).
    keyboard_focus: Option<WlSurface>,
    /// Serial of the last `wl_keyboard.enter`, for requests that need an input serial.
    keyboard_serial: u32,
    /// Popups the compositor dismissed (`xdg_popup.popup_done`), by `wl_surface` id.
    dismissed: Vec<ObjectId>,
    /// `wl_keyboard.key` events (press and release) of non-modifier keys.
    keys: u32,
    /// Toplevels that got `xdg_toplevel.close`, by `wl_surface` id.
    close_requests: Vec<ObjectId>,
    /// Foreign-toplevel handles not closed yet, in the order they came.
    foreign: Vec<ForeignEntry>,
    /// Workspace handles not removed yet, in the order they came.
    workspaces: Vec<WorkspaceEntry>,
    /// The workspaces as of the last `ext_workspace_manager_v1.done`.
    workspaces_published: Vec<TestWorkspace>,
    /// Workspace groups not removed yet.
    workspace_groups: Vec<ExtWorkspaceGroupHandleV1>,
}

impl ClientApp {
    /// Answers a configure of `surface`: picks the size (0 on an axis keeps the
    /// requested size), attaches a buffer of that size and commits.
    fn answer_configure(&mut self, surface: &WlSurface, size: (i32, i32)) {
        let id = surface.id();
        let Some(info) = self.surfaces.get(&id) else { panic!("configure for an unknown surface {id}") };
        let w = if size.0 > 0 { size.0 } else { info.requested.0 };
        let h = if size.1 > 0 { size.1 } else { info.requested.1 };
        let (buffer, canvas) = self.pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888).expect("shm buffer");
        canvas.fill(0x80);
        surface.damage_buffer(0, 0, w, h);
        buffer.attach_to(surface).expect("attach buffer");
        surface.commit();
        let info = self.surfaces.get_mut(&id).expect("surface still known");
        info.last_configure = (w, h);
        info.configured = true;
        info.buffer = Some(buffer);
    }
}

/// A smithay-client-toolkit client connected to a `Compositor`.
pub struct TestClient {
    conn: Connection,
    queue: EventQueue<ClientApp>,
    qh: QueueHandle<ClientApp>,
    app: ClientApp,
    /// Seat and keyboard, kept alive so the enter / leave events keep coming.
    _seat: wl_seat::WlSeat,
    _keyboard: wl_keyboard::WlKeyboard,
    /// Every layer surface mapped, kept alive as long as the client: a test
    /// may drop its `TestLayer` (a panel it does not look at again) and the
    /// panel must stay mapped.
    layers: Vec<LayerSurface>,
    /// The `zwlr_foreign_toplevel_manager_v1`, when the compositor has it.
    foreign_manager: Option<ZwlrForeignToplevelManagerV1>,
    /// The `ext_workspace_manager_v1`, when the compositor has it.
    workspace_manager: Option<ExtWorkspaceManagerV1>,
}

impl TestClient {
    /// Connects to `comp` and binds the globals the tests need.
    pub fn new(comp: &Compositor) -> TestClient {
        TestClient::connect_to(&comp.socket)
    }

    /// Connects to the Wayland socket at `socket` (a helper process that
    /// has no `Compositor`) and binds the globals the tests need.
    pub fn connect_to(socket: &Path) -> TestClient {
        let conn = connect_path(socket);
        let (globals, queue) = registry_queue_init::<ClientApp>(&conn).expect("registry");
        let qh = queue.handle();
        let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
        let xdg = XdgShell::bind(&globals, &qh).expect("xdg_wm_base");
        let layer_shell = LayerShell::bind(&globals, &qh).expect("zwlr_layer_shell_v1");
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).expect("wl_seat");
        let keyboard = seat.get_keyboard(&qh, ());
        let shm = Shm::bind(&globals, &qh).expect("wl_shm");
        let pool = SlotPool::new(400 * 300 * 4, &shm).expect("shm pool");
        let app = ClientApp {
            registry_state: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &qh),
            compositor,
            xdg,
            layer_shell,
            shm,
            pool,
            surfaces: HashMap::new(),
            frames: Vec::new(),
            keyboard_focus: None,
            keyboard_serial: 0,
            dismissed: Vec::new(),
            keys: 0,
            close_requests: Vec::new(),
            foreign: Vec::new(),
            workspaces: Vec::new(),
            workspaces_published: Vec::new(),
            workspace_groups: Vec::new(),
        };
        // Bound after `wl_output` (bound by `OutputState::new`); `None` when the compositor has no such global.
        let foreign_manager = globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ()).ok();
        let workspace_manager = globals.bind::<ExtWorkspaceManagerV1, _, _>(&qh, 1..=1, ()).ok();
        let mut client = TestClient {
            conn,
            queue,
            qh,
            app,
            _seat: seat,
            _keyboard: keyboard,
            layers: Vec::new(),
            foreign_manager,
            workspace_manager,
        };
        client.roundtrip();
        client
    }

    /// Maps an xdg toplevel of `size`: initial commit, waits for the first
    /// configure, answers it with a buffer and waits until the compositor has
    /// handled that commit.
    pub fn map_toplevel(&mut self, size: (i32, i32)) -> TestWindow {
        let surface = self.app.compositor.create_surface(&self.qh);
        let window = self.app.xdg.create_window(surface, WindowDecorations::None, &self.qh);
        window.set_title("opusdm-compositor test");
        let id = window.wl_surface().id();
        let info = SurfaceInfo::new(size);
        self.app.surfaces.insert(id.clone(), info);
        window.commit();
        let ok = self.dispatch_until(CONFIGURE_TIMEOUT, |app| app.surfaces.get(&id).is_some_and(|s| s.configured));
        assert!(ok, "the toplevel got no configure");
        // The compositor has handled the buffer commit (the window is mapped) after this.
        self.roundtrip();
        TestWindow { window }
    }

    /// Maps an xdg popup of `size` on `parent`, anchored at the rectangle
    /// `anchor` of the parent (anchor top-left, gravity bottom-right, every
    /// constraint adjustment on): initial commit, waits for the configure,
    /// answers it with a buffer and waits until the compositor has handled it.
    pub fn map_popup(&mut self, parent: &impl PopupParent, anchor: Rect, size: (i32, i32)) -> TestPopup {
        self._map_popup(parent, anchor, size, false)
    }

    /// Maps an xdg popup as `map_popup` does, asking for a popup grab
    /// (`xdg_popup.grab`, a menu) before its first commit, with the serial of
    /// the last keyboard enter this client got.
    pub fn map_popup_grabbed(&mut self, parent: &impl PopupParent, anchor: Rect, size: (i32, i32)) -> TestPopup {
        self._map_popup(parent, anchor, size, true)
    }

    /// Maps an xdg popup with a popup grab (a menu): the same as
    /// `map_popup_grabbed`, under the name the stage 2 tests use.
    pub fn map_popup_with_grab(&mut self, parent: &impl PopupParent, anchor: Rect, size: (i32, i32)) -> TestPopup {
        self._map_popup(parent, anchor, size, true)
    }

    /// Maps an xdg popup (see `map_popup`), with a popup grab when `grab`.
    fn _map_popup(&mut self, parent: &impl PopupParent, anchor: Rect, size: (i32, i32), grab: bool) -> TestPopup {
        let positioner = XdgPositioner::new(&self.app.xdg).expect("xdg_positioner");
        positioner.set_size(size.0, size.1);
        positioner.set_anchor_rect(anchor.x, anchor.y, anchor.w, anchor.h);
        positioner.set_anchor(PositionerAnchor::TopLeft);
        positioner.set_gravity(Gravity::BottomRight);
        positioner.set_constraint_adjustment(
            ConstraintAdjustment::SlideX | ConstraintAdjustment::SlideY | ConstraintAdjustment::FlipX | ConstraintAdjustment::FlipY,
        );
        let surface = Surface::new(&self.app.compositor, &self.qh).expect("popup surface");
        let popup =
            Popup::from_surface(parent.parent_xdg_surface(), &positioner, &self.qh, surface, &self.app.xdg).expect("popup");
        parent.adopt_popup(popup.xdg_popup());
        if grab {
            popup.xdg_popup().grab(&self._seat, self.app.keyboard_serial);
        }
        let id = popup.wl_surface().id();
        let info = SurfaceInfo::new(size);
        self.app.surfaces.insert(id.clone(), info);
        popup.wl_surface().commit();
        let ok = self.dispatch_until(CONFIGURE_TIMEOUT, |app| app.surfaces.get(&id).is_some_and(|s| s.configured));
        assert!(ok, "the popup got no configure");
        self.roundtrip();
        TestPopup { popup }
    }

    /// Maps a layer surface on the default output: sets its size, anchor,
    /// exclusive zone and keyboard interactivity, does the initial commit,
    /// waits for the first configure, answers it with a buffer of the
    /// configured size (the asked size on an axis left at 0) and waits until
    /// the compositor has handled it. The layer surface lives as long as the
    /// client, even when the returned `TestLayer` is dropped.
    pub fn map_layer(
        &mut self,
        layer: Layer,
        anchor: Anchor,
        size: (u32, u32),
        exclusive_zone: i32,
        keyboard: KeyboardInteractivity,
    ) -> TestLayer {
        let surface = self.app.compositor.create_surface(&self.qh);
        self._map_layer_on(Surface::from(surface), layer, anchor, size, exclusive_zone, keyboard)
    }

    /// Maps a new layer surface on the `wl_surface` of `old`, whose layer
    /// role was destroyed by `hide_layer_keeping_surface` (what GTK does when
    /// a hidden layer window is shown again), as `map_layer` does.
    pub fn map_layer_again(
        &mut self,
        old: &TestLayer,
        layer: Layer,
        anchor: Anchor,
        size: (u32, u32),
    ) -> TestLayer {
        let surface = old.wl_surface().clone();
        self._map_layer_on(Surface::from(surface), layer, anchor, size, 0, KeyboardInteractivity::None)
    }

    /// Gives the `wl_surface` of `old` (layer role destroyed by
    /// `hide_layer_keeping_surface`) a new layer surface that never sends
    /// `set_anchor`, asks `size` and does the initial commit. The result of
    /// the roundtrip that follows: an error when the compositor killed the
    /// client (as it must for a size of 0 with no anchor on a fresh surface).
    pub fn layer_again_without_anchor(&mut self, old: &TestLayer, size: (u32, u32)) -> Result<(), String> {
        let surface = Surface::from(old.wl_surface().clone());
        let ls = self.app.layer_shell.create_layer_surface(&self.qh, surface, Layer::Overlay, Some("opusdm-test"), None);
        ls.set_size(size.0, size.1);
        ls.commit();
        let result = self.queue.roundtrip(&mut self.app).map(|_| ()).map_err(|e| e.to_string());
        self.layers.push(ls);
        result
    }

    /// Gives `surface` a layer role and maps it (see `map_layer`).
    fn _map_layer_on(
        &mut self,
        surface: Surface,
        layer: Layer,
        anchor: Anchor,
        size: (u32, u32),
        exclusive_zone: i32,
        keyboard: KeyboardInteractivity,
    ) -> TestLayer {
        let ls = self.app.layer_shell.create_layer_surface(&self.qh, surface, layer, Some("opusdm-test"), None);
        ls.set_size(size.0, size.1);
        ls.set_anchor(anchor);
        ls.set_exclusive_zone(exclusive_zone);
        ls.set_keyboard_interactivity(keyboard);
        let requested = (i32::try_from(size.0).expect("width"), i32::try_from(size.1).expect("height"));
        let id = ls.wl_surface().id();
        self.app.surfaces.insert(id.clone(), SurfaceInfo::new(requested));
        ls.commit();
        let ok = self.dispatch_until(CONFIGURE_TIMEOUT, |app| app.surfaces.get(&id).is_some_and(|s| s.configured));
        assert!(ok, "the layer surface got no configure");
        self.roundtrip();
        self.layers.push(ls.clone());
        TestLayer { layer: ls }
    }

    /// Destroys the layer surface `l` (and its `wl_surface`) and waits until
    /// the compositor has handled it.
    pub fn destroy_layer(&mut self, l: TestLayer) {
        let id = l.wl_surface().id();
        self.layers.retain(|ls| ls.wl_surface().id() != id);
        self.app.surfaces.remove(&id);
        drop(l);
        self.roundtrip();
    }

    /// Hides the layer surface `l` the GTK way: destroys only its
    /// `zwlr_layer_surface_v1` (the `wl_surface` stays), then attaches a null
    /// buffer and commits the bare `wl_surface`. The result of the roundtrip
    /// that follows: an error when the compositor killed the client.
    pub fn hide_layer_keeping_surface(&mut self, l: &TestLayer) -> Result<(), String> {
        let smithay_client_toolkit::shell::wlr_layer::SurfaceKind::Wlr(role) = l.layer.kind() else {
            return Err("not a wlr layer surface".to_string());
        };
        role.destroy();
        let wl = l.wl_surface();
        wl.attach(None, 0, 0);
        wl.commit();
        self.queue.roundtrip(&mut self.app).map(|_| ()).map_err(|e| e.to_string())
    }

    /// Hides the layer surface `l` keeping its role (the wlr-layer-shell
    /// unmap): attaches a null buffer, commits and waits until the
    /// compositor has handled it.
    pub fn hide_layer(&mut self, l: &TestLayer) {
        let wl = l.wl_surface();
        let info = self.app.surfaces.get_mut(&wl.id()).expect("known layer surface");
        info.configured = false;
        wl.attach(None, 0, 0);
        wl.commit();
        self.roundtrip();
    }

    /// Shows the layer surface `l` hidden by `hide_layer` again, as the
    /// protocol asks: a new initial commit (no buffer), then waits for a
    /// configure, answered with a buffer. True when the configure came;
    /// then waits until the compositor has handled the buffer.
    pub fn show_layer_again(&mut self, l: &TestLayer) -> bool {
        let id = l.wl_surface().id();
        l.wl_surface().commit();
        let ok = self.dispatch_until(CONFIGURE_TIMEOUT, |app| app.surfaces.get(&id).is_some_and(|s| s.configured));
        if ok {
            self.roundtrip();
        }
        ok
    }

    /// Size sent by the last configure of the layer surface `l`.
    pub fn layer_configure(&self, l: &TestLayer) -> (u32, u32) {
        let id = l.wl_surface().id();
        self.app.surfaces.get(&id).map(|s| s.layer_size).expect("known layer surface")
    }

    /// True once the compositor dismissed `popup` (`xdg_popup.popup_done`).
    pub fn popup_dismissed(&self, popup: &TestPopup) -> bool {
        self.app.dismissed.contains(&popup.wl_surface().id())
    }

    /// True once the compositor sent `xdg_popup.popup_done` to `popup`: the
    /// same as `popup_dismissed`, under the name the stage 2 tests use.
    pub fn popup_done(&self, popup: &TestPopup) -> bool {
        self.popup_dismissed(popup)
    }

    /// Destroys `popup` (a menu closed by the client) and waits until the
    /// compositor has handled it.
    pub fn destroy_popup(&mut self, popup: TestPopup) {
        let id = popup.wl_surface().id();
        self.app.surfaces.remove(&id);
        drop(popup);
        self.roundtrip();
    }

    /// Waits at most `timeout` until `surface` has this client's keyboard.
    /// True when it has it.
    pub fn wait_keyboard_focus(&mut self, surface: &impl HasSurface, timeout: Duration) -> bool {
        let target = surface.wl_surface().clone();
        self.dispatch_until(timeout, |app| app.keyboard_focus.as_ref() == Some(&target))
    }

    /// Waits at most `timeout` until this client got `count` key events
    /// (presses and releases) of non-modifier keys. True when it did.
    pub fn wait_keys(&mut self, count: u32, timeout: Duration) -> bool {
        self.dispatch_until(timeout, |app| app.keys >= count)
    }

    /// Waits at most `timeout` until `window` got `xdg_toplevel.close`.
    /// True when it did.
    pub fn wait_close_request(&mut self, window: &TestWindow, timeout: Duration) -> bool {
        let id = window.wl_surface().id();
        self.dispatch_until(timeout, |app| app.close_requests.contains(&id))
    }

    /// Key events (presses and releases) of non-modifier keys received so far.
    pub fn keys_received(&self) -> u32 {
        self.app.keys
    }

    /// Surface that has this client's keyboard, from `wl_keyboard` enter / leave.
    pub fn keyboard_focus(&self) -> Option<WlSurface> {
        self.app.keyboard_focus.clone()
    }

    /// Position (relative to the parent's window geometry) and size of the
    /// last configure of `popup`.
    pub fn popup_configure(&self, popup: &TestPopup) -> Rect {
        let id = popup.wl_surface().id();
        self.app.surfaces.get(&id).map(|s| s.popup_rect).expect("known popup")
    }

    /// Unmaps `window` the xdg way: attaches a null buffer, commits and waits
    /// until the compositor has handled it.
    pub fn unmap(&mut self, window: &TestWindow) {
        let wl = window.wl_surface();
        let info = self.app.surfaces.get_mut(&wl.id()).expect("known window");
        info.configured = false;
        wl.attach(None, 0, 0);
        wl.commit();
        self.roundtrip();
    }

    /// Maps an unmapped `window` again: redoes the initial commit (no buffer)
    /// and waits for a configure, answered with a buffer. True when the
    /// configure came; then waits until the compositor has handled the buffer.
    pub fn remap(&mut self, window: &TestWindow) -> bool {
        let id = window.wl_surface().id();
        window.wl_surface().commit();
        let ok = self.dispatch_until(CONFIGURE_TIMEOUT, |app| app.surfaces.get(&id).is_some_and(|s| s.configured));
        if ok {
            self.roundtrip();
        }
        ok
    }

    /// The workspaces the workspace manager of this client lists after a
    /// roundtrip, as of the last manager `done`, in the order they came;
    /// panics when the compositor has no workspace manager global.
    pub fn workspaces(&mut self) -> Vec<TestWorkspace> {
        assert!(self.workspace_manager.is_some(), "the compositor has no ext_workspace_manager_v1");
        self.roundtrip();
        self.app.workspaces_published.clone()
    }

    /// Asks the compositor to activate the workspace at `index` of
    /// `workspaces()` (`activate` + manager `commit`).
    pub fn activate_workspace(&mut self, index: usize) {
        let Some(manager) = &self.workspace_manager else { panic!("the compositor has no ext_workspace_manager_v1") };
        let Some(entry) = self.app.workspaces.get(index) else { panic!("no workspace {index}") };
        entry.handle.activate();
        manager.commit();
        self.roundtrip();
    }

    /// The workspace groups this client knows (not removed).
    pub fn workspace_groups(&mut self) -> usize {
        self.roundtrip();
        self.app.workspace_groups.len()
    }

    /// The toplevels the foreign-toplevel manager of this client lists after
    /// a roundtrip, as of the last `done` of each handle, in the order the
    /// handles came; panics when the compositor has no manager global.
    pub fn toplevels(&mut self) -> Vec<ForeignToplevel> {
        assert!(self.foreign_manager.is_some(), "the compositor has no zwlr_foreign_toplevel_manager_v1");
        self.roundtrip();
        self.app.foreign.iter().filter_map(|e| e.published.clone()).collect()
    }

    /// Asks the compositor to activate the toplevel `t` (`activate` with the seat of this client).
    pub fn activate(&self, t: &ForeignToplevel) {
        t.handle.activate(&self._seat);
    }

    /// Asks the compositor to minimize the toplevel `t` (`set_minimized`).
    pub fn minimize(&self, t: &ForeignToplevel) {
        t.handle.set_minimized();
    }

    /// Sends a `wl_display.sync` and handles every event up to its answer;
    /// panics when the answer does not come within `ROUNDTRIP_TIMEOUT`.
    pub fn roundtrip(&mut self) {
        let done = Arc::new(AtomicBool::new(false));
        self.conn.display().sync(&self.qh, Arc::clone(&done));
        let ok = self.dispatch_until(ROUNDTRIP_TIMEOUT, |_| done.load(Ordering::SeqCst));
        assert!(ok, "roundtrip: no answer from the compositor within {ROUNDTRIP_TIMEOUT:?} (hung?)");
    }

    /// Waits at most `timeout` until the last configure of `window` has
    /// `size`. True when it has.
    pub fn wait_configure(&mut self, window: &TestWindow, size: (i32, i32), timeout: Duration) -> bool {
        let id = window.wl_surface().id();
        self.dispatch_until(timeout, |app| app.surfaces.get(&id).is_some_and(|s| s.last_configure == size))
    }

    /// Requests a frame callback on `surface`, commits and waits at most
    /// `timeout` for it. True when the callback came.
    pub fn wait_frame(&mut self, surface: &impl HasSurface, timeout: Duration) -> bool {
        let wl = surface.wl_surface();
        let id = wl.id();
        self.app.frames.retain(|f| f != &id);
        wl.frame(&self.qh, wl.clone());
        wl.commit();
        self.dispatch_until(timeout, |app| app.frames.contains(&id))
    }

    /// Size of the last configure of `window` (the requested size on an axis
    /// the compositor left to the client).
    pub fn last_configure(&self, window: &TestWindow) -> (i32, i32) {
        let id = window.wl_surface().id();
        self.app.surfaces.get(&id).map(|s| s.last_configure).expect("known window")
    }

    /// Dispatches events until `done` is true or `timeout` passes. True when `done`.
    fn dispatch_until(&mut self, timeout: Duration, done: impl Fn(&ClientApp) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.queue.dispatch_pending(&mut self.app).expect("dispatch");
            if done(&self.app) {
                return true;
            }
            self.conn.flush().expect("flush");
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let Some(guard) = self.queue.prepare_read() else { continue };
            let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let ms = i32::try_from((deadline - now).as_millis()).unwrap_or(i32::MAX).max(1);
            // SAFETY: one valid pollfd for the duration of the call.
            let ready = unsafe { libc::poll(&mut pfd, 1, ms) };
            if ready > 0 {
                guard.read().expect("read events");
            }
        }
    }
}

impl CompositorHandler for ClientApp {
    /// Not used by the tests.
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: i32) {}

    /// Not used by the tests.
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}

    /// Records the frame callback of `surface`.
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _time: u32) {
        self.frames.push(surface.id());
    }

    /// Not used by the tests.
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}

    /// Not used by the tests.
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
}

impl WindowHandler for ClientApp {
    /// The tests close windows by dropping them.
    /// Records the `xdg_toplevel.close` of `window` (the tests close windows by dropping them).
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window) {
        self.close_requests.push(window.wl_surface().id());
    }

    /// Answers every configure (SCTK has already acked it) with a buffer.
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window, configure: WindowConfigure, _serial: u32) {
        let w = configure.new_size.0.map_or(0, |v| i32::try_from(v.get()).unwrap_or(i32::MAX));
        let h = configure.new_size.1.map_or(0, |v| i32::try_from(v.get()).unwrap_or(i32::MAX));
        self.answer_configure(window.wl_surface(), (w, h));
    }
}

impl PopupHandler for ClientApp {
    /// Records the position and size of the configure and answers it with a buffer.
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup, config: PopupConfigure) {
        let id = popup.wl_surface().id();
        if let Some(info) = self.surfaces.get_mut(&id) {
            info.popup_rect = Rect { x: config.position.0, y: config.position.1, w: config.width, h: config.height };
        }
        self.answer_configure(popup.wl_surface(), (config.width, config.height));
    }

    /// Records that the compositor dismissed `popup`.
    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup) {
        self.dismissed.push(popup.wl_surface().id());
    }
}

impl LayerShellHandler for ClientApp {
    /// The tests do not close layer surfaces from the compositor side.
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    /// Records the configured size and answers it (SCTK has already acked it) with a buffer.
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _serial: u32) {
        let (w, h) = configure.new_size;
        if let Some(info) = self.surfaces.get_mut(&layer.wl_surface().id()) {
            info.layer_size = (w, h);
        }
        let size = (i32::try_from(w).unwrap_or(i32::MAX), i32::try_from(h).unwrap_or(i32::MAX));
        self.answer_configure(layer.wl_surface(), size);
    }
}

impl Dispatch<wl_callback::WlCallback, Arc<AtomicBool>> for ClientApp {
    /// Answer to a `wl_display.sync` of `roundtrip`: sets its flag.
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        done: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            done.store(true, Ordering::SeqCst);
        }
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for ClientApp {
    /// Records every new group and workspace, publishes the workspaces on
    /// `done`; `finished` needs nothing (the tests never stop the manager).
    fn event(
        app: &mut Self,
        _: &ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_manager_v1::Event;
        match event {
            Event::WorkspaceGroup { workspace_group } => app.workspace_groups.push(workspace_group),
            Event::Workspace { workspace } => app.workspaces.push(WorkspaceEntry {
                handle: workspace,
                fields: TestWorkspace { name: String::new(), active: false, coordinates: Vec::new() },
            }),
            Event::Done => app.workspaces_published = app.workspaces.iter().map(|e| e.fields.clone()).collect(),
            _ => {}
        }
    }

    event_created_child!(ClientApp, ExtWorkspaceManagerV1, [
        ext_workspace_manager_v1::EVT_WORKSPACE_GROUP_OPCODE => (ExtWorkspaceGroupHandleV1, ()),
        ext_workspace_manager_v1::EVT_WORKSPACE_OPCODE => (ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for ClientApp {
    /// Forgets (and destroys) a removed group; the other group events are not needed.
    fn event(
        app: &mut Self,
        group: &ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_workspace_group_handle_v1::Event::Removed = event {
            app.workspace_groups.retain(|g| g != group);
            group.destroy();
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for ClientApp {
    /// Collects the fields of a workspace and forgets (and destroys) it on
    /// `removed`, as the OpusDM pager does.
    fn event(
        app: &mut Self,
        handle: &ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_workspace_handle_v1::Event;
        if let Event::Removed = event {
            app.workspaces.retain(|e| &e.handle != handle);
            handle.destroy();
            return;
        }
        let Some(entry) = app.workspaces.iter_mut().find(|e| &e.handle == handle) else { panic!("event for an unknown workspace {}", handle.id()) };
        match event {
            Event::Name { name } => entry.fields.name = name,
            Event::Coordinates { coordinates } => {
                entry.fields.coordinates = coordinates.as_chunks::<4>().0.iter().map(|b| u32::from_ne_bytes(*b)).collect();
            }
            Event::State { state: WEnum::Value(flags) } => entry.fields.active = flags.contains(ext_workspace_handle_v1::State::Active),
            Event::State { state: WEnum::Unknown(bits) } => panic!("workspace state with unknown bits {bits:#x}"),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for ClientApp {
    /// Records every new handle; `finished` needs nothing (the tests never stop the manager).
    fn event(
        app: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            app.foreign.push(ForeignEntry {
                handle: toplevel,
                title: String::new(),
                app_id: String::new(),
                states: Vec::new(),
                outputs: Vec::new(),
                published: None,
            });
        }
    }

    event_created_child!(ClientApp, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for ClientApp {
    /// Collects the events of a handle, publishes them on `done` and forgets
    /// the handle on `closed`.
    fn event(
        app: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        if let Event::Closed = event {
            app.foreign.retain(|e| &e.handle != handle);
            return;
        }
        let Some(entry) = app.foreign.iter_mut().find(|e| &e.handle == handle) else { panic!("event for an unknown handle {}", handle.id()) };
        match event {
            Event::Title { title } => entry.title = title,
            Event::AppId { app_id } => entry.app_id = app_id,
            Event::State { state } => {
                entry.states = state.as_chunks::<4>().0.iter().map(|b| u32::from_ne_bytes(*b)).collect();
            }
            Event::OutputEnter { output } => entry.outputs.push(output.id()),
            Event::OutputLeave { output } => entry.outputs.retain(|o| *o != output.id()),
            Event::Done => {
                entry.published = Some(ForeignToplevel {
                    title: entry.title.clone(),
                    app_id: entry.app_id.clone(),
                    states: entry.states.clone(),
                    outputs: entry.outputs.len(),
                    handle: entry.handle.clone(),
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for ClientApp {
    /// The keyboard is asked for at bind time: seat events are not needed.
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientApp {
    /// Tracks the surface that has the keyboard and the enter serial (enter /
    /// leave) and counts the key events of non-modifier keys.
    fn event(app: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Enter { serial, surface, .. } => {
                app.keyboard_serial = serial;
                app.keyboard_focus = Some(surface);
            }
            wl_keyboard::Event::Leave { surface, .. } if app.keyboard_focus.as_ref() == Some(&surface) => {
                app.keyboard_focus = None;
            }
            wl_keyboard::Event::Key { key, .. } if !MODIFIER_KEYCODES.contains(&key) => {
                app.keys += 1;
            }
            _ => {}
        }
    }
}

impl OutputHandler for ClientApp {
    /// The output state.
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    /// Not used by the tests.
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    /// Not used by the tests.
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    /// Not used by the tests.
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for ClientApp {
    /// The shm global.
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for ClientApp {
    /// The registry state.
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(ClientApp);
delegate_output!(ClientApp);
delegate_shm!(ClientApp);
delegate_xdg_shell!(ClientApp);
delegate_xdg_popup!(ClientApp);
delegate_xdg_window!(ClientApp);
delegate_layer!(ClientApp);
delegate_registry!(ClientApp);
