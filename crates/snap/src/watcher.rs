//! The thread that watches for window drags: polls the pointer while a button
//! is down, feeds the state machine, drives the overlay and places windows.

use crate::machine::{Action, Machine, Sample, WindowId};
use crate::overlay::Overlay;
use crate::x11::{Display, Environment};
use crate::zones::zone_rect;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Time between samples while a button is down or the overlay animates.
const POLL: Duration = Duration::from_millis(16);

/// Time between samples when nothing happens: only the button is watched.
const IDLE_POLL: Duration = Duration::from_millis(50);

/// Time after a placement at which the real rectangle of the window is read.
const SETTLE_DELAY: Duration = Duration::from_millis(250);

/// Seconds to wait before reconnecting, per consecutive failure; the last
/// value repeats.
const BACKOFF_SECS: [u64; 5] = [1, 2, 4, 8, 30];

/// A run that lasted this long counts as healthy and resets the backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

/// Handle on the running watcher thread.
pub struct Watcher {
    enabled: Arc<AtomicBool>,
}

impl Watcher {
    /// Start the watcher thread, snapping on when `enabled`. `None` (after one
    /// log line) when the thread cannot be spawned. A missing X connection is
    /// retried with backoff inside the thread.
    pub fn start(enabled: bool) -> Option<Watcher> {
        let flag = Arc::new(AtomicBool::new(enabled));
        let thread_flag = flag.clone();
        let spawned = std::thread::Builder::new().name("opusdm-snap".into()).spawn(move || run_forever(&thread_flag));
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("opusdm-snap", "cannot spawn the snap thread: {e}");
            return None;
        }
        Some(Watcher { enabled: flag })
    }

    /// Turn snapping on or off live.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }
}

/// Connect and watch, reconnecting with backoff when the connection fails.
fn run_forever(enabled: &AtomicBool) {
    let mut failures = 0usize;
    loop {
        let started = Instant::now();
        match Display::connect() {
            Ok(display) => {
                if let Err(e) = watch(&display, enabled) {
                    opusdm_ipc::err_log!("opusdm-snap", "snap watcher stopped: {e}");
                }
            }
            Err(e) => opusdm_ipc::err_log!("opusdm-snap", "snap unavailable (no X11 connection): {e}"),
        }
        if started.elapsed() >= HEALTHY_RUN { failures = 0; }
        let wait = BACKOFF_SECS[failures.min(BACKOFF_SECS.len() - 1)];
        failures += 1;
        std::thread::sleep(Duration::from_secs(wait));
    }
}

/// The watch loop on one connection. Returns only on a connection error.
fn watch(display: &Display, enabled: &AtomicBool) -> Result<(), Box<dyn Error>> {
    let mut machine = Machine::new();
    let mut overlay = Overlay::new(display)?;
    if overlay.is_none() {
        opusdm_ipc::err_log!("opusdm-snap", "no compositing manager or ARGB visual: snapping works without the zone preview");
    }
    let mut env = display.environment()?;
    let mut was_down = false;
    let mut settle: Option<(WindowId, Instant)> = None;
    let started = Instant::now();

    loop {
        let now_ms = started.elapsed().as_millis() as u64;
        if !enabled.load(Ordering::Relaxed) {
            machine = Machine::new();
            settle = None;
            if let Some(overlay) = overlay.as_mut() {
                overlay.hide(now_ms);
                overlay.tick(display, now_ms)?;
            }
            std::thread::sleep(IDLE_POLL);
            continue;
        }

        let (pointer, down) = display.pointer()?;
        if down && !was_down {
            env = display.environment()?;
        }
        was_down = down;
        let busy = down || !machine.is_idle();
        if busy {
            let window = if down { display.active_frame()? } else { None };
            let sample = Sample { button: down, pointer, window, area: env.area(pointer) };
            for action in machine.step(&sample) {
                apply(action, display, overlay.as_mut(), &env, pointer, now_ms, &mut settle);
            }
        }

        if let Some((window, _)) = settle.filter(|(_, due)| Instant::now() >= *due) {
            match display.frame_rect(window)? {
                Some(actual) => machine.settle(window, actual),
                None => machine.forget(window),
            }
            settle = None;
        }

        let animating = match overlay.as_mut() {
            Some(overlay) => overlay.tick(display, now_ms)?,
            None => false,
        };
        std::thread::sleep(if busy || animating || settle.is_some() { POLL } else { IDLE_POLL });
    }
}

/// Carry out one action of the machine. Failures are logged and ignored: a
/// window that vanished must not stop the watcher.
fn apply(
    action: Action,
    display: &Display,
    overlay: Option<&mut Overlay>,
    env: &Environment,
    pointer: (i32, i32),
    now_ms: u64,
    settle: &mut Option<(WindowId, Instant)>,
) {
    match action {
        Action::Zone(Some(zone)) => {
            if let Some(overlay) = overlay {
                overlay.show(zone_rect(zone, env.area(pointer)), now_ms);
            }
        }
        Action::Zone(None) => {
            if let Some(overlay) = overlay {
                overlay.hide(now_ms);
            }
        }
        Action::Place { window, zone, rect } => {
            match display.place(window, zone, rect) {
                Ok(()) => *settle = Some((window, Instant::now() + SETTLE_DELAY)),
                Err(e) => opusdm_ipc::err_log!("opusdm-snap", "cannot snap window {window:#x} to {zone:?}: {e}"),
            }
        }
        Action::Restore { window, rect } => {
            if let Err(e) = display.restore(window, rect) {
                opusdm_ipc::err_log!("opusdm-snap", "cannot restore window {window:#x} to {rect:?}: {e}");
            }
        }
    }
}
