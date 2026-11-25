//! Headless backend: no window and no rendering, used by the integration tests.
//! Screen captures (wlr-screencopy) get a fixed pixel, `HEADLESS_CAPTURE_PIXEL`.

use std::time::Duration;

use smithay::reexports::calloop::{
    timer::{TimeoutAction, Timer},
    LoopSignal,
};

use super::{announce, housekeeping, new_output, setup_common, stop_on_quit, LoopData};
use crate::args::Args;
use crate::geometry::Rect;
use crate::state::State;

/// Name of the headless output.
const OUTPUT_NAME: &str = "HEADLESS-1";

/// Period of the frame / housekeeping timer.
const TICK: Duration = Duration::from_millis(16);

/// Every pixel of a screen capture of the headless output (B, G, R, X:
/// `Xrgb8888` in memory): there is no renderer to read back.
pub const HEADLESS_CAPTURE_PIXEL: [u8; 4] = [0x20, 0x20, 0x20, 0xff];

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

/// One timer tick: stops the loop on a quit signal, else answers the
/// pending screen captures, sends the frame callbacks, reaps the startup
/// command, drops dead windows and flushes.
fn _tick(data: &mut LoopData, signal: &LoopSignal) -> TimeoutAction {
    if stop_on_quit(data, signal) { return TimeoutAction::Drop; }
    _service_captures(&mut data.state);
    data.state.send_frames();
    housekeeping(data);
    TimeoutAction::ToDuration(TICK)
}

/// Answers every pending screen capture of the output with
/// `HEADLESS_CAPTURE_PIXEL` (a capture of another output fails).
fn _service_captures(state: &mut State) {
    for capture in state.take_pending_captures() {
        let pixels = if capture.output == state.output {
            _capture_pixels(capture.region)
        } else {
            Err(format!("output {} is not the headless output", capture.output.name()))
        };
        state.complete_capture(capture, pixels);
    }
}

/// The pixels of a capture of `region`: `HEADLESS_CAPTURE_PIXEL` repeated.
fn _capture_pixels(region: Rect) -> Result<Vec<u8>, String> {
    let w = usize::try_from(region.w).map_err(|_| format!("negative capture width {}", region.w))?;
    let h = usize::try_from(region.h).map_err(|_| format!("negative capture height {}", region.h))?;
    Ok(HEADLESS_CAPTURE_PIXEL.repeat(w * h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_is_the_fixed_pixel_repeated() {
        let pixels = _capture_pixels(Rect { x: 5, y: 5, w: 3, h: 2 }).unwrap();
        assert_eq!(pixels.len(), 3 * 2 * 4);
        assert!(pixels.as_chunks::<4>().0.iter().all(|p| *p == HEADLESS_CAPTURE_PIXEL));
        assert!(_capture_pixels(Rect { x: 0, y: 0, w: -1, h: 2 }).is_err());
    }
}
