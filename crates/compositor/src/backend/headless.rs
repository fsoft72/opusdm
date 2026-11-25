//! Headless backend: no window and no rendering, used by the integration tests.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::Duration;

use smithay::{
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopSignal,
        },
        wayland_server::Display,
    },
    utils::Transform,
};

use super::{add_input_devices, init_wayland, register_quit_signals, start_startup, LoopData};
use crate::args::Args;
use crate::child::reap;
use crate::state::{State, LOG_TAG};

/// Name of the headless output.
const OUTPUT_NAME: &str = "HEADLESS-1";

/// Refresh rate of the headless output, in mHz.
const REFRESH_MHZ: i32 = 60_000;

/// Period of the frame / housekeeping timer.
const TICK: Duration = Duration::from_millis(16);

/// Runs the headless compositor until SIGTERM or SIGINT.
pub fn run(args: &Args) -> Result<(), String> {
    let mut event_loop: EventLoop<'static, LoopData> =
        EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    let mut display: Display<State> = Display::new().map_err(|e| format!("wayland display: {e}"))?;
    let mut state = State::new(&display.handle(), _headless_output(args.size));
    register_quit_signals(&state.quit)?;
    add_input_devices(&mut state)?;
    let socket_name = init_wayland(&event_loop, &mut display)?;
    let signal = event_loop.get_signal();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(TICK), move |_, _, data| _tick(data, &signal))
        .map_err(|e| format!("frame timer: {e}"))?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "WAYLAND_DISPLAY={socket_name}")
        .and_then(|_| stdout.flush())
        .map_err(|e| format!("print the socket name: {e}"))?;
    start_startup(&mut state, args, &socket_name);
    let mut data = LoopData { state, display };
    event_loop.run(None, &mut data, |_| {}).map_err(|e| format!("event loop: {e}"))?;
    Ok(())
}

/// The `HEADLESS-1` output: `size` at 60 Hz, scale 1, at the origin.
fn _headless_output(size: (i32, i32)) -> Output {
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "OpusDM".into(),
            model: "Headless".into(),
        },
    );
    let mode = Mode { size: size.into(), refresh: REFRESH_MHZ };
    output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Integer(1)), Some((0, 0).into()));
    output.set_preferred(mode);
    output
}

/// One timer tick: stops the loop on a quit signal, else sends the frame
/// callbacks, reaps the startup command, drops dead windows and flushes.
fn _tick(data: &mut LoopData, signal: &LoopSignal) -> TimeoutAction {
    if data.state.quit.load(Ordering::Relaxed) {
        opusdm_ipc::debug_log!(LOG_TAG, "quit on signal");
        signal.stop();
        return TimeoutAction::Drop;
    }
    data.state.send_frames();
    reap(&mut data.state.startup);
    data.state.refresh();
    if let Err(e) = data.display.flush_clients() {
        opusdm_ipc::err_log!(LOG_TAG, "flush clients: {e}");
    }
    TimeoutAction::ToDuration(TICK)
}
