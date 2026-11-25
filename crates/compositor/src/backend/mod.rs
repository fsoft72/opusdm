//! Backends of the compositor (nested winit window, headless test backend)
//! and the event loop pieces they share.

mod headless;
mod nested;

use std::os::unix::net::UnixStream;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use smithay::{
    input::keyboard::XkbConfig,
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction},
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};

use crate::args::{Args, BackendKind};
use crate::child::spawn_startup;
use crate::state::{ClientState, State, LOG_TAG};

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

/// Runs the selected backend until it quits.
pub fn run(args: &Args) -> Result<(), String> {
    match args.backend {
        BackendKind::Headless => headless::run(args),
        BackendKind::Nested => nested::run(args),
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
            if let Err(e) = data.display.flush_clients() {
                opusdm_ipc::err_log!(LOG_TAG, "flush clients: {e}");
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| format!("display source: {e}"))?;
    Ok(name)
}

/// Starts the `--startup` command, if any, on the socket `socket_name`; a
/// failed start is logged and the compositor goes on without it.
fn start_startup(state: &mut State, args: &Args, socket_name: &str) {
    let Some(command) = args.startup.as_deref() else { return };
    match spawn_startup(command, socket_name) {
        Ok(child) => state.startup = Some(child),
        Err(e) => opusdm_ipc::err_log!(LOG_TAG, "startup command '{command}' failed to start: {e}"),
    }
}
