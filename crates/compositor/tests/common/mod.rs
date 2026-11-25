//! Test harness: starts `opusdm-compositor --headless` and drives it with a
//! smithay-client-toolkit client connected to its socket.
//!
//! Every `Compositor` has its own `XDG_RUNTIME_DIR` and the client connects to
//! the socket path directly, never through the process environment, so the
//! tests run in parallel.

// Each test binary uses only a part of the harness.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface::WlSurface},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
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

/// A running headless compositor, killed when dropped.
pub struct Compositor {
    /// Path of the Wayland socket.
    pub socket: PathBuf,
    child: Child,
    _runtime: TempDir,
}

impl Compositor {
    /// Starts the compositor in a fresh `XDG_RUNTIME_DIR` and waits (at most
    /// `START_TIMEOUT`) for the `WAYLAND_DISPLAY=<name>` line on its stdout.
    pub fn start() -> Compositor {
        let runtime = tempfile::tempdir().expect("temp XDG_RUNTIME_DIR");
        let mut child = Command::new(env!("CARGO_BIN_EXE_opusdm-compositor"))
            .args(["--headless", "--size", OUTPUT_SIZE])
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn opusdm-compositor");
        let stdout = child.stdout.take().expect("compositor stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(read.map(|_| line));
        });
        let line = match rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(line)) => line,
            other => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("compositor did not print its socket: {other:?}");
            }
        };
        let Some(name) = line.trim().strip_prefix("WAYLAND_DISPLAY=") else {
            let _ = child.kill();
            let status = child.wait();
            panic!("compositor did not print WAYLAND_DISPLAY (got {line:?}, exit {status:?})");
        };
        let socket = runtime.path().join(name);
        Compositor { socket, child, _runtime: runtime }
    }

    /// True while the compositor process has not exited.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
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

impl Drop for Compositor {
    /// Kills the compositor and reaps it.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Opens a client connection to `comp` through its socket path.
pub fn connect(comp: &Compositor) -> Connection {
    let stream = UnixStream::connect(&comp.socket).expect("connect to the compositor socket");
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
}

impl TestClient {
    /// Connects to `comp` and binds the globals the tests need.
    pub fn new(comp: &Compositor) -> TestClient {
        let conn = connect(comp);
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
        };
        let mut client = TestClient { conn, queue, qh, app, _seat: seat, _keyboard: keyboard, layers: Vec::new() };
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

    /// Size sent by the last configure of the layer surface `l`.
    pub fn layer_configure(&self, l: &TestLayer) -> (u32, u32) {
        let id = l.wl_surface().id();
        self.app.surfaces.get(&id).map(|s| s.layer_size).expect("known layer surface")
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

    /// Sends a `wl_display.sync` and handles every event up to its answer.
    pub fn roundtrip(&mut self) {
        self.queue.roundtrip(&mut self.app).expect("roundtrip");
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
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

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

    /// The tests do not dismiss popups.
    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Popup) {}
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

impl Dispatch<wl_seat::WlSeat, ()> for ClientApp {
    /// The keyboard is asked for at bind time: seat events are not needed.
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientApp {
    /// Tracks the surface that has the keyboard (enter / leave); keys are ignored.
    fn event(app: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => app.keyboard_focus = Some(surface),
            wl_keyboard::Event::Leave { surface, .. } if app.keyboard_focus.as_ref() == Some(&surface) => {
                app.keyboard_focus = None;
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
