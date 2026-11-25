//! Backends of the compositor (nested winit window, headless test backend)
//! and the event loop pieces they share.

mod headless;
mod nested;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use smithay::{
    input::keyboard::XkbConfig,
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction},
        wayland_server::Display,
    },
    utils::Transform,
    wayland::socket::ListeningSocketSource,
};

use crate::args::{Args, BackendKind};
use crate::child::{reap, spawn_startup};
use crate::config::CompositorConfig;
use crate::decor::geometry::TitleLayout;
use crate::hotkeys::HotkeyTable;
use crate::ipc::{socket_path, IpcServer};
use crate::state::{ClientState, State, LOG_TAG};
use crate::wm::settings::WmSettings;

/// Refresh rate given to the output of every backend, in mHz.
pub const REFRESH_MHZ: i32 = 60_000;

/// Delay before a held key repeats, in milliseconds.
pub const KEY_REPEAT_DELAY: i32 = 600;

/// Key repeats per second.
pub const KEY_REPEAT_RATE: i32 = 25;

/// What the event loop owns for the whole run: the state and the display.
pub struct LoopData {
    /// The compositor state.
    pub state: State,
    /// The Wayland display: dispatches client requests and flushes events.
    pub display: Display<State>,
}

/// What `setup_common` builds for a backend: the event loop, the display,
/// the state and the name of the listening socket.
pub struct Common {
    /// The event loop; the backend adds its own sources and runs it.
    pub event_loop: EventLoop<'static, LoopData>,
    /// The Wayland display, moved into `LoopData` before the loop runs.
    pub display: Display<State>,
    /// The compositor state, moved into `LoopData` before the loop runs.
    pub state: State,
    /// Name of the `wayland-N` socket the clients connect to.
    pub socket_name: String,
}

/// Runs the selected backend until it quits.
pub fn run(args: &Args) -> Result<(), String> {
    match args.backend {
        BackendKind::Headless => headless::run(args),
        BackendKind::Nested => nested::run(args),
    }
}

/// Setup shared by every backend: event loop, display, the state with
/// `output` and the desktop count of `DesktopSettings.virtual_screens`,
/// the window manager settings of `desktop.json`, the built-in bindings of
/// `compositor.json`, the SIGTERM / SIGINT flag, keyboard and pointer, the Wayland socket
/// with its client sources and the private IPC socket next to it.
fn setup_common(output: Output) -> Result<Common, String> {
    let event_loop: EventLoop<'static, LoopData> = EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    let mut display: Display<State> = Display::new().map_err(|e| format!("wayland display: {e}"))?;
    let settings = opusdm_ipc::settings::load_desktop_settings(LOG_TAG);
    let mut state = State::new(&display.handle(), output, settings.virtual_screens);
    opusdm_ipc::debug_log!(LOG_TAG, "{} virtual desktops", state.desktops.count());
    state.wm = WmSettings::from_desktop(&settings);
    state.config = CompositorConfig::load();
    state.title_layout = TitleLayout::parse(&state.config.title_layout);
    state.hotkeys = HotkeyTable::new(&state.config);
    register_quit_signals(&state.quit)?;
    add_input_devices(&mut state)?;
    let socket_name = init_wayland(&event_loop, &mut display)?;
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("ipc: XDG_RUNTIME_DIR is not set")?;
    state.ipc = IpcServer::bind(event_loop.handle(), &socket_path(std::path::Path::new(&runtime_dir), &socket_name))?;
    Ok(Common { event_loop, display, state, socket_name })
}

/// A single output named `name`: `size` at `REFRESH_MHZ`, scale 1, normal
/// transform, at the origin.
fn new_output(name: &str, model: &str, size: (i32, i32)) -> Output {
    let output = Output::new(
        name.to_string(),
        PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "OpusDM".into(), model: model.into() },
    );
    let mode = OutputMode { size: size.into(), refresh: REFRESH_MHZ };
    output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Integer(1)), Some((0, 0).into()));
    output.set_preferred(mode);
    output
}

/// Prints `WAYLAND_DISPLAY=<socket_name>` and then
/// `OPUSDM_COMPOSITOR_SOCKET=<path>` on stdout (the tests and the scripts
/// read them) and starts the `--startup` command. Called once every global
/// of the backend exists, so the first client sees them all.
fn announce(state: &mut State, args: &Args, socket_name: &str) -> Result<(), String> {
    let ipc = state.ipc.path().ok_or("ipc: the socket is not bound")?.to_string_lossy().into_owned();
    let mut stdout = std::io::stdout();
    writeln!(stdout, "WAYLAND_DISPLAY={socket_name}")
        .and_then(|_| writeln!(stdout, "OPUSDM_COMPOSITOR_SOCKET={ipc}"))
        .and_then(|_| stdout.flush())
        .map_err(|e| format!("print the socket names: {e}"))?;
    start_startup(state, args, socket_name, &ipc);
    Ok(())
}

/// True when SIGTERM / SIGINT (or the nested window close) asked to quit:
/// the loop is stopped and the caller drops its timer.
fn stop_on_quit(data: &LoopData, signal: &LoopSignal) -> bool {
    if !data.state.quit.load(Ordering::Relaxed) { return false; }
    opusdm_ipc::debug_log!(LOG_TAG, "quit requested, stopping the loop");
    signal.stop();
    true
}

/// Work shared by the frame timers after the frame callbacks: reaps the
/// startup command, drops dead windows, tells the IPC watchers what
/// changed (input focus, dead windows) and flushes the clients.
fn housekeeping(data: &mut LoopData) {
    reap(&mut data.state.startup);
    data.state.refresh();
    data.state.notify_watchers();
    if let Err(e) = data.display.flush_clients() {
        opusdm_ipc::err_log!(LOG_TAG, "flush clients: {e}");
    }
}

/// Adds the keyboard and the pointer to the seat.
fn add_input_devices(state: &mut State) -> Result<(), String> {
    state
        .seat
        .add_keyboard(XkbConfig::default(), KEY_REPEAT_DELAY, KEY_REPEAT_RATE)
        .map_err(|e| format!("keyboard: {e}"))?;
    state.seat.add_pointer();
    Ok(())
}

/// Makes SIGTERM and SIGINT set `quit`.
fn register_quit_signals(quit: &Arc<AtomicBool>) -> Result<(), String> {
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(quit)).map_err(|e| format!("signal {sig}: {e}"))?;
    }
    Ok(())
}

/// Binds a `wayland-N` socket in `XDG_RUNTIME_DIR`, accepts its clients and
/// dispatches their requests in `event_loop`. Returns the socket name.
fn init_wayland(event_loop: &EventLoop<'static, LoopData>, display: &mut Display<State>) -> Result<String, String> {
    let socket = ListeningSocketSource::new_auto().map_err(|e| format!("wayland socket: {e}"))?;
    let name = socket.socket_name().to_string_lossy().into_owned();
    let handle = event_loop.handle();
    handle
        .insert_source(socket, |stream: UnixStream, _, data: &mut LoopData| {
            if let Err(e) = data.state.display_handle.insert_client(stream, Arc::new(ClientState::default())) {
                opusdm_ipc::err_log!(LOG_TAG, "new client refused: {e}");
            }
        })
        .map_err(|e| format!("socket source: {e}"))?;
    let fd = display.backend().poll_fd().try_clone_to_owned().map_err(|e| format!("display fd: {e}"))?;
    handle
        .insert_source(Generic::new(fd, Interest::READ, Mode::Level), |_, _, data: &mut LoopData| {
            if let Err(e) = data.display.dispatch_clients(&mut data.state) {
                opusdm_ipc::err_log!(LOG_TAG, "dispatch clients: {e}");
            }
            data.state.notify_watchers();
            if let Err(e) = data.display.flush_clients() {
                opusdm_ipc::err_log!(LOG_TAG, "flush clients: {e}");
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| format!("display source: {e}"))?;
    Ok(name)
}

/// Starts the `--startup` command, if any, on the socket `socket_name`
/// with the IPC socket `ipc_socket`; a failed start is logged and the
/// compositor goes on without it.
fn start_startup(state: &mut State, args: &Args, socket_name: &str, ipc_socket: &str) {
    let Some(command) = args.startup.as_deref() else { return };
    match spawn_startup(command, socket_name, ipc_socket) {
        Ok(child) => state.startup = Some(child),
        Err(e) => opusdm_ipc::err_log!(LOG_TAG, "startup command '{command}' failed to start: {e}"),
    }
}
