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
            window::{DecorationMode, Window, WindowConfigure, WindowHandler},
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
    protocol::{
        wl_callback, wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_keyboard, wl_output,
        wl_pointer, wl_seat, wl_shm, wl_surface::WlSurface,
    },
    event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
    zwp_primary_selection_device_v1::{self, ZwpPrimarySelectionDeviceV1},
    zwp_primary_selection_offer_v1::{self, ZwpPrimarySelectionOfferV1},
    zwp_primary_selection_source_v1::{self, ZwpPrimarySelectionSourceV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
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
use wayland_protocols::xdg::activation::v1::client::{
    xdg_activation_token_v1::{self, XdgActivationTokenV1},
    xdg_activation_v1::XdgActivationV1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup::XdgPopup,
    xdg_positioner::{Anchor as PositionerAnchor, ConstraintAdjustment, Gravity},
    xdg_surface::XdgSurface,
    xdg_toplevel::XdgToplevel,
};

pub use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer};
pub use smithay_client_toolkit::shell::xdg::window::WindowDecorations;

/// How long `Compositor::start` waits for the socket name.
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a surface waits for its first configure.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long `TestClient::roundtrip` waits for the answer to its sync: a
/// hung compositor fails the test instead of hanging `cargo test`.
const ROUNDTRIP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a screen capture waits for each step (`buffer_done`, `ready`
/// or `failed`).
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);

/// Byte the test fills a capture buffer with before `copy`, so a pixel the
/// compositor did not write is visible.
pub const CAPTURE_FILL: u8 = 0x55;

/// How long `TestClient::activation_token` waits for the token.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(2);

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
    /// Every stderr line of the compositor so far (also echoed to stderr).
    log: Arc<std::sync::Mutex<Vec<String>>>,
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
        Compositor::_start(false, &[])
    }

    /// Starts the compositor as `start` does, with `OPUSDM_DEBUG` set, so its
    /// `debug_log!` lines reach `wait_log` / `log_count`.
    pub fn start_debug() -> Compositor {
        Compositor::_start(true, &[])
    }

    /// Starts the compositor as `start` does, after writing each `(name,
    /// text)` of `files` into its OpusDM config directory (read at startup).
    pub fn start_with_config(files: &[(&str, &str)]) -> Compositor {
        Compositor::_start(false, files)
    }

    /// Starts the compositor (see `start`); `debug` sets `OPUSDM_DEBUG`,
    /// `files` are written into the config directory first.
    fn _start(debug: bool, files: &[(&str, &str)]) -> Compositor {
        let runtime = tempfile::tempdir().expect("temp XDG_RUNTIME_DIR");
        if !files.is_empty() {
            let dir = runtime.path().join("config").join("opusdm");
            std::fs::create_dir_all(&dir).expect("create the test config dir");
            for (name, text) in files {
                std::fs::write(dir.join(name), text).expect("write the test config file");
            }
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_opusdm-compositor"));
        if debug {
            command.env("OPUSDM_DEBUG", "1");
        } else {
            command.env_remove("OPUSDM_DEBUG");
        }
        let mut child = command
            .args(["--headless", "--size", OUTPUT_SIZE])
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("HOME", runtime.path())
            .env("XDG_CONFIG_HOME", runtime.path().join("config"))
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("OPUSDM_COMPOSITOR_CONFIG")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("compositor stderr");
        let sink = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                eprintln!("{line}");
                sink.lock().expect("log lock").push(line);
            }
        });
        let name = _read_announce(&mut child, &rx, "WAYLAND_DISPLAY=");
        let ipc = _read_announce(&mut child, &rx, "OPUSDM_COMPOSITOR_SOCKET=");
        let socket = runtime.path().join(name);
        Compositor { socket, ipc_socket: PathBuf::from(ipc), child, _runtime: runtime, log }
    }

    /// Waits at most `timeout` for a stderr line of the compositor that
    /// contains `text`. True when one came.
    pub fn wait_log(&self, text: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.log_count(text) > 0 { return true; }
            if Instant::now() >= deadline { return false; }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Number of stderr lines so far that contain `text`.
    pub fn log_count(&self, text: &str) -> usize {
        self.log.lock().expect("log lock").iter().filter(|l| l.contains(text)).count()
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

    /// Writes `text` to the file `name` of the compositor's OpusDM config
    /// directory (`$XDG_CONFIG_HOME/opusdm`, inside the test's temp dir),
    /// for a later `reload`.
    pub fn write_config(&self, name: &str, text: &str) {
        let dir = self._runtime.path().join("config").join("opusdm");
        std::fs::create_dir_all(&dir).expect("create the test config dir");
        std::fs::write(dir.join(name), text).expect("write the test config file");
    }

    /// Path of the file `name` of the compositor's OpusDM config directory
    /// (inside the test's temp dir); the directory is created.
    pub fn config_path(&self, name: &str) -> PathBuf {
        let dir = self._runtime.path().join("config").join("opusdm");
        std::fs::create_dir_all(&dir).expect("create the test config dir");
        dir.join(name)
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

    /// Asks for server side (`true`) or client side (`false`) decorations
    /// on the decoration object of the window (one made by
    /// `map_toplevel_decorated` with a mode or `ServerDefault`); the test
    /// waits for the configure after it.
    pub fn request_server_side(&self, server: bool) {
        let mode = if server { DecorationMode::Server } else { DecorationMode::Client };
        self.window.request_decoration_mode(Some(mode));
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

/// The drag and drop icon surface of a drag started by a `TestClient`.
pub struct TestIcon {
    surface: WlSurface,
}

impl HasSurface for TestIcon {
    /// The icon surface.
    fn wl_surface(&self) -> &WlSurface {
        &self.surface
    }
}

/// Reads `fd` until end of file, waiting at most `timeout` in all: the
/// bytes read, or an error when the end of file did not come in time (a
/// transfer that hangs).
pub fn read_pipe(fd: std::os::fd::OwnedFd, timeout: Duration) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let deadline = Instant::now() + timeout;
    let mut file = std::fs::File::from(fd);
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(format!("no end of file within {timeout:?} ({} bytes read)", out.len()));
        }
        let mut pfd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = i32::try_from((deadline - now).as_millis()).unwrap_or(i32::MAX).max(1);
        // SAFETY: one valid pollfd for the duration of the call.
        if unsafe { libc::poll(&mut pfd, 1, ms) } <= 0 {
            continue;
        }
        match file.read(&mut chunk) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(format!("read: {e}")),
        }
    }
}

/// A new close-on-exec pipe: (read end, write end).
fn _pipe() -> (std::os::fd::OwnedFd, std::os::fd::OwnedFd) {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe2 writes.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    assert_eq!(rc, 0, "pipe2: {}", std::io::Error::last_os_error());
    // SAFETY: pipe2 succeeded, both descriptors are new and owned here only.
    unsafe { (std::os::fd::OwnedFd::from_raw_fd(fds[0]), std::os::fd::OwnedFd::from_raw_fd(fds[1])) }
}

/// True when the offer `offer` (by id) announced `mime`.
fn _offer_has(app: &ClientApp, offer: Option<ObjectId>, mime: &str) -> bool {
    offer.and_then(|id| app.offer_mimes.get(&id)).is_some_and(|m| m.iter().any(|t| t == mime))
}

/// Writes the bytes of source `source` into `fd` and closes it (the
/// `send` event of a data device or primary selection source).
fn _write_source(app: &ClientApp, source: &ObjectId, fd: std::os::fd::OwnedFd) {
    use std::io::Write;
    let data = app.source_data.get(source).cloned().unwrap_or_default();
    let mut file = std::fs::File::from(fd);
    file.write_all(&data).expect("source write");
}

/// What a `zwlr_screencopy_frame_v1` told the client so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureEvents {
    /// The `buffer` event: `wl_shm` format, width, height and stride.
    pub buffer: Option<(u32, u32, u32, u32)>,
    /// A `buffer_done` event came (version 3).
    pub buffer_done: bool,
    /// A `linux_dmabuf` event came.
    pub linux_dmabuf: bool,
    /// The `flags` event.
    pub flags: Option<u32>,
    /// The `damage` event: x, y, width, height.
    pub damage: Option<(u32, u32, u32, u32)>,
    /// A `ready` event came.
    pub ready: bool,
    /// A `failed` event came.
    pub failed: bool,
}

/// A screen capture frame of a `TestClient`.
pub struct TestCapture {
    /// The frame.
    pub frame: ZwlrScreencopyFrameV1,
    /// The buffer handed to `copy`, kept alive while the compositor may write it.
    buffer: Option<Buffer>,
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
    /// True when the last toplevel configure had the server side decoration
    /// mode (`zxdg_toplevel_decoration_v1.configure` with `server_side`).
    server_side: bool,
}

impl SurfaceInfo {
    /// Info of a new surface the test wants of size `requested`.
    fn new(requested: (i32, i32)) -> SurfaceInfo {
        SurfaceInfo { requested, last_configure: (0, 0), configured: false, buffer: None, popup_rect: Rect::default(), layer_size: (0, 0), server_side: false }
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
    /// `mods_depressed` of the last `wl_keyboard.modifiers` (0: no modifier held).
    mods_depressed: u32,
    /// Surface with the pointer of this client and the surface-local
    /// position of the pointer on it (`wl_pointer` enter / motion / leave).
    pointer: Option<(WlSurface, f64, f64)>,
    /// Every `wl_pointer.button` press: the surface with the pointer and the
    /// surface-local position at that moment.
    presses: Vec<(WlSurface, f64, f64)>,
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
    /// Events of every screen capture frame, by frame id.
    captures: HashMap<ObjectId, CaptureEvents>,
    /// Serial of the last `wl_pointer.button` press (for `start_drag`).
    button_serial: u32,
    /// Mime types announced by every data offer (clipboard, primary and
    /// drag and drop), by offer id.
    offer_mimes: HashMap<ObjectId, Vec<String>>,
    /// Number of `wl_data_device.selection` events and the offer of the last
    /// one (`None`: the clipboard was cleared).
    clipboard_events: u32,
    clipboard_offer: Option<wl_data_offer::WlDataOffer>,
    /// Number of `zwp_primary_selection_device_v1.selection` events and the
    /// offer of the last one (`None`: the primary selection was cleared).
    primary_events: u32,
    primary_offer: Option<ZwpPrimarySelectionOfferV1>,
    /// Drag and drop events of the data device, in order: "enter",
    /// "motion", "leave", "drop".
    dnd_events: Vec<&'static str>,
    /// The offer and the serial of the last `wl_data_device.enter`.
    dnd_offer: Option<wl_data_offer::WlDataOffer>,
    dnd_serial: u32,
    /// Bytes each source of this client writes on `send`, by source id
    /// (data device and primary selection sources).
    source_data: HashMap<ObjectId, Vec<u8>>,
    /// Events of the sources of this client, in order: "send", "cancelled",
    /// "dnd_drop_performed", "dnd_finished".
    source_events: Vec<&'static str>,
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
    /// The `xdg_activation_v1`, when the compositor has it.
    activation: Option<XdgActivationV1>,
    /// The `zwlr_screencopy_manager_v1`, when the compositor has it.
    screencopy: Option<ZwlrScreencopyManagerV1>,
    /// `wl_data_device_manager` and this client's `wl_data_device` (clipboard
    /// and drag and drop), when the compositor has the global.
    data_manager: Option<(wl_data_device_manager::WlDataDeviceManager, wl_data_device::WlDataDevice)>,
    /// `zwp_primary_selection_device_manager_v1` and this client's device,
    /// when the compositor has the global.
    primary_manager: Option<(ZwpPrimarySelectionDeviceManagerV1, ZwpPrimarySelectionDeviceV1)>,
    /// Data sources of this client, kept alive for the whole client.
    data_sources: Vec<wl_data_source::WlDataSource>,
    primary_sources: Vec<ZwpPrimarySelectionSourceV1>,
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
        let _pointer = seat.get_pointer(&qh, ());
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
            mods_depressed: 0,
            pointer: None,
            presses: Vec::new(),
            close_requests: Vec::new(),
            foreign: Vec::new(),
            workspaces: Vec::new(),
            workspaces_published: Vec::new(),
            workspace_groups: Vec::new(),
            captures: HashMap::new(),
            button_serial: 0,
            offer_mimes: HashMap::new(),
            clipboard_events: 0,
            clipboard_offer: None,
            primary_events: 0,
            primary_offer: None,
            dnd_events: Vec::new(),
            dnd_offer: None,
            dnd_serial: 0,
            source_data: HashMap::new(),
            source_events: Vec::new(),
        };
        // Bound after `wl_output` (bound by `OutputState::new`); `None` when the compositor has no such global.
        let foreign_manager = globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ()).ok();
        let workspace_manager = globals.bind::<ExtWorkspaceManagerV1, _, _>(&qh, 1..=1, ()).ok();
        let activation = globals.bind::<XdgActivationV1, _, _>(&qh, 1..=1, ()).ok();
        let screencopy = globals.bind::<ZwlrScreencopyManagerV1, _, _>(&qh, 1..=3, ()).ok();
        let data_manager = globals.bind::<wl_data_device_manager::WlDataDeviceManager, _, _>(&qh, 3..=3, ()).ok().map(|m| {
            let device = m.get_data_device(&seat, &qh, ());
            (m, device)
        });
        let primary_manager = globals.bind::<ZwpPrimarySelectionDeviceManagerV1, _, _>(&qh, 1..=1, ()).ok().map(|m| {
            let device = m.get_device(&seat, &qh, ());
            (m, device)
        });
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
            activation,
            screencopy,
            data_manager,
            primary_manager,
            data_sources: Vec::new(),
            primary_sources: Vec::new(),
        };
        client.roundtrip();
        client
    }

    /// Maps an xdg toplevel of `size`: initial commit, waits for the first
    /// configure, answers it with a buffer and waits until the compositor has
    /// handled that commit.
    pub fn map_toplevel(&mut self, size: (i32, i32)) -> TestWindow {
        self.map_toplevel_decorated(size, WindowDecorations::None)
    }

    /// Maps an xdg toplevel of `size` as `map_toplevel` does, asking for
    /// `decorations` through `zxdg_decoration_manager_v1` before the initial
    /// commit (`RequestServer`: `set_mode(server_side)`, `RequestClient`:
    /// `set_mode(client_side)`, `ServerDefault`: a decoration object and no
    /// mode, `None` / `ClientOnly`: no decoration object at all).
    pub fn map_toplevel_decorated(&mut self, size: (i32, i32), decorations: WindowDecorations) -> TestWindow {
        let surface = self.app.compositor.create_surface(&self.qh);
        let window = self.app.xdg.create_window(surface, decorations, &self.qh);
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

    /// Maps an xdg toplevel of `size` as `map_toplevel` does, sending
    /// `xdg_activation_v1.activate` with `token` right after its initial
    /// commit, while the compositor has not mapped it yet.
    pub fn map_toplevel_activated(&mut self, size: (i32, i32), token: &str) -> TestWindow {
        let surface = self.app.compositor.create_surface(&self.qh);
        let window = self.app.xdg.create_window(surface, WindowDecorations::None, &self.qh);
        window.set_title("opusdm-compositor test");
        let id = window.wl_surface().id();
        self.app.surfaces.insert(id.clone(), SurfaceInfo::new(size));
        window.commit();
        self._activation().activate(token.to_string(), window.wl_surface());
        let ok = self.dispatch_until(CONFIGURE_TIMEOUT, |app| app.surfaces.get(&id).is_some_and(|s| s.configured));
        assert!(ok, "the toplevel got no configure");
        self.roundtrip();
        TestWindow { window }
    }

    /// Creates an xdg toplevel, does its initial commit, sends
    /// `xdg_activation_v1.activate` with `token` and destroys it in the same
    /// batch of requests (it never maps), then waits for the compositor.
    pub fn activate_toplevel_destroyed_before_map(&mut self, token: &str) {
        let surface = self.app.compositor.create_surface(&self.qh);
        let window = self.app.xdg.create_window(surface, WindowDecorations::None, &self.qh);
        window.commit();
        self._activation().activate(token.to_string(), window.wl_surface());
        drop(window);
        self.roundtrip();
    }

    /// Asks the compositor for an xdg-activation token (with the serial of
    /// the last keyboard enter of this client) and waits for it.
    pub fn activation_token(&mut self) -> String {
        let cell: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
        let token = self._activation().get_activation_token(&self.qh, Arc::clone(&cell));
        token.set_serial(self.app.keyboard_serial, &self._seat);
        token.commit();
        let ok = self.dispatch_until(TOKEN_TIMEOUT, |_| cell.lock().expect("token lock").is_some());
        assert!(ok, "no activation token within {TOKEN_TIMEOUT:?}");
        token.destroy();
        let value = cell.lock().expect("token lock").clone();
        value.expect("token present")
    }

    /// Asks the compositor to activate the surface of `w` (a window, a layer
    /// surface) with `token` (`xdg_activation_v1.activate`); the test
    /// roundtrips after it.
    pub fn request_activation(&mut self, w: &impl HasSurface, token: &str) {
        self._activation().activate(token.to_string(), w.wl_surface());
    }

    /// The `xdg_activation_v1` of this client; panics when the compositor
    /// has no such global.
    fn _activation(&self) -> &XdgActivationV1 {
        self.activation.as_ref().expect("the compositor has no xdg_activation_v1")
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

    /// Changes the keyboard interactivity of the mapped layer surface `l`,
    /// commits and waits until the compositor has handled it.
    pub fn set_layer_keyboard(&mut self, l: &TestLayer, keyboard: KeyboardInteractivity) {
        l.layer.set_keyboard_interactivity(keyboard);
        l.layer.commit();
        self.roundtrip();
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

    /// `mods_depressed` of the last `wl_keyboard.modifiers` this client got
    /// (0 when no modifier is held, as the client sees it).
    pub fn modifiers_depressed(&self) -> u32 {
        self.app.mods_depressed
    }

    /// Surface that has this client's keyboard, from `wl_keyboard` enter / leave.
    pub fn keyboard_focus(&self) -> Option<WlSurface> {
        self.app.keyboard_focus.clone()
    }

    /// Every pointer button press this client got so far: the surface with
    /// the pointer and the surface-local position at that moment.
    pub fn pointer_presses(&self) -> Vec<(WlSurface, f64, f64)> {
        self.app.presses.clone()
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

    /// Version of the bound `zwlr_screencopy_manager_v1`; panics when the
    /// compositor has none.
    pub fn screencopy_version(&self) -> u32 {
        self.screencopy.as_ref().expect("the compositor offers zwlr_screencopy_manager_v1").version()
    }

    /// Asks for a capture of the first output (`region`: only that box of
    /// it, output coordinates) and waits for `buffer_done` or `failed`.
    pub fn capture_frame(&mut self, region: Option<Rect>) -> TestCapture {
        let manager = self.screencopy.clone().expect("the compositor offers zwlr_screencopy_manager_v1");
        let output = self.app.output_state.outputs().next().expect("an output");
        let frame = match region {
            None => manager.capture_output(0, &output, &self.qh, ()),
            Some(r) => manager.capture_output_region(0, &output, r.x, r.y, r.w, r.h, &self.qh, ()),
        };
        self.app.captures.insert(frame.id(), CaptureEvents::default());
        let id = frame.id();
        let done = self.dispatch_until(CAPTURE_TIMEOUT, |app| app.captures.get(&id).is_some_and(|e| e.buffer_done || e.failed));
        assert!(done, "capture: neither buffer_done nor failed within {CAPTURE_TIMEOUT:?}");
        TestCapture { frame, buffer: None }
    }

    /// What the frame of `capture` reported so far.
    pub fn capture_events(&self, capture: &TestCapture) -> CaptureEvents {
        self.app.captures.get(&capture.frame.id()).cloned().expect("known capture")
    }

    /// Sends `copy` (or `copy_with_damage`) with a fresh shm buffer of
    /// `size` and `format` filled with `CAPTURE_FILL`, without waiting.
    pub fn capture_send_copy(&mut self, capture: &mut TestCapture, size: (i32, i32), format: wl_shm::Format, with_damage: bool) {
        let (buffer, canvas) = self.app.pool.create_buffer(size.0, size.1, size.0 * 4, format).expect("shm buffer");
        canvas.fill(CAPTURE_FILL);
        if with_damage {
            capture.frame.copy_with_damage(buffer.wl_buffer());
        } else {
            capture.frame.copy(buffer.wl_buffer());
        }
        capture.buffer = Some(buffer);
        self.conn.flush().expect("flush");
    }

    /// Sends `copy` as `capture_send_copy` and waits for `ready` or
    /// `failed`; returns the pixels of the buffer on `ready`.
    pub fn capture_copy(
        &mut self,
        capture: &mut TestCapture,
        size: (i32, i32),
        format: wl_shm::Format,
        with_damage: bool,
    ) -> Option<Vec<u8>> {
        self.capture_send_copy(capture, size, format, with_damage);
        let id = capture.frame.id();
        let done = self.dispatch_until(CAPTURE_TIMEOUT, |app| app.captures.get(&id).is_some_and(|e| e.ready || e.failed));
        assert!(done, "capture: neither ready nor failed within {CAPTURE_TIMEOUT:?}");
        if !self.capture_events(capture).ready { return None; }
        let buffer = capture.buffer.as_ref().expect("buffer kept");
        Some(buffer.canvas(&mut self.app.pool).expect("capture canvas").to_vec())
    }

    /// Destroys the `wl_buffer` handed to `copy` while the frame stays alive.
    pub fn capture_drop_buffer(&mut self, capture: &mut TestCapture) {
        // Dropping the slot buffer (never attached) destroys its `wl_buffer`.
        drop(capture.buffer.take());
        self.conn.flush().expect("flush");
    }

    /// Waits for `ready` or `failed` on the frame of `capture` and returns
    /// its events; panics when neither comes within `CAPTURE_TIMEOUT`.
    pub fn capture_wait(&mut self, capture: &TestCapture) -> CaptureEvents {
        let id = capture.frame.id();
        let done = self.dispatch_until(CAPTURE_TIMEOUT, |app| app.captures.get(&id).is_some_and(|e| e.ready || e.failed));
        assert!(done, "capture: neither ready nor failed within {CAPTURE_TIMEOUT:?}");
        self.capture_events(capture)
    }

    /// A whole capture as `opusdm-shot` does it: frame, then `copy` with the
    /// announced Xrgb8888 buffer, then destroy. The events and the pixels
    /// (on `ready`).
    pub fn capture(&mut self, region: Option<Rect>) -> (CaptureEvents, Option<Vec<u8>>) {
        let mut capture = self.capture_frame(region);
        let events = self.capture_events(&capture);
        let Some((_, w, h, _)) = events.buffer.filter(|_| !events.failed) else {
            self.capture_destroy(capture);
            return (events, None);
        };
        let pixels = self.capture_copy(&mut capture, (w as i32, h as i32), wl_shm::Format::Xrgb8888, false);
        let events = self.capture_events(&capture);
        self.capture_destroy(capture);
        (events, pixels)
    }

    /// Destroys the frame of `capture` and its buffer.
    pub fn capture_destroy(&mut self, capture: TestCapture) {
        capture.frame.destroy();
        self.app.captures.remove(&capture.frame.id());
        // Dropping the slot buffer destroys its `wl_buffer` and frees the slot.
        drop(capture.buffer);
        self.conn.flush().expect("flush");
    }

    /// True when the compositor has `zwp_primary_selection_device_manager_v1`.
    pub fn has_primary_selection(&self) -> bool {
        self.primary_manager.is_some()
    }

    /// Sets the clipboard to a new source offering `mime` that writes `text`
    /// (`wl_data_device.set_selection` with the last keyboard enter serial)
    /// and waits until the compositor has handled it.
    pub fn set_clipboard(&mut self, mime: &str, text: &str) {
        let source = self._data_source(mime, text);
        let (_, device) = self.data_manager.as_ref().expect("wl_data_device_manager");
        device.set_selection(Some(&source), self.app.keyboard_serial);
        self.roundtrip();
    }

    /// Sets the primary selection to a new source offering `mime` that
    /// writes `text` and waits until the compositor has handled it.
    pub fn set_primary(&mut self, mime: &str, text: &str) {
        let (manager, device) = self.primary_manager.as_ref().expect("zwp_primary_selection_device_manager_v1");
        let source = manager.create_source(&self.qh, ());
        source.offer(mime.to_string());
        device.set_selection(Some(&source), self.app.keyboard_serial);
        self.app.source_data.insert(source.id(), text.as_bytes().to_vec());
        self.primary_sources.push(source);
        self.roundtrip();
    }

    /// Number of `wl_data_device.selection` events received so far.
    pub fn clipboard_events(&self) -> u32 {
        self.app.clipboard_events
    }

    /// Number of `zwp_primary_selection_device_v1.selection` events so far.
    pub fn primary_events(&self) -> u32 {
        self.app.primary_events
    }

    /// Waits at most `timeout` until the current clipboard offer has `mime`.
    pub fn wait_clipboard_offer(&mut self, mime: &str, timeout: Duration) -> bool {
        self.dispatch_until(timeout, |app| _offer_has(app, app.clipboard_offer.as_ref().map(Proxy::id), mime))
    }

    /// Waits at most `timeout` until the current primary offer has `mime`.
    pub fn wait_primary_offer(&mut self, mime: &str, timeout: Duration) -> bool {
        self.dispatch_until(timeout, |app| _offer_has(app, app.primary_offer.as_ref().map(Proxy::id), mime))
    }

    /// Waits at most `timeout` for a `wl_data_device.selection` event after
    /// the first `after` ones that clears the clipboard (no offer).
    pub fn wait_clipboard_cleared(&mut self, after: u32, timeout: Duration) -> bool {
        self.dispatch_until(timeout, |app| app.clipboard_events > after && app.clipboard_offer.is_none())
    }

    /// Asks the current clipboard offer for `mime` into a new pipe and
    /// returns its read end (the write end is closed once sent).
    pub fn receive_clipboard(&mut self, mime: &str) -> std::os::fd::OwnedFd {
        let offer = self.app.clipboard_offer.clone().expect("a clipboard offer");
        let (read, write) = _pipe();
        offer.receive(mime.to_string(), std::os::fd::AsFd::as_fd(&write));
        self.conn.flush().expect("flush");
        read
    }

    /// Asks the current primary offer for `mime` into a new pipe and
    /// returns its read end.
    pub fn receive_primary(&mut self, mime: &str) -> std::os::fd::OwnedFd {
        let offer = self.app.primary_offer.clone().expect("a primary offer");
        let (read, write) = _pipe();
        offer.receive(mime.to_string(), std::os::fd::AsFd::as_fd(&write));
        self.conn.flush().expect("flush");
        read
    }

    /// Starts a drag from `origin` with the serial of the last button
    /// press: a new source offering `mime` (copy action) that writes
    /// `text`, and a 16x16 icon surface with a buffer. Returns the icon.
    pub fn start_drag(&mut self, origin: &impl HasSurface, mime: &str, text: &str) -> TestIcon {
        let source = self._data_source(mime, text);
        source.set_actions(wl_data_device_manager::DndAction::Copy);
        let icon = self.app.compositor.create_surface(&self.qh);
        let (buffer, canvas) = self.app.pool.create_buffer(16, 16, 64, wl_shm::Format::Argb8888).expect("shm buffer");
        canvas.fill(0xff);
        let (_, device) = self.data_manager.as_ref().expect("wl_data_device_manager");
        device.start_drag(Some(&source), origin.wl_surface(), Some(&icon), self.app.button_serial);
        buffer.attach_to(&icon).expect("attach buffer");
        icon.damage_buffer(0, 0, 16, 16);
        icon.commit();
        let mut info = SurfaceInfo::new((16, 16));
        info.configured = true;
        info.buffer = Some(buffer);
        self.app.surfaces.insert(icon.id(), info);
        self.roundtrip();
        TestIcon { surface: icon }
    }

    /// Drag and drop events of the data device so far ("enter", "motion",
    /// "leave", "drop").
    pub fn dnd_events(&self) -> Vec<&'static str> {
        self.app.dnd_events.clone()
    }

    /// Waits at most `timeout` for the drag and drop event `name`.
    pub fn wait_dnd_event(&mut self, name: &str, timeout: Duration) -> bool {
        self.dispatch_until(timeout, |app| app.dnd_events.contains(&name))
    }

    /// Accepts `mime` with the copy action on the offer of the last enter.
    pub fn dnd_accept(&mut self, mime: &str) {
        let offer = self.app.dnd_offer.clone().expect("a drag and drop offer");
        offer.accept(self.app.dnd_serial, Some(mime.to_string()));
        let copy = wl_data_device_manager::DndAction::Copy;
        offer.set_actions(copy, copy);
        self.roundtrip();
    }

    /// Asks the drag and drop offer for `mime` into a new pipe and returns
    /// its read end.
    pub fn receive_dnd(&mut self, mime: &str) -> std::os::fd::OwnedFd {
        let offer = self.app.dnd_offer.clone().expect("a drag and drop offer");
        let (read, write) = _pipe();
        offer.receive(mime.to_string(), std::os::fd::AsFd::as_fd(&write));
        self.conn.flush().expect("flush");
        read
    }

    /// Ends the drop: `wl_data_offer.finish` on the offer of the last enter.
    pub fn dnd_finish(&mut self) {
        let offer = self.app.dnd_offer.clone().expect("a drag and drop offer");
        offer.finish();
        self.roundtrip();
    }

    /// Events of this client's data sources so far ("send", "cancelled",
    /// "dnd_drop_performed", "dnd_finished").
    pub fn source_events(&self) -> Vec<&'static str> {
        self.app.source_events.clone()
    }

    /// Waits at most `timeout` for the source event `name`.
    pub fn wait_source_event(&mut self, name: &str, timeout: Duration) -> bool {
        self.dispatch_until(timeout, |app| app.source_events.contains(&name))
    }

    /// Destroys every `wl_data_source` of this client (the client and its
    /// windows stay) and waits until the compositor has handled it.
    pub fn destroy_data_sources(&mut self) {
        for source in self.data_sources.drain(..) {
            source.destroy();
        }
        self.roundtrip();
    }

    /// A new `wl_data_source` offering `mime` that writes `text` on `send`.
    fn _data_source(&mut self, mime: &str, text: &str) -> wl_data_source::WlDataSource {
        let (manager, _) = self.data_manager.as_ref().expect("wl_data_device_manager");
        let source = manager.create_data_source(&self.qh, ());
        source.offer(mime.to_string());
        self.app.source_data.insert(source.id(), text.as_bytes().to_vec());
        self.data_sources.push(source.clone());
        source
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

    /// True when the last configure of `window` had the server side
    /// decoration mode.
    pub fn server_side(&self, window: &TestWindow) -> bool {
        let id = window.wl_surface().id();
        self.app.surfaces.get(&id).map(|s| s.server_side).expect("known window")
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
        if let Some(info) = self.surfaces.get_mut(&window.wl_surface().id()) {
            info.server_side = configure.decoration_mode == DecorationMode::Server;
        }
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

impl Dispatch<XdgActivationV1, ()> for ClientApp {
    /// `xdg_activation_v1` has no events.
    fn event(_: &mut Self, _: &XdgActivationV1, _: <XdgActivationV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationTokenV1, Arc<std::sync::Mutex<Option<String>>>> for ClientApp {
    /// Stores the token string of `done` in the cell of the request.
    fn event(
        _: &mut Self,
        _: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        cell: &Arc<std::sync::Mutex<Option<String>>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            *cell.lock().expect("token lock") = Some(token);
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

impl Dispatch<ZwlrScreencopyManagerV1, ()> for ClientApp {
    /// The manager has no events.
    fn event(_: &mut Self, _: &ZwlrScreencopyManagerV1, _: <ZwlrScreencopyManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for ClientApp {
    /// Records every event of a capture frame.
    fn event(
        app: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        let Some(e) = app.captures.get_mut(&frame.id()) else { return };
        match event {
            Event::Buffer { format, width, height, stride } => e.buffer = Some((u32::from(format), width, height, stride)),
            Event::BufferDone => e.buffer_done = true,
            Event::LinuxDmabuf { .. } => e.linux_dmabuf = true,
            Event::Flags { flags } => e.flags = Some(u32::from(flags)),
            Event::Damage { x, y, width, height } => e.damage = Some((x, y, width, height)),
            Event::Ready { .. } => e.ready = true,
            Event::Failed => e.failed = true,
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
            wl_keyboard::Event::Modifiers { mods_depressed, .. } => app.mods_depressed = mods_depressed,
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for ClientApp {
    /// Tracks the surface with the pointer and the surface-local position
    /// (enter / motion / leave) and records every button press with them.
    fn event(app: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { surface, surface_x, surface_y, .. } => app.pointer = Some((surface, surface_x, surface_y)),
            wl_pointer::Event::Leave { surface, .. } if app.pointer.as_ref().map(|p| &p.0) == Some(&surface) => app.pointer = None,
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if let Some(p) = app.pointer.as_mut() {
                    p.1 = surface_x;
                    p.2 = surface_y;
                }
            }
            wl_pointer::Event::Button { serial, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } => {
                app.button_serial = serial;
                if let Some(p) = app.pointer.clone() {
                    app.presses.push(p);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for ClientApp {
    /// The manager has no events.
    fn event(_: &mut Self, _: &wl_data_device_manager::WlDataDeviceManager, _: wl_data_device_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for ClientApp {
    /// Records the clipboard selection and the drag and drop events.
    fn event(app: &mut Self, _: &wl_data_device::WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use wl_data_device::Event;
        match event {
            Event::DataOffer { id } => {
                app.offer_mimes.insert(id.id(), Vec::new());
            }
            Event::Selection { id } => {
                app.clipboard_events += 1;
                app.clipboard_offer = id;
            }
            Event::Enter { serial, id, .. } => {
                app.dnd_serial = serial;
                app.dnd_offer = id;
                app.dnd_events.push("enter");
            }
            Event::Motion { .. } => app.dnd_events.push("motion"),
            Event::Leave => app.dnd_events.push("leave"),
            Event::Drop => app.dnd_events.push("drop"),
            _ => {}
        }
    }

    event_created_child!(ClientApp, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for ClientApp {
    /// Records the mime types of the offer.
    fn event(app: &mut Self, offer: &wl_data_offer::WlDataOffer, event: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            app.offer_mimes.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for ClientApp {
    /// Writes the source data on `send` and records the source events.
    fn event(app: &mut Self, source: &wl_data_source::WlDataSource, event: wl_data_source::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use wl_data_source::Event;
        match event {
            Event::Send { fd, .. } => {
                _write_source(app, &source.id(), fd);
                app.source_events.push("send");
            }
            Event::Cancelled => app.source_events.push("cancelled"),
            Event::DndDropPerformed => app.source_events.push("dnd_drop_performed"),
            Event::DndFinished => app.source_events.push("dnd_finished"),
            _ => {}
        }
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceManagerV1, ()> for ClientApp {
    /// The manager has no events.
    fn event(_: &mut Self, _: &ZwpPrimarySelectionDeviceManagerV1, _: <ZwpPrimarySelectionDeviceManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpPrimarySelectionDeviceV1, ()> for ClientApp {
    /// Records the primary selection.
    fn event(app: &mut Self, _: &ZwpPrimarySelectionDeviceV1, event: zwp_primary_selection_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_primary_selection_device_v1::Event;
        match event {
            Event::DataOffer { offer } => {
                app.offer_mimes.insert(offer.id(), Vec::new());
            }
            Event::Selection { id } => {
                app.primary_events += 1;
                app.primary_offer = id;
            }
            _ => {}
        }
    }

    event_created_child!(ClientApp, ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<ZwpPrimarySelectionOfferV1, ()> for ClientApp {
    /// Records the mime types of the offer.
    fn event(app: &mut Self, offer: &ZwpPrimarySelectionOfferV1, event: zwp_primary_selection_offer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            app.offer_mimes.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<ZwpPrimarySelectionSourceV1, ()> for ClientApp {
    /// Writes the source data on `send` and records the source events.
    fn event(app: &mut Self, source: &ZwpPrimarySelectionSourceV1, event: zwp_primary_selection_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_primary_selection_source_v1::Event;
        match event {
            Event::Send { fd, .. } => {
                _write_source(app, &source.id(), fd);
                app.source_events.push("send");
            }
            Event::Cancelled => app.source_events.push("cancelled"),
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
