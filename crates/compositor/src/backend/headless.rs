//! Headless backend: no window and no rendering, used by the integration tests.

use std::time::Duration;

use smithay::reexports::calloop::{
    timer::{TimeoutAction, Timer},
    LoopSignal,
};

use super::{announce, housekeeping, new_output, setup_common, stop_on_quit, LoopData};
use crate::args::Args;

/// Name of the headless output.
const OUTPUT_NAME: &str = "HEADLESS-1";

/// Period of the frame / housekeeping timer.
const TICK: Duration = Duration::from_millis(16);

/// Runs the headless compositor until SIGTERM or SIGINT.
pub fn run(args: &Args) -> Result<(), String> {
    let common = setup_common(new_output(OUTPUT_NAME, "Headless", args.size))?;
    let mut event_loop = common.event_loop;
    let mut state = common.state;
    let signal = event_loop.get_signal();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(TICK), move |_, _, data| _tick(data, &signal))
        .map_err(|e| format!("frame timer: {e}"))?;
    // Only the test backend takes synthetic input from the IPC (`debug/key`).
    state.test_input = true;
    announce(&mut state, args, &common.socket_name)?;
    let mut data = LoopData { state, display: common.display };
    event_loop.run(None, &mut data, |_| {}).map_err(|e| format!("event loop: {e}"))?;
    Ok(())
}

/// One timer tick: stops the loop on a quit signal, else sends the frame
/// callbacks, reaps the startup command, drops dead windows and flushes.
fn _tick(data: &mut LoopData, signal: &LoopSignal) -> TimeoutAction {
    if stop_on_quit(data, signal) { return TimeoutAction::Drop; }
    data.state.send_frames();
    housekeeping(data);
    TimeoutAction::ToDuration(TICK)
}
