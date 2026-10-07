# Audio tray Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A panel icon with a popover to control volume and mute of the output and the input, and to choose the default device of each, through the PulseAudio protocol.

**Architecture:** A new GTK-free crate `opusdm-audio` (model, volume maths, monitor filter, diff, `Backend` trait, `FakeBackend`, and a `PulseBackend` where ONE thread owns the libpulse threaded mainloop and context). The panel gets a pure row model (`audio_model.rs`), a controller (`audio_center.rs`) and a widget (`widgets/audio.rs`), the same split as the device notifier.

**Tech Stack:** Rust 2021 (rust-version 1.91), `libpulse-binding` 2.30 (libpulse 17 and headers are installed), GTK4 (`gtk4` 0.10), `async-channel`.

**Spec:** `docs/superpowers/specs/2026-10-07-audio-tray-design.md`. Read it together with the device notifier it copies: `docs/superpowers/specs/2026-10-06-device-notifier-design.md`, `crates/devices/src/{backend,watch,error}.rs`, `crates/panel/src/{devices_model,devices_center}.rs`, `crates/panel/src/widgets/devices.rs`.

## Global Constraints

- Package manager is `pnpm`, but this is a Rust workspace: use `cargo`. No new tools.
- NEVER write the em dash character in code, comments, docs or commit messages: use `-`.
- Code and comments in English. Every function and method has a doc comment; public ones use `///` rustdoc.
- Constants are `UPPER_SNAKE_CASE`. Private functions in Rust do not need a `_` prefix (the `_` rule is for Python and C).
- Exit conditions at the top of a function, one liners when possible (`let Some(x) = y else { return };`).
- Edition is 2021: **no let chains** (`if let ... && ...`).
- Text files end with an empty line (a final newline).
- Commit after every task, only the files that task changed. Do NOT push. No AI attribution lines in commit messages. Never use `--no-verify`; if the pre-commit hook (dprint) fails or reformats, fix the cause and commit again.
- Volume: 100% is `PA_VOLUME_NORM` = 65536 (0 dB). The panel never sets more than 100%.
- Sound server unreachable: the widget is hidden, one `err_log!` line, retry every 30 seconds.
- Error line in the popover lasts 5 seconds.
- The GTK main loop never calls libpulse and never blocks on it.
- `CHANGES.md` gets one line per task that changes behavior; `docs/usage.md` gets section 3.32.

## Review Focus

Failure modes the spec implies but no explicit requirement pins. Each has a test in the task named in brackets.

1. A device whose volume is above 100% (set by another tool) shows its real percentage and the slider sits at the top, nothing panics and nothing is lowered silently [Task 1 `from_raw`, Task 4 `tooltip`].
2. A source whose name ends in `.monitor` must never appear as a microphone, also when it is the server's default source [Task 1 filter, Task 3 `source_device`].
3. Unplugging the default output makes the server pick another default: the model must show exactly one default per kind afterwards, and a `Changed` for the old and the new device must not leave two [Task 1 diff, Task 4 model].
4. The sound server restarts while the panel runs: the icon hides, then comes back with a fresh device list and a new backend without a panel restart [Task 3 `run` and `reported` reset, Task 4 `Unavailable` then `Snapshot`].
5. A slider drag produces dozens of values per second: only the last pending volume per device reaches the server, and server events arriving mid-drag do not yank the slider back [Task 4 `coalesce` and `DragGuard`].

---

### Task 1: Crate `opusdm-audio`, pure parts

**Files:**
- Create: `crates/audio/Cargo.toml`
- Create: `crates/audio/src/lib.rs`
- Create: `crates/audio/src/model.rs`
- Create: `crates/audio/src/volume.rs`
- Create: `crates/audio/src/filter.rs`
- Create: `crates/audio/src/error.rs`
- Create: `crates/audio/src/diff.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces:
  - `model::Kind { Output, Input }` (derives `Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord`)
  - `model::AudioDevice { index: u32, name: String, description: String, kind: Kind, volume_percent: u32, muted: bool, is_default: bool }` with `fn key(&self) -> (Kind, u32)`
  - `volume::{NORM: u32, MAX_PERCENT: u32, to_raw(u32) -> u32, from_raw(u32) -> u32}`
  - `filter::is_input(source_name: &str) -> bool`
  - `error::AudioError { ConnectionFailed(String), NoSuchDevice, Rejected(String) }` with `fn text(&self) -> String`, `Display`, `std::error::Error`
  - `diff::{AudioEvent { Added(AudioDevice), Removed { kind: Kind, index: u32 }, Changed(AudioDevice) }, diff(&[AudioDevice], &[AudioDevice]) -> Vec<AudioEvent>}`

- [ ] **Step 1: Create the crate skeleton and register it**

`crates/audio/Cargo.toml`:

```toml
[package]
name = "opusdm-audio"
version = "0.5.0"
edition.workspace = true
rust-version.workspace = true

[lib]
name = "opusdm_audio"
path = "src/lib.rs"
```

In the root `Cargo.toml`, add `"crates/audio"` to `members` (after `"crates/devices"`).

`crates/audio/src/lib.rs`:

```rust
//! opusdm-audio: volume, mute and default device of the sound server, with no GTK.
//!
//! A pure model, volume maths, monitor filter and device diff, a `Backend`
//! trait (real `PulseBackend` over libpulse, `FakeBackend` for tests) and a
//! watcher that turns server events into `AudioEvent`s.

pub mod diff;
pub mod error;
pub mod filter;
pub mod model;
pub mod volume;

pub use diff::{diff, AudioEvent};
pub use error::AudioError;
pub use model::{AudioDevice, Kind};
```

- [ ] **Step 2: Write the failing tests (model, volume, filter, error, diff)**

`crates/audio/src/model.rs`:

```rust
//! The data the panel needs about one sound device.

/// Which side of the sound card a device is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A sink: speakers, headphones, HDMI.
    Output,
    /// A source that is not a monitor: a microphone, a line in.
    Input,
}

/// One output or input device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    /// Index in the sound server; only unique within a `Kind`.
    pub index: u32,
    /// Stable server name, used to make the device the default.
    pub name: String,
    /// Human readable name.
    pub description: String,
    pub kind: Kind,
    /// Average over the channels. Can be above 100 when another tool raised it.
    pub volume_percent: u32,
    pub muted: bool,
    pub is_default: bool,
}

impl AudioDevice {
    /// Identity of the device: the index alone repeats between outputs and inputs.
    pub fn key(&self) -> (Kind, u32) {
        (self.kind, self.index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_tells_an_output_from_an_input_with_the_same_index() {
        let mk = |kind| AudioDevice {
            index: 3,
            name: "n".into(),
            description: "d".into(),
            kind,
            volume_percent: 50,
            muted: false,
            is_default: false,
        };
        assert_ne!(mk(Kind::Output).key(), mk(Kind::Input).key());
    }
}
```

`crates/audio/src/volume.rs`:

```rust
//! Conversion between a percentage and a PulseAudio volume value.

/// PulseAudio volume of 100% (0 dB).
pub const NORM: u32 = 65_536;
/// The highest percentage the panel sets.
pub const MAX_PERCENT: u32 = 100;

/// PulseAudio value for `percent`, clamped to `MAX_PERCENT` and rounded.
pub fn to_raw(percent: u32) -> u32 {
    let p = percent.min(MAX_PERCENT) as u64;
    ((p * NORM as u64 + 50) / 100) as u32
}

/// Percentage for a PulseAudio value, rounded. Not clamped: a volume raised
/// above 100% by another tool shows its real value.
pub fn from_raw(raw: u32) -> u32 {
    ((raw as u64 * 100 + NORM as u64 / 2) / NORM as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_points() {
        assert_eq!(to_raw(0), 0);
        assert_eq!(to_raw(50), 32_768);
        assert_eq!(to_raw(100), NORM);
    }

    #[test]
    fn to_raw_clamps_at_100() {
        assert_eq!(to_raw(250), NORM);
    }

    #[test]
    fn from_raw_rounds_to_the_nearest_percent() {
        assert_eq!(from_raw(51_773), 79);
        assert_eq!(from_raw(13_763), 21);
        assert_eq!(from_raw(64_225), 98);
        assert_eq!(from_raw(NORM), 100);
    }

    #[test]
    fn from_raw_keeps_an_amplified_volume() {
        assert_eq!(from_raw(2 * NORM), 200);
    }

    #[test]
    fn every_percentage_survives_a_round_trip() {
        for p in 0..=MAX_PERCENT {
            assert_eq!(from_raw(to_raw(p)), p, "percent {p}");
        }
    }
}
```

`crates/audio/src/filter.rs`:

```rust
//! Which sources are real inputs.

/// Suffix PulseAudio gives to the "listen to an output" source of every sink.
const MONITOR_SUFFIX: &str = ".monitor";

/// Whether a source called `source_name` is an input (a microphone, a line in).
/// Monitors are the listening side of an output and are never listed.
pub fn is_input(source_name: &str) -> bool {
    !source_name.ends_with(MONITOR_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_are_dropped() {
        assert!(!is_input("alsa_output.pci-0000_01_00.1.hdmi-stereo.monitor"));
    }

    #[test]
    fn real_inputs_are_kept() {
        assert!(is_input("alsa_input.usb-Blue_Microphones_Yeti_Stereo_Microphone_REV8-00.analog-stereo"));
        assert!(is_input("bluez_input.00_11_22_33_44_55.0"));
    }
}
```

`crates/audio/src/error.rs`:

```rust
//! Errors of the sound server calls, with a readable one line text.

/// An error of the sound server or of the connection to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// No server, a lost connection, or a call that got no answer in time.
    ConnectionFailed(String),
    /// The device is gone (unplugged in the meantime).
    NoSuchDevice,
    /// The server answered that it did not do it.
    Rejected(String),
}

impl AudioError {
    /// One line for the user.
    pub fn text(&self) -> String {
        match self {
            AudioError::ConnectionFailed(why) => format!("Cannot reach the sound server: {why}"),
            AudioError::NoSuchDevice => "The device is gone".to_string(),
            AudioError::Rejected(why) => format!("The sound server refused the change: {why}"),
        }
    }
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_variant_has_a_one_line_text() {
        assert_eq!(AudioError::NoSuchDevice.text(), "The device is gone");
        assert_eq!(
            AudioError::ConnectionFailed("timeout".into()).text(),
            "Cannot reach the sound server: timeout"
        );
        assert_eq!(
            AudioError::Rejected("no entity".into()).text(),
            "The sound server refused the change: no entity"
        );
        for e in [AudioError::NoSuchDevice, AudioError::ConnectionFailed("x".into()), AudioError::Rejected("x".into())] {
            assert!(!e.text().contains('\n'));
        }
    }
}
```

`crates/audio/src/diff.rs`:

```rust
//! The events that turn one device list into the next.

use crate::model::{AudioDevice, Kind};

/// A change of the device list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    Added(AudioDevice),
    Removed { kind: Kind, index: u32 },
    /// Volume, mute or default flag changed.
    Changed(AudioDevice),
}

/// The events that take `old` to `now`: removals first, then additions and changes.
pub fn diff(old: &[AudioDevice], now: &[AudioDevice]) -> Vec<AudioEvent> {
    let mut out = Vec::new();
    for d in old {
        if !now.iter().any(|n| n.key() == d.key()) {
            out.push(AudioEvent::Removed { kind: d.kind, index: d.index });
        }
    }
    for n in now {
        match old.iter().find(|d| d.key() == n.key()) {
            None => out.push(AudioEvent::Added(n.clone())),
            Some(o) if o != n => out.push(AudioEvent::Changed(n.clone())),
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: Kind, index: u32, percent: u32, is_default: bool) -> AudioDevice {
        AudioDevice {
            index,
            name: format!("dev{index}"),
            description: format!("Device {index}"),
            kind,
            volume_percent: percent,
            muted: false,
            is_default,
        }
    }

    #[test]
    fn nothing_changed_nothing_reported() {
        let list = vec![dev(Kind::Output, 1, 50, true)];
        assert!(diff(&list, &list).is_empty());
    }

    #[test]
    fn a_new_and_a_gone_device() {
        let old = vec![dev(Kind::Output, 1, 50, true)];
        let now = vec![dev(Kind::Output, 2, 50, true)];
        let ev = diff(&old, &now);
        assert_eq!(ev, vec![AudioEvent::Removed { kind: Kind::Output, index: 1 }, AudioEvent::Added(now[0].clone())]);
    }

    #[test]
    fn a_volume_change_is_one_changed_event() {
        let old = vec![dev(Kind::Input, 4, 50, true)];
        let now = vec![dev(Kind::Input, 4, 60, true)];
        assert_eq!(diff(&old, &now), vec![AudioEvent::Changed(now[0].clone())]);
    }

    #[test]
    fn switching_the_default_changes_both_devices() {
        let old = vec![dev(Kind::Output, 1, 50, true), dev(Kind::Output, 2, 50, false)];
        let now = vec![dev(Kind::Output, 1, 50, false), dev(Kind::Output, 2, 50, true)];
        assert_eq!(diff(&old, &now).len(), 2);
    }

    #[test]
    fn an_output_and_an_input_with_the_same_index_are_different_devices() {
        let old = vec![dev(Kind::Output, 3, 50, true)];
        let now = vec![dev(Kind::Output, 3, 50, true), dev(Kind::Input, 3, 50, true)];
        assert_eq!(diff(&old, &now), vec![AudioEvent::Added(now[1].clone())]);
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p opusdm-audio`
Expected: all tests PASS (the code and its tests are written together because each file is a few lines of pure code; if any FAIL, fix the code, not the test).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audio
git commit -m "feat(audio): model, volume maths, monitor filter, error and diff"
```

---

### Task 2: `Backend` trait and `FakeBackend`

**Files:**
- Create: `crates/audio/src/backend.rs`
- Modify: `crates/audio/src/lib.rs`

**Interfaces:**
- Consumes: `AudioDevice`, `Kind`, `AudioError`, `volume::MAX_PERCENT` (Task 1).
- Produces:
  - `trait Backend: Send + Sync { fn list(&self) -> Result<Vec<AudioDevice>, AudioError>; fn set_volume(&self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError>; fn set_mute(&self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError>; fn set_default(&self, kind: Kind, name: &str) -> Result<(), AudioError>; }`
  - `FakeBackend { pub devices: Mutex<Vec<AudioDevice>>, pub calls: Mutex<Vec<String>> }` (`Default`), `fn calls(&self) -> Vec<String>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/audio/src/backend.rs` with the trait and the fake left out, and only the tests module at the bottom first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: Kind, index: u32, is_default: bool) -> AudioDevice {
        AudioDevice {
            index,
            name: format!("dev{index}"),
            description: String::new(),
            kind,
            volume_percent: 40,
            muted: false,
            is_default,
        }
    }

    fn fake() -> FakeBackend {
        let f = FakeBackend::default();
        *f.devices.lock().unwrap() = vec![
            dev(Kind::Output, 1, true),
            dev(Kind::Output, 2, false),
            dev(Kind::Input, 1, true),
        ];
        f
    }

    #[test]
    fn set_volume_is_recorded_and_clamped() {
        let f = fake();
        f.set_volume(Kind::Output, 2, 250).unwrap();
        assert_eq!(f.calls(), vec!["volume Output 2 250"]);
        assert_eq!(f.list().unwrap()[1].volume_percent, 100);
    }

    #[test]
    fn an_unknown_device_is_no_such_device() {
        let f = fake();
        assert_eq!(f.set_volume(Kind::Output, 9, 10), Err(AudioError::NoSuchDevice));
        assert_eq!(f.set_mute(Kind::Input, 9, true), Err(AudioError::NoSuchDevice));
        assert_eq!(f.set_default(Kind::Output, "nope"), Err(AudioError::NoSuchDevice));
    }

    #[test]
    fn mute_only_touches_the_chosen_device() {
        let f = fake();
        f.set_mute(Kind::Input, 1, true).unwrap();
        let list = f.list().unwrap();
        assert!(list[2].muted);
        assert!(!list[0].muted);
    }

    #[test]
    fn the_default_moves_within_its_kind_only() {
        let f = fake();
        f.set_default(Kind::Output, "dev2").unwrap();
        let list = f.list().unwrap();
        assert!(!list[0].is_default);
        assert!(list[1].is_default);
        assert!(list[2].is_default, "the input default must not change");
    }
}
```

Add `pub mod backend;` and `pub use backend::{Backend, FakeBackend};` to `lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-audio backend`
Expected: FAIL to compile ("cannot find type `FakeBackend`").

- [ ] **Step 3: Write the implementation (above the tests module)**

```rust
//! The operations the panel needs, behind a trait so tests need no server.

use crate::error::AudioError;
use crate::model::{AudioDevice, Kind};
use crate::volume::MAX_PERCENT;
use std::sync::Mutex;

/// Operations on the sound devices.
pub trait Backend: Send + Sync {
    /// Every output and every input (monitors left out), outputs first.
    fn list(&self) -> Result<Vec<AudioDevice>, AudioError>;
    /// Set the volume of a device to `percent` (at most 100) on every channel.
    fn set_volume(&self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError>;
    /// Mute or unmute a device.
    fn set_mute(&self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError>;
    /// Make the device called `name` the default of its `kind`.
    fn set_default(&self, kind: Kind, name: &str) -> Result<(), AudioError>;
}

/// A backend for tests: records every call and applies it to its own list.
#[derive(Default)]
pub struct FakeBackend {
    pub devices: Mutex<Vec<AudioDevice>>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeBackend {
    /// The recorded calls, oldest first.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Record `call` in the log.
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

impl Backend for FakeBackend {
    fn list(&self) -> Result<Vec<AudioDevice>, AudioError> {
        Ok(self.devices.lock().unwrap().clone())
    }

    fn set_volume(&self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError> {
        self.record(format!("volume {kind:?} {index} {percent}"));
        let mut devices = self.devices.lock().unwrap();
        let d = devices.iter_mut().find(|d| d.key() == (kind, index)).ok_or(AudioError::NoSuchDevice)?;
        d.volume_percent = percent.min(MAX_PERCENT);
        Ok(())
    }

    fn set_mute(&self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError> {
        self.record(format!("mute {kind:?} {index} {muted}"));
        let mut devices = self.devices.lock().unwrap();
        let d = devices.iter_mut().find(|d| d.key() == (kind, index)).ok_or(AudioError::NoSuchDevice)?;
        d.muted = muted;
        Ok(())
    }

    fn set_default(&self, kind: Kind, name: &str) -> Result<(), AudioError> {
        self.record(format!("default {kind:?} {name}"));
        let mut devices = self.devices.lock().unwrap();
        if !devices.iter().any(|d| d.kind == kind && d.name == name) {
            return Err(AudioError::NoSuchDevice);
        }
        for d in devices.iter_mut().filter(|d| d.kind == kind) {
            d.is_default = d.name == name;
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p opusdm-audio`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/audio
git commit -m "feat(audio): backend trait and fake backend"
```

---

### Task 3: PulseAudio backend and watcher

**Files:**
- Modify: `crates/audio/Cargo.toml`
- Create: `crates/audio/src/pulse.rs`
- Create: `crates/audio/src/watch.rs`
- Create: `crates/audio/examples/list.rs`
- Create: `crates/audio/tests/live.rs`
- Modify: `crates/audio/src/lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 1 and 2.
- Produces:
  - `pulse::PulseBackend` implementing `Backend` (a handle: it sends jobs to the thread that owns libpulse)
  - `pulse::Session { connect() -> Result<Session, AudioError>, backend(&self) -> Arc<PulseBackend>, list(&mut self) -> Result<Vec<AudioDevice>, AudioError>, follow(&mut self, known: Vec<AudioDevice>, on_event: impl FnMut(AudioEvent) -> bool) -> Result<(), AudioError> }` (`follow` returns `Ok(())` when `on_event` says stop, `Err` when the connection is lost)
  - `watch::WatchMsg { Backend(Arc<dyn Backend>), Snapshot(Vec<AudioDevice>), Event(AudioEvent), Unavailable(String) }` and `watch::spawn() -> async_channel::Receiver<WatchMsg>`
  - `UdisksBackend`-like re-exports: `pub use pulse::PulseBackend; pub use watch::WatchMsg;`

The libpulse calls below were compiled and run against the installed server in a throwaway spike (5 sinks listed, server info read, subscribe accepted), so the API shapes are real. Design rule: the `Mainloop` and `Context` are not `Send`, so ONE thread creates and uses them; callbacks only send into `std::sync::mpsc` channels; every libpulse call is wrapped in `ml.lock()` / `ml.unlock()`.

- [ ] **Step 1: Add the dependency**

In `crates/audio/Cargo.toml` add:

```toml
[dependencies]
libpulse-binding = "2.30"
async-channel.workspace = true
```

- [ ] **Step 2: Write the pure test for the watcher helper and the live test**

`crates/audio/tests/live.rs` (needs a running sound server, so it is ignored by default):

```rust
//! Talks to the real sound server. Run with `cargo test -p opusdm-audio -- --ignored`.

use opusdm_audio::pulse::Session;
use opusdm_audio::Kind;

#[test]
#[ignore = "needs a running PulseAudio or pipewire-pulse"]
fn lists_outputs_and_no_monitors() {
    let mut session = Session::connect().expect("connect");
    let list = session.list().expect("list");
    assert!(list.iter().any(|d| d.kind == Kind::Output), "no output device: {list:?}");
    assert!(list.iter().all(|d| !d.name.ends_with(".monitor")), "a monitor leaked: {list:?}");
    for kind in [Kind::Output, Kind::Input] {
        let defaults = list.iter().filter(|d| d.kind == kind && d.is_default).count();
        assert!(defaults <= 1, "{kind:?} has {defaults} defaults");
    }
}
```

In `watch.rs`, the `drain_burst`-style helper is not needed (coalescing lives in `Session::follow`), so there is no pure unit test there; the live test and the example are the checks for this task.

- [ ] **Step 3: Write `pulse.rs`**

```rust
//! The PulseAudio client. One thread owns the threaded mainloop and the
//! context (neither is `Send`); every other thread talks to it through jobs.

use crate::backend::Backend;
use crate::diff::{diff, AudioEvent};
use crate::error::AudioError;
use crate::filter;
use crate::model::{AudioDevice, Kind};
use crate::volume;
use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::introspect::{SinkInfo, SourceInfo};
use libpulse_binding::context::subscribe::InterestMaskSet;
use libpulse_binding::context::{Context, FlagSet, State};
use libpulse_binding::mainloop::threaded::Mainloop;
use libpulse_binding::volume::{ChannelVolumes, Volume};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Name the client shows to the sound server.
const APP_NAME: &str = "opusdm-panel";
/// How many times to look at the connection state while connecting.
const CONNECT_STEPS: u32 = 100;
/// Pause between two looks, in milliseconds (100 x 50 ms = 5 s).
const CONNECT_STEP_MS: u64 = 50;
/// How long one server call may take.
const CALL_TIMEOUT_SECS: u64 = 5;
/// Quiet time after a burst of server events before the list is read again.
const COALESCE_MS: u64 = 100;
/// Channel count used when a device was not listed yet.
const DEFAULT_CHANNELS: u8 = 2;

/// Work for the thread that owns libpulse.
type Job = Box<dyn FnOnce(&mut Pulse) + Send>;

/// What the owner thread receives.
enum Msg {
    /// A request from `PulseBackend`.
    Job(Job),
    /// The server says a sink, a source or the defaults changed.
    Tick,
    /// The connection state changed; check whether it is still alive.
    StateChanged,
}

/// One item of a list answer, sent from the libpulse callback.
enum Item {
    Device(AudioDevice, u8),
    End,
    Failed,
}

/// The error for a call that got no answer.
fn timeout_error() -> AudioError {
    AudioError::ConnectionFailed("the sound server did not answer".to_string())
}

/// The error for a lost connection.
fn gone_error() -> AudioError {
    AudioError::ConnectionFailed("the sound server went away".to_string())
}

/// The libpulse objects, only ever touched by the owner thread.
struct Pulse {
    // Field order matters: the context must drop before the mainloop.
    ctx: Context,
    ml: Mainloop,
    /// Channel count of every device seen by the last `list`.
    channels: HashMap<(Kind, u32), u8>,
}

impl Drop for Pulse {
    /// Disconnect and stop the mainloop thread before the fields drop.
    fn drop(&mut self) {
        self.ml.lock();
        self.ctx.disconnect();
        self.ml.unlock();
        self.ml.stop();
    }
}

impl Pulse {
    /// Create the mainloop and the context and wait until the server is ready.
    /// `events` receives a `StateChanged` whenever the connection state moves.
    fn connect(events: &Sender<Msg>) -> Result<Self, AudioError> {
        let fail = |why: &str| AudioError::ConnectionFailed(why.to_string());
        let mut ml = Mainloop::new().ok_or_else(|| fail("cannot create the mainloop"))?;
        let mut ctx = Context::new(&ml, APP_NAME).ok_or_else(|| fail("cannot create the context"))?;
        let state_tx = events.clone();
        ctx.set_state_callback(Some(Box::new(move || {
            let _ = state_tx.send(Msg::StateChanged);
        })));
        ml.lock();
        let started = ctx
            .connect(None, FlagSet::NOFLAGS, None)
            .map_err(|e| fail(&e.to_string()))
            .and_then(|_| ml.start().map_err(|e| fail(&e.to_string())));
        ml.unlock();
        started?;
        let mut pulse = Self { ctx, ml, channels: HashMap::new() };
        pulse.wait_ready()?;
        Ok(pulse)
    }

    /// The connection state, read under the mainloop lock.
    fn state(&mut self) -> State {
        self.ml.lock();
        let state = self.ctx.get_state();
        self.ml.unlock();
        state
    }

    /// Poll the connection state until it is ready, failed or out of time.
    fn wait_ready(&mut self) -> Result<(), AudioError> {
        for _ in 0..CONNECT_STEPS {
            match self.state() {
                State::Ready => return Ok(()),
                State::Failed | State::Terminated => return Err(gone_error()),
                _ => std::thread::sleep(Duration::from_millis(CONNECT_STEP_MS)),
            }
        }
        Err(timeout_error())
    }

    /// Fail when the connection is no longer usable.
    fn check_alive(&mut self) -> Result<(), AudioError> {
        match self.state() {
            State::Failed | State::Terminated => Err(gone_error()),
            _ => Ok(()),
        }
    }

    /// Ask for events about sinks, sources and the defaults; each one is a `Tick`.
    fn subscribe(&mut self, events: Sender<Msg>) {
        self.ml.lock();
        self.ctx.set_subscribe_callback(Some(Box::new(move |_, _, _| {
            let _ = events.send(Msg::Tick);
        })));
        self.ctx.subscribe(InterestMaskSet::SINK | InterestMaskSet::SOURCE | InterestMaskSet::SERVER, |_| {});
        self.ml.unlock();
    }

    /// The names of the default sink and the default source.
    fn defaults(&mut self) -> Result<(Option<String>, Option<String>), AudioError> {
        let (tx, rx) = mpsc::channel();
        self.ml.lock();
        self.ctx.introspect().get_server_info(move |s| {
            let _ = tx.send((
                s.default_sink_name.as_ref().map(|n| n.to_string()),
                s.default_source_name.as_ref().map(|n| n.to_string()),
            ));
        });
        self.ml.unlock();
        rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS)).map_err(|_| timeout_error())
    }

    /// Every sink, with its channel count.
    fn list_sinks(&mut self, default: Option<String>) -> Result<Vec<(AudioDevice, u8)>, AudioError> {
        let (tx, rx) = mpsc::channel();
        self.ml.lock();
        self.ctx.introspect().get_sink_info_list(move |r| {
            let item = match r {
                ListResult::Item(i) => Item::Device(sink_device(i, default.as_deref()), i.volume.len()),
                ListResult::End => Item::End,
                ListResult::Error => Item::Failed,
            };
            let _ = tx.send(item);
        });
        self.ml.unlock();
        collect(&rx)
    }

    /// Every source that is not a monitor, with its channel count.
    fn list_sources(&mut self, default: Option<String>) -> Result<Vec<(AudioDevice, u8)>, AudioError> {
        let (tx, rx) = mpsc::channel();
        self.ml.lock();
        self.ctx.introspect().get_source_info_list(move |r| {
            let item = match r {
                ListResult::Item(i) => match source_device(i, default.as_deref()) {
                    Some(d) => Item::Device(d, i.volume.len()),
                    None => return,
                },
                ListResult::End => Item::End,
                ListResult::Error => Item::Failed,
            };
            let _ = tx.send(item);
        });
        self.ml.unlock();
        collect(&rx)
    }

    /// Outputs first, then inputs. Remembers each device's channel count.
    fn list(&mut self) -> Result<Vec<AudioDevice>, AudioError> {
        let (sink_default, source_default) = self.defaults()?;
        let mut all = self.list_sinks(sink_default)?;
        all.extend(self.list_sources(source_default)?);
        self.channels = all.iter().map(|(d, c)| (d.key(), *c)).collect();
        Ok(all.into_iter().map(|(d, _)| d).collect())
    }

    /// Set one volume on every channel of a device.
    fn set_volume(&mut self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError> {
        let channels = self.channels.get(&(kind, index)).copied().unwrap_or(DEFAULT_CHANNELS);
        let mut cv = ChannelVolumes::default();
        cv.set(channels, Volume(volume::to_raw(percent)));
        let (tx, rx) = mpsc::channel();
        let done = Box::new(move |ok: bool| {
            let _ = tx.send(ok);
        });
        self.ml.lock();
        let mut intro = self.ctx.introspect();
        let _ = match kind {
            Kind::Output => intro.set_sink_volume_by_index(index, &cv, Some(done)),
            Kind::Input => intro.set_source_volume_by_index(index, &cv, Some(done)),
        };
        self.ml.unlock();
        wait_ack(&rx)
    }

    /// Mute or unmute a device.
    fn set_mute(&mut self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError> {
        let (tx, rx) = mpsc::channel();
        let done = Box::new(move |ok: bool| {
            let _ = tx.send(ok);
        });
        self.ml.lock();
        let mut intro = self.ctx.introspect();
        let _ = match kind {
            Kind::Output => intro.set_sink_mute_by_index(index, muted, Some(done)),
            Kind::Input => intro.set_source_mute_by_index(index, muted, Some(done)),
        };
        self.ml.unlock();
        wait_ack(&rx)
    }

    /// Make the device called `name` the default of its kind.
    fn set_default(&mut self, kind: Kind, name: &str) -> Result<(), AudioError> {
        let (tx, rx) = mpsc::channel();
        let done = move |ok: bool| {
            let _ = tx.send(ok);
        };
        self.ml.lock();
        let _ = match kind {
            Kind::Output => self.ctx.set_default_sink(name, done),
            Kind::Input => self.ctx.set_default_source(name, done),
        };
        self.ml.unlock();
        wait_ack(&rx)
    }
}

/// A sink as the panel model.
fn sink_device(i: &SinkInfo, default: Option<&str>) -> AudioDevice {
    let name = i.name.as_deref().unwrap_or("").to_string();
    let is_default = default == Some(name.as_str());
    AudioDevice {
        index: i.index,
        description: i.description.as_deref().unwrap_or("").to_string(),
        name,
        kind: Kind::Output,
        volume_percent: volume::from_raw(i.volume.avg().0),
        muted: i.mute,
        is_default,
    }
}

/// A source as the panel model; `None` for a monitor.
fn source_device(i: &SourceInfo, default: Option<&str>) -> Option<AudioDevice> {
    let name = i.name.as_deref().unwrap_or("").to_string();
    if !filter::is_input(&name) {
        return None;
    }
    let is_default = default == Some(name.as_str());
    Some(AudioDevice {
        index: i.index,
        description: i.description.as_deref().unwrap_or("").to_string(),
        name,
        kind: Kind::Input,
        volume_percent: volume::from_raw(i.volume.avg().0),
        muted: i.mute,
        is_default,
    })
}

/// Read list items until the end marker.
fn collect(rx: &Receiver<Item>) -> Result<Vec<(AudioDevice, u8)>, AudioError> {
    let mut out = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS)) {
            Ok(Item::Device(d, channels)) => out.push((d, channels)),
            Ok(Item::End) => return Ok(out),
            Ok(Item::Failed) => return Err(AudioError::Rejected("the device list failed".to_string())),
            Err(_) => return Err(timeout_error()),
        }
    }
}

/// Wait for the success flag of a set call.
fn wait_ack(rx: &Receiver<bool>) -> Result<(), AudioError> {
    match rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS)) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AudioError::Rejected("the server said no".to_string())),
        Err(_) => Err(timeout_error()),
    }
}

/// The handle other threads use: every call becomes a job for the owner thread.
pub struct PulseBackend {
    tx: Mutex<Sender<Msg>>,
}

impl PulseBackend {
    /// Run `f` on the owner thread and wait for its answer.
    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Pulse) -> Result<T, AudioError> + Send + 'static,
    ) -> Result<T, AudioError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let job: Job = Box::new(move |p| {
            let _ = reply_tx.send(f(p));
        });
        self.tx.lock().unwrap().send(Msg::Job(job)).map_err(|_| gone_error())?;
        // Longer than one server call: `list` makes three of them.
        reply_rx.recv_timeout(Duration::from_secs(CALL_TIMEOUT_SECS * 3)).map_err(|_| timeout_error())?
    }
}

impl Backend for PulseBackend {
    fn list(&self) -> Result<Vec<AudioDevice>, AudioError> {
        self.call(|p| p.list())
    }

    fn set_volume(&self, kind: Kind, index: u32, percent: u32) -> Result<(), AudioError> {
        self.call(move |p| p.set_volume(kind, index, percent))
    }

    fn set_mute(&self, kind: Kind, index: u32, muted: bool) -> Result<(), AudioError> {
        self.call(move |p| p.set_mute(kind, index, muted))
    }

    fn set_default(&self, kind: Kind, name: &str) -> Result<(), AudioError> {
        let name = name.to_string();
        self.call(move |p| p.set_default(kind, &name))
    }
}

/// A connection to the sound server, owned by the thread that created it.
pub struct Session {
    pulse: Pulse,
    rx: Receiver<Msg>,
    backend: Arc<PulseBackend>,
}

impl Session {
    /// Connect and subscribe to server events.
    pub fn connect() -> Result<Self, AudioError> {
        let (tx, rx) = mpsc::channel();
        let mut pulse = Pulse::connect(&tx)?;
        pulse.subscribe(tx.clone());
        Ok(Self { pulse, rx, backend: Arc::new(PulseBackend { tx: Mutex::new(tx) }) })
    }

    /// The handle for other threads; its jobs run while `follow` is running.
    pub fn backend(&self) -> Arc<PulseBackend> {
        self.backend.clone()
    }

    /// Read the devices now.
    pub fn list(&mut self) -> Result<Vec<AudioDevice>, AudioError> {
        self.pulse.list()
    }

    /// Serve jobs and report changes against `known` until `on_event` returns
    /// false (`Ok`) or the connection is lost (`Err`).
    pub fn follow(
        &mut self,
        mut known: Vec<AudioDevice>,
        mut on_event: impl FnMut(AudioEvent) -> bool,
    ) -> Result<(), AudioError> {
        loop {
            let msg = self.rx.recv().map_err(|_| gone_error())?;
            let mut changed = self.dispatch(msg)?;
            // A burst of events (one volume drag) is one re-read.
            while changed {
                match self.rx.recv_timeout(Duration::from_millis(COALESCE_MS)) {
                    Ok(next) => changed |= self.dispatch(next)?,
                    Err(_) => break,
                }
            }
            if !changed {
                continue;
            }
            let now = self.pulse.list()?;
            for event in diff(&known, &now) {
                if !on_event(event) {
                    return Ok(());
                }
            }
            known = now;
        }
    }

    /// Handle one message; true when it was a server event.
    fn dispatch(&mut self, msg: Msg) -> Result<bool, AudioError> {
        match msg {
            Msg::Job(job) => {
                job(&mut self.pulse);
                Ok(false)
            }
            Msg::StateChanged => self.pulse.check_alive().map(|_| false),
            Msg::Tick => Ok(true),
        }
    }
}
```

Known trap: `while changed { ... Err(_) => break }` then `if !changed { continue; }` is correct only because `changed` stays true when the quiet period ends; do not "simplify" it.

If a libpulse signature differs from the above on the installed version, the spike that proved these calls is at `/tmp/claude-1000/-home-fabio-dev-projects-opusdm/9bf714e9-0b24-4946-8ed9-3e20ed0fb9ee/scratchpad/pulsespike/src/main.rs` (it may be gone; otherwise read `docs.rs/libpulse-binding/2.30.1`).

- [ ] **Step 4: Write `watch.rs`**

```rust
//! A worker that follows the sound server and reports the device list and its changes.

use crate::backend::Backend;
use crate::diff::AudioEvent;
use crate::model::AudioDevice;
use crate::pulse::Session;
use std::sync::Arc;
use std::time::Duration;

/// Wait before trying again when the sound server is missing.
const RETRY_SECS: u64 = 30;

/// What the worker sends.
pub enum WatchMsg {
    /// The backend to use for operations; sent before every snapshot.
    Backend(Arc<dyn Backend>),
    /// The full list, at start and after the server comes back.
    Snapshot(Vec<AudioDevice>),
    Event(AudioEvent),
    /// The server is not available (sent once per outage).
    Unavailable(String),
}

/// Start the worker. Messages arrive on the returned channel until it is dropped.
pub fn spawn() -> async_channel::Receiver<WatchMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || run(tx));
    rx
}

/// Connect (retrying), then follow the server until the receiver is dropped.
fn run(tx: async_channel::Sender<WatchMsg>) {
    let mut reported = false;
    loop {
        match connect_and_follow(&tx, &mut reported) {
            Ok(()) => return,
            Err(msg) => {
                if !reported && tx.send_blocking(WatchMsg::Unavailable(msg)).is_err() {
                    return;
                }
                reported = true;
            }
        }
        std::thread::sleep(Duration::from_secs(RETRY_SECS));
    }
}

/// One session: `Ok` when the receiver is gone, `Err` when the server failed.
/// `reported` is cleared once a session is up, so the next outage is reported again.
fn connect_and_follow(tx: &async_channel::Sender<WatchMsg>, reported: &mut bool) -> Result<(), String> {
    let mut session = Session::connect().map_err(|e| e.text())?;
    let known = session.list().map_err(|e| e.text())?;
    let send = |m: WatchMsg| tx.send_blocking(m).is_ok();
    if !send(WatchMsg::Backend(session.backend())) || !send(WatchMsg::Snapshot(known.clone())) {
        return Ok(());
    }
    *reported = false;
    session.follow(known, |event| send(WatchMsg::Event(event))).map_err(|e| e.text())
}
```

- [ ] **Step 5: Wire the modules, add the example**

`lib.rs`: add `pub mod pulse;` and `pub mod watch;` and `pub use pulse::PulseBackend;` and `pub use watch::WatchMsg;`.

`crates/audio/examples/list.rs`:

```rust
//! Prints the devices the audio tray would list, one per line.

use opusdm_audio::pulse::Session;

fn main() {
    let mut session = Session::connect().expect("sound server");
    for d in session.list().expect("device list") {
        println!(
            "{:?} #{} | {} | {}% muted={} default={} | {}",
            d.kind, d.index, d.description, d.volume_percent, d.muted, d.is_default, d.name
        );
    }
}
```

- [ ] **Step 6: Build and check against the real server**

Run: `cargo build -p opusdm-audio --examples && cargo run -q -p opusdm-audio --example list`
Expected: one line per output and per input, exactly one `default=true` per kind, no name ending in `.monitor`.

Run: `cargo test -p opusdm-audio -- --include-ignored`
Expected: PASS, including `lists_outputs_and_no_monitors`.

Fix compile errors against the libpulse docs if any signature differs; do not change the design (one owner thread, callbacks only send into channels).

- [ ] **Step 7: Commit**

```bash
git add crates/audio Cargo.lock
git commit -m "feat(audio): libpulse backend on an owner thread and change watcher"
```

---

### Task 4: Panel row model (pure)

**Files:**
- Create: `crates/panel/src/audio_model.rs`
- Modify: `crates/panel/src/lib.rs` (`mod audio_model;`)
- Modify: `crates/panel/Cargo.toml`

**Interfaces:**
- Consumes: `opusdm_audio::{AudioDevice, AudioEvent, Kind, WatchMsg}`.
- Produces:
  - `IconLevel { Muted, Low, Medium, High }` (`Clone, Copy, Debug, PartialEq, Eq`)
  - `Model` (`Default`): `apply(&mut self, WatchMsg)`, `is_available(&self) -> bool`, `devices_of(&self, Kind) -> Vec<AudioDevice>`, `default_of(&self, Kind) -> Option<&AudioDevice>`, `icon_level(&self) -> IconLevel`, `tooltip(&self) -> String`
  - `DragGuard` (`Default`): `touch(&mut self, now: Instant)`, `is_held(&self, now: Instant) -> bool`
  - `Request { SetVolume { kind: Kind, index: u32, percent: u32 }, SetMute { kind: Kind, index: u32, muted: bool }, SetDefault { kind: Kind, name: String } }` (`Clone, Debug, PartialEq, Eq`)
  - `coalesce(Vec<Request>) -> Vec<Request>`

- [ ] **Step 1: Add the dependency and the module**

`crates/panel/Cargo.toml`, under `[dependencies]`: `opusdm-audio = { path = "../audio" }`.
`crates/panel/src/lib.rs`, next to `mod devices_model;`: `mod audio_model;`.

- [ ] **Step 2: Write the failing tests**

Create `crates/panel/src/audio_model.rs` with only this module at the bottom first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn dev(kind: Kind, index: u32, percent: u32, muted: bool, is_default: bool) -> AudioDevice {
        AudioDevice {
            index,
            name: format!("dev{index}"),
            description: format!("Device {index}"),
            kind,
            volume_percent: percent,
            muted,
            is_default,
        }
    }

    fn snapshot(list: Vec<AudioDevice>) -> WatchMsg {
        WatchMsg::Snapshot(list)
    }

    #[test]
    fn nothing_is_available_before_the_first_snapshot() {
        let m = Model::default();
        assert!(!m.is_available());
        assert_eq!(m.tooltip(), "No sound server");
    }

    #[test]
    fn a_snapshot_makes_it_available_and_unavailable_clears_it() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 50, false, true)]));
        assert!(m.is_available());
        m.apply(WatchMsg::Unavailable("gone".into()));
        assert!(!m.is_available());
        assert!(m.devices_of(Kind::Output).is_empty());
    }

    #[test]
    fn a_new_snapshot_after_an_outage_replaces_the_list() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 50, false, true)]));
        m.apply(WatchMsg::Unavailable("gone".into()));
        m.apply(snapshot(vec![dev(Kind::Output, 7, 20, false, true)]));
        assert!(m.is_available());
        assert_eq!(m.devices_of(Kind::Output)[0].index, 7);
    }

    #[test]
    fn events_add_change_and_remove() {
        let mut m = Model::default();
        m.apply(snapshot(vec![]));
        m.apply(WatchMsg::Event(AudioEvent::Added(dev(Kind::Output, 2, 30, false, true))));
        m.apply(WatchMsg::Event(AudioEvent::Added(dev(Kind::Output, 2, 30, false, true))));
        assert_eq!(m.devices_of(Kind::Output).len(), 1, "an Added twice is one device");
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Output, 2, 80, false, true))));
        assert_eq!(m.devices_of(Kind::Output)[0].volume_percent, 80);
        m.apply(WatchMsg::Event(AudioEvent::Removed { kind: Kind::Output, index: 2 }));
        assert!(m.devices_of(Kind::Output).is_empty());
    }

    #[test]
    fn a_changed_for_an_unknown_device_is_ignored() {
        let mut m = Model::default();
        m.apply(snapshot(vec![]));
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Input, 5, 80, false, true))));
        assert!(m.devices_of(Kind::Input).is_empty());
    }

    #[test]
    fn removing_the_default_then_the_server_picks_another() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 50, false, true), dev(Kind::Output, 2, 50, false, false)]));
        m.apply(WatchMsg::Event(AudioEvent::Removed { kind: Kind::Output, index: 1 }));
        assert!(m.default_of(Kind::Output).is_none());
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Output, 2, 50, false, true))));
        assert_eq!(m.default_of(Kind::Output).unwrap().index, 2);
        assert_eq!(m.devices_of(Kind::Output).iter().filter(|d| d.is_default).count(), 1);
    }

    #[test]
    fn devices_are_listed_by_kind_in_index_order() {
        let mut m = Model::default();
        m.apply(snapshot(vec![
            dev(Kind::Input, 1, 50, false, true),
            dev(Kind::Output, 9, 50, false, false),
            dev(Kind::Output, 3, 50, false, true),
        ]));
        let outs: Vec<u32> = m.devices_of(Kind::Output).iter().map(|d| d.index).collect();
        assert_eq!(outs, vec![3, 9]);
        assert_eq!(m.devices_of(Kind::Input).len(), 1);
    }

    #[test]
    fn the_icon_follows_the_default_output() {
        let level = |p: u32, muted: bool| {
            let mut m = Model::default();
            m.apply(snapshot(vec![dev(Kind::Output, 1, p, muted, true)]));
            m.icon_level()
        };
        assert_eq!(level(0, false), IconLevel::Muted);
        assert_eq!(level(80, true), IconLevel::Muted);
        assert_eq!(level(20, false), IconLevel::Low);
        assert_eq!(level(50, false), IconLevel::Medium);
        assert_eq!(level(90, false), IconLevel::High);
        assert_eq!(Model::default().icon_level(), IconLevel::Muted);
    }

    #[test]
    fn the_tooltip_shows_the_real_percentage_even_above_100() {
        let mut m = Model::default();
        m.apply(snapshot(vec![dev(Kind::Output, 1, 130, false, true)]));
        assert_eq!(m.tooltip(), "Volume 130%");
        m.apply(WatchMsg::Event(AudioEvent::Changed(dev(Kind::Output, 1, 130, true, true))));
        assert_eq!(m.tooltip(), "Muted");
    }

    #[test]
    fn the_drag_guard_holds_for_a_short_time_after_a_touch() {
        let mut g = DragGuard::default();
        let t0 = Instant::now();
        assert!(!g.is_held(t0));
        g.touch(t0);
        assert!(g.is_held(t0 + Duration::from_millis(HOLD_MS - 1)));
        assert!(!g.is_held(t0 + Duration::from_millis(HOLD_MS + 1)));
    }

    #[test]
    fn coalesce_keeps_only_the_last_volume_per_device() {
        let v = |index, percent| Request::SetVolume { kind: Kind::Output, index, percent };
        let out = coalesce(vec![v(1, 10), v(2, 5), v(1, 20), v(1, 30)]);
        assert_eq!(out, vec![v(2, 5), v(1, 30)]);
    }

    #[test]
    fn coalesce_never_drops_a_mute_or_a_default_change() {
        let reqs = vec![
            Request::SetVolume { kind: Kind::Input, index: 1, percent: 10 },
            Request::SetMute { kind: Kind::Input, index: 1, muted: true },
            Request::SetDefault { kind: Kind::Output, name: "a".into() },
            Request::SetVolume { kind: Kind::Input, index: 1, percent: 12 },
        ];
        let out = coalesce(reqs);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], Request::SetMute { .. }));
        assert!(matches!(out[2], Request::SetVolume { percent: 12, .. }));
    }

    #[test]
    fn an_output_and_an_input_volume_with_the_same_index_are_not_merged() {
        let out = coalesce(vec![
            Request::SetVolume { kind: Kind::Output, index: 1, percent: 10 },
            Request::SetVolume { kind: Kind::Input, index: 1, percent: 20 },
        ]);
        assert_eq!(out.len(), 2);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p opusdm-panel audio_model`
Expected: FAIL to compile (`Model`, `Request`, ... not found).

- [ ] **Step 4: Write the implementation (above the tests module)**

```rust
//! The audio devices as the panel sees them: pure state, no GTK.

use opusdm_audio::{AudioDevice, AudioEvent, Kind, WatchMsg};
use std::time::{Duration, Instant};

/// How long after the last slider change a server event may not move the slider.
pub const HOLD_MS: u64 = 400;
/// Volumes below this are drawn with one wave.
const LOW_BELOW: u32 = 34;
/// Volumes below this are drawn with two waves.
const MEDIUM_BELOW: u32 = 67;

/// Which speaker symbol the panel icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconLevel {
    Muted,
    Low,
    Medium,
    High,
}

/// Every device, and whether the sound server is reachable.
#[derive(Default)]
pub struct Model {
    devices: Vec<AudioDevice>,
    available: bool,
}

impl Model {
    /// Apply one message of the watcher.
    pub fn apply(&mut self, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(_) => {}
            WatchMsg::Unavailable(_) => {
                self.devices.clear();
                self.available = false;
            }
            WatchMsg::Snapshot(list) => {
                self.devices = list;
                self.available = true;
            }
            WatchMsg::Event(AudioEvent::Added(d)) => {
                if self.devices.iter().any(|x| x.key() == d.key()) {
                    return;
                }
                self.devices.push(d);
                self.devices.sort_by_key(|d| (d.kind, d.index));
            }
            WatchMsg::Event(AudioEvent::Changed(d)) => {
                if let Some(slot) = self.devices.iter_mut().find(|x| x.key() == d.key()) {
                    *slot = d;
                }
            }
            WatchMsg::Event(AudioEvent::Removed { kind, index }) => {
                self.devices.retain(|d| d.key() != (kind, index));
            }
        }
    }

    /// Whether the sound server is reachable.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// The devices of one kind, by index.
    pub fn devices_of(&self, kind: Kind) -> Vec<AudioDevice> {
        self.devices.iter().filter(|d| d.kind == kind).cloned().collect()
    }

    /// The default device of one kind.
    pub fn default_of(&self, kind: Kind) -> Option<&AudioDevice> {
        self.devices.iter().find(|d| d.kind == kind && d.is_default)
    }

    /// The icon symbol for the default output.
    pub fn icon_level(&self) -> IconLevel {
        let Some(d) = self.default_of(Kind::Output) else { return IconLevel::Muted };
        if d.muted || d.volume_percent == 0 {
            return IconLevel::Muted;
        }
        match d.volume_percent {
            p if p < LOW_BELOW => IconLevel::Low,
            p if p < MEDIUM_BELOW => IconLevel::Medium,
            _ => IconLevel::High,
        }
    }

    /// The icon tooltip: the default output's volume, "Muted", or why there is none.
    pub fn tooltip(&self) -> String {
        if !self.available {
            return "No sound server".to_string();
        }
        match self.default_of(Kind::Output) {
            None => "No output device".to_string(),
            Some(d) if d.muted => "Muted".to_string(),
            Some(d) => format!("Volume {}%", d.volume_percent),
        }
    }
}

/// Remembers when the user last moved a slider.
#[derive(Default)]
pub struct DragGuard {
    last: Option<Instant>,
}

impl DragGuard {
    /// The user moved the slider at `now`.
    pub fn touch(&mut self, now: Instant) {
        self.last = Some(now);
    }

    /// Whether a server event at `now` must leave the slider alone.
    pub fn is_held(&self, now: Instant) -> bool {
        self.last.is_some_and(|t| now.duration_since(t) < Duration::from_millis(HOLD_MS))
    }
}

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    SetVolume { kind: Kind, index: u32, percent: u32 },
    SetMute { kind: Kind, index: u32, muted: bool },
    SetDefault { kind: Kind, name: String },
}

/// Drop every volume request that a later one for the same device replaces.
/// The order of everything else is kept.
pub fn coalesce(requests: Vec<Request>) -> Vec<Request> {
    let mut out = Vec::new();
    for (i, r) in requests.iter().enumerate() {
        if let Request::SetVolume { kind, index, .. } = r {
            let replaced = requests[i + 1..]
                .iter()
                .any(|n| matches!(n, Request::SetVolume { kind: k, index: x, .. } if k == kind && x == index));
            if replaced {
                continue;
            }
        }
        out.push(r.clone());
    }
    out
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p opusdm-panel audio_model`
Expected: PASS (13 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/panel/Cargo.toml crates/panel/src/audio_model.rs crates/panel/src/lib.rs Cargo.lock
git commit -m "feat(panel): pure model for the audio tray"
```

---

### Task 5: Controller, widget and wiring

**Files:**
- Create: `crates/panel/src/audio_center.rs`
- Create: `crates/panel/src/widgets/audio.rs`
- Modify: `crates/panel/src/widgets/mod.rs` (`pub mod audio;`)
- Modify: `crates/panel/src/lib.rs` (`mod audio_center;`, `attach`, `build_bar_window`)
- Modify: `docs/usage.md` (new section 3.32)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: Task 3 `opusdm_audio::{watch, AudioDevice, AudioError, Backend, Kind, WatchMsg}`, Task 4 `audio_model::{Model, IconLevel, DragGuard, Request, coalesce}`.
- Produces: `AudioCenter::new() -> Rc<AudioCenter>`, `AudioCenter::button(self: &Rc<Self>) -> gtk::Widget`; `AudioButton` (widget). Reads for the widget: `is_available`, `devices_of`, `icon_level`, `tooltip`, `message`; write: `send(Request)`.

This task is GTK code with no unit-testable logic left over (the logic is in Task 4), so the check is a clean build, clippy, the existing tests, and the manual run in the final step.

- [ ] **Step 1: Write `audio_center.rs`**

```rust
//! The panel side of the audio tray: one shared device list, the requests
//! sent to the sound server, and the buttons that show it. Owned by `attach`,
//! so a monitor-hotplug rebuild of the bars never loses the state.

use crate::audio_model::{coalesce, IconLevel, Model, Request};
use crate::widgets::audio::AudioButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_audio::{watch, AudioDevice, AudioError, Backend, Kind, WatchMsg};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, Arc};

/// How long an error line stays in the popover.
const MESSAGE_SECS: u32 = 5;

/// Shared state of the audio tray.
pub struct AudioCenter {
    model: RefCell<Model>,
    /// Strong, because nothing else owns an `AudioButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<AudioButton>>>,
    /// Feeds the worker thread; `None` while the server is not reachable.
    requests: RefCell<Option<mpsc::Sender<Request>>>,
    error_tx: async_channel::Sender<AudioError>,
    message: RefCell<Option<String>>,
    message_gen: Cell<u32>,
}

impl AudioCenter {
    /// Start the watcher and the error pump.
    pub fn new() -> Rc<Self> {
        let (error_tx, error_rx) = async_channel::unbounded();
        let center = Rc::new(Self {
            model: RefCell::new(Model::default()),
            buttons: RefCell::new(Vec::new()),
            requests: RefCell::new(None),
            error_tx,
            message: RefCell::new(None),
            message_gen: Cell::new(0),
        });
        let watch_rx = watch::spawn();
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = watch_rx.recv().await {
                    center.on_watch(msg);
                }
            });
        }
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(e) = error_rx.recv().await {
                    center.show_message(e.text());
                }
            });
        }
        center
    }

    /// The tray widget for one bar. It follows the shared list while it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = AudioButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// Whether the sound server is reachable.
    pub fn is_available(&self) -> bool {
        self.model.borrow().is_available()
    }

    /// The devices of one kind.
    pub fn devices_of(&self, kind: Kind) -> Vec<AudioDevice> {
        self.model.borrow().devices_of(kind)
    }

    /// The icon symbol.
    pub fn icon_level(&self) -> IconLevel {
        self.model.borrow().icon_level()
    }

    /// The icon tooltip.
    pub fn tooltip(&self) -> String {
        self.model.borrow().tooltip()
    }

    /// The error line to show, if any.
    pub fn message(&self) -> Option<String> {
        self.message.borrow().clone()
    }

    /// Hand a request to the worker thread; the GTK thread never waits for it.
    pub fn send(self: &Rc<Self>, request: Request) {
        let sent = self.requests.borrow().as_ref().is_some_and(|tx| tx.send(request).is_ok());
        if !sent {
            self.show_message("The sound server is not ready".to_string());
        }
    }

    /// One message of the watcher: the backend, a new list or a change.
    fn on_watch(self: &Rc<Self>, msg: WatchMsg) {
        match msg {
            WatchMsg::Backend(backend) => {
                *self.requests.borrow_mut() = Some(spawn_worker(backend, self.error_tx.clone()));
            }
            WatchMsg::Unavailable(why) => {
                opusdm_ipc::err_log!("opusdm-panel", "audio tray: {why}");
                *self.requests.borrow_mut() = None;
                self.model.borrow_mut().apply(WatchMsg::Unavailable(why));
            }
            other => self.model.borrow_mut().apply(other),
        }
        self.notify();
    }

    /// Show an error line for a few seconds.
    fn show_message(self: &Rc<Self>, text: String) {
        *self.message.borrow_mut() = Some(text);
        let gen = self.message_gen.get() + 1;
        self.message_gen.set(gen);
        self.notify();
        let center = self.clone();
        glib::timeout_add_seconds_local_once(MESSAGE_SECS, move || {
            if center.message_gen.get() != gen {
                return;
            }
            *center.message.borrow_mut() = None;
            center.notify();
        });
    }

    /// Redraw every live button.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<AudioButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}

/// Run requests on their own thread so a slow server never freezes the panel.
/// Requests that queued up meanwhile are merged: a drag sends only its last
/// volume. Errors go back to the GTK thread through `errors`.
fn spawn_worker(backend: Arc<dyn Backend>, errors: async_channel::Sender<AudioError>) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            for request in coalesce(batch) {
                let Err(e) = execute(&*backend, &request) else { continue };
                if errors.send_blocking(e).is_err() {
                    return;
                }
            }
        }
    });
    tx
}

/// Run one request on the backend.
fn execute(backend: &dyn Backend, request: &Request) -> Result<(), AudioError> {
    match request {
        Request::SetVolume { kind, index, percent } => backend.set_volume(*kind, *index, *percent),
        Request::SetMute { kind, index, muted } => backend.set_mute(*kind, *index, *muted),
        Request::SetDefault { kind, name } => backend.set_default(*kind, name),
    }
}
```

- [ ] **Step 2: Write `widgets/audio.rs`**

```rust
//! The audio icon and its popover, in the right side of the panel.

use crate::audio_center::AudioCenter;
use crate::audio_model::{DragGuard, IconLevel, Request};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_audio::{AudioDevice, Kind};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

/// Size of the speaker icon in pixels.
const ICON_SIZE: i32 = 20;
/// Width of the percentage label, in characters.
const PERCENT_CHARS: i32 = 5;

/// The button and its popover. Hidden while the sound server is not reachable.
pub struct AudioButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    level: Rc<Cell<IconLevel>>,
    button: gtk::MenuButton,
    message: gtk::Label,
    outputs: Section,
    inputs: Section,
}

/// The controls of one kind: device list, slider and mute.
struct Section {
    kind: Kind,
    root: gtk::Box,
    list: gtk::Box,
    scale: gtk::Scale,
    mute: gtk::ToggleButton,
    percent: gtk::Label,
    /// Index of the default device the slider and the mute act on.
    target: Rc<Cell<Option<u32>>>,
    guard: Rc<RefCell<DragGuard>>,
    /// Set while `refresh` changes the mute button, so it does not send a request.
    updating: Rc<Cell<bool>>,
}

impl AudioButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<AudioCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);

        let level = Rc::new(Cell::new(IconLevel::Muted));
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_SIZE);
        icon.set_content_height(ICON_SIZE);
        {
            let level = level.clone();
            icon.set_draw_func(move |area, cr, w, h| draw_icon(area, cr, w, h, level.get()));
        }
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_width_request(280);
        let message = gtk::Label::new(None);
        message.add_css_class("error");
        message.set_visible(false);
        message.set_xalign(0.0);
        let outputs = Section::new(center, Kind::Output, "Output");
        let inputs = Section::new(center, Kind::Input, "Input");
        content.append(&message);
        content.append(&outputs.root);
        content.append(&inputs.root);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));

        root.append(&button);
        root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        Rc::new(Self { root, icon, level, button, message, outputs, inputs })
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the shared state.
    pub fn refresh(&self, center: &Rc<AudioCenter>) {
        self.root.set_visible(center.is_available());
        self.level.set(center.icon_level());
        self.icon.queue_draw();
        self.button.set_tooltip_text(Some(&center.tooltip()));
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        let now = Instant::now();
        self.outputs.refresh(center, now);
        self.inputs.refresh(center, now);
    }
}

impl Section {
    /// Build the controls of one kind and connect them to the center.
    fn new(center: &Rc<AudioCenter>, kind: Kind, title: &str) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some(title));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);

        let target: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
        let guard = Rc::new(RefCell::new(DragGuard::default()));
        let updating = Rc::new(Cell::new(false));

        let mute = gtk::ToggleButton::with_label("Mute");
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_hexpand(true);
        scale.set_draw_value(false);
        let percent = gtk::Label::new(Some("0%"));
        percent.set_width_chars(PERCENT_CHARS);
        percent.set_xalign(1.0);

        {
            // `change-value` fires for the user only, never for `set_value`,
            // so a refresh cannot loop back into a request.
            let (center, target, guard) = (center.clone(), target.clone(), guard.clone());
            scale.connect_change_value(move |_, _, value| {
                let Some(index) = target.get() else { return glib::Propagation::Proceed };
                guard.borrow_mut().touch(Instant::now());
                let percent = value.round().clamp(0.0, 100.0) as u32;
                center.send(Request::SetVolume { kind, index, percent });
                glib::Propagation::Proceed
            });
        }
        {
            let (center, target, updating) = (center.clone(), target.clone(), updating.clone());
            mute.connect_toggled(move |b| {
                if updating.get() {
                    return;
                }
                let Some(index) = target.get() else { return };
                center.send(Request::SetMute { kind, index, muted: b.is_active() });
            });
        }

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.append(&mute);
        controls.append(&scale);
        controls.append(&percent);
        root.append(&heading);
        root.append(&list);
        root.append(&controls);
        root.set_visible(false);
        Self { kind, root, list, scale, mute, percent, target, guard, updating }
    }

    /// Rebuild the device list and show the default device's state.
    fn refresh(&self, center: &Rc<AudioCenter>, now: Instant) {
        let devices = center.devices_of(self.kind);
        self.root.set_visible(!devices.is_empty());
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for d in &devices {
            self.list.append(&build_row(center, d));
        }
        let default = devices.iter().find(|d| d.is_default);
        self.target.set(default.map(|d| d.index));
        self.scale.set_sensitive(default.is_some());
        self.mute.set_sensitive(default.is_some());
        let Some(d) = default else { return };
        self.updating.set(true);
        self.mute.set_active(d.muted);
        self.updating.set(false);
        self.percent.set_text(&format!("{}%", d.volume_percent));
        // A server event must not yank the slider away from the user's finger.
        if !self.guard.borrow().is_held(now) {
            self.scale.set_value(d.volume_percent as f64);
        }
    }
}

/// One device row: a check mark on the default, then the name. A click makes it the default.
fn build_row(center: &Rc<AudioCenter>, d: &AudioDevice) -> gtk::Button {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let check = gtk::Label::new(Some(if d.is_default { "\u{2713}" } else { "" }));
    check.set_width_chars(2);
    let name = gtk::Label::new(Some(&d.description));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&check);
    line.append(&name);
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.set_child(Some(&line));
    let (center, kind, device_name) = (center.clone(), d.kind, d.name.clone());
    button.connect_clicked(move |_| center.send(Request::SetDefault { kind, name: device_name.clone() }));
    button
}

/// A speaker with 0 to 3 sound waves, or a cross when muted, in the text colour.
fn draw_icon(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32, level: IconLevel) {
    let c = area.color();
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
    let (w, h) = (w as f64, h as f64);
    cr.move_to(1.0, h * 0.38);
    cr.line_to(w * 0.28, h * 0.38);
    cr.line_to(w * 0.50, h * 0.18);
    cr.line_to(w * 0.50, h * 0.82);
    cr.line_to(w * 0.28, h * 0.62);
    cr.line_to(1.0, h * 0.62);
    cr.close_path();
    let _ = cr.fill();
    cr.set_line_width(1.6);
    let waves = match level {
        IconLevel::Muted => {
            cr.move_to(w * 0.62, h * 0.36);
            cr.line_to(w - 1.5, h * 0.64);
            cr.move_to(w - 1.5, h * 0.36);
            cr.line_to(w * 0.62, h * 0.64);
            let _ = cr.stroke();
            0
        }
        IconLevel::Low => 1,
        IconLevel::Medium => 2,
        IconLevel::High => 3,
    };
    for i in 0..waves {
        let radius = w * (0.18 + 0.14 * i as f64);
        cr.new_sub_path();
        cr.arc(w * 0.50, h * 0.50, radius, -std::f64::consts::FRAC_PI_4, std::f64::consts::FRAC_PI_4);
        let _ = cr.stroke();
    }
}
```

If `connect_change_value`'s closure signature differs in `gtk4` 0.10 (it should return `glib::Propagation`), fix it by reading the compiler error; the behavior to keep is "only user input sends a request".

- [ ] **Step 3: Wire it into the panel**

`crates/panel/src/widgets/mod.rs`: add `pub mod audio;` (alphabetical, before `clock`).

`crates/panel/src/lib.rs`:
- add `mod audio_center;` next to `mod audio_model;`
- `build_bar_window` gets one more parameter `audio: Rc<audio_center::AudioCenter>` after `devices` (if clippy reports `too_many_arguments`, add `#[allow(clippy::too_many_arguments)]` above the function with a one line reason: the bar needs every shared center)
- right after the `if is_primary { right.append(&devices.button()); }` block, add:

```rust
    // The audio tray sits next to the device notifier, on the primary bar only.
    if is_primary {
        right.append(&audio.button());
    }
```
- in `attach`: `let audio = audio_center::AudioCenter::new();` next to `let devices = ...`, a `let audio = audio.clone();` next to `let devices = devices.clone();` in the `rebuild` setup, and pass `audio.clone()` as the new last argument of the `build_bar_window(...)` call.

Update the doc comment line of `build_bar_window` (`... tasklist, sysmon, clock`) to mention the audio tray if it lists widgets.

- [ ] **Step 4: Build, lint, test**

Run: `cargo build -p opusdm-hub` then `cargo clippy -p opusdm-audio -p opusdm-panel --all-targets` then `cargo test -p opusdm-audio -p opusdm-panel`
Expected: build OK, no new clippy warnings (compare with `git stash`-free baseline: warnings that already existed are not yours), tests PASS.

- [ ] **Step 5: Docs and changelog**

`docs/usage.md`: add after section 3.31 (before `## 4. Keyboard Shortcuts Reference`):

```markdown
### 3.32 Audio tray (panel)

Like the KDE Audio Volume applet: volume, mute and default device of the sound output and input.

- **Icon:** a speaker on the right side of the panel, next to the device notifier and before the CPU
  and network monitors (primary bar only). It shows no waves when muted, and one, two or three waves
  as the volume of the default output grows. The tooltip shows the percentage, or *Muted*. It is
  hidden while the sound server is not reachable.
- **Popover (click the icon):** an **Output** and an **Input** section. Each lists its devices (a
  check mark on the default one; click a device to make it the default), with a **Mute** button and a
  volume slider for the default device. A volume raised above 100% by another tool shows its real
  percentage and the slider rests at the top; the panel itself never goes above 100%. A failure
  shows as one red line for 5 seconds.
- **Live:** changes made by other tools (a keyboard volume key, `pavucontrol`, plugging in a USB
  headset) show up at once; the slider does not jump while you drag it.
- **Needs:** PulseAudio, or PipeWire with `pipewire-pulse`. If the server is missing the icon stays
  hidden and the panel tries again every 30 seconds.
- **Not included:** volume per application, profiles and ports, mouse wheel and middle click on the icon.
- **Balance:** setting the volume applies one value to every channel, so a left/right balance set
  elsewhere is flattened.
```

`CHANGES.md`: append at the end

```markdown
- Feature: audio tray in the panel. New crate `opusdm-audio` (GTK free): `model`, `volume` (percent <-> PulseAudio value, clamped at 100), `filter` (monitor sources are not inputs), `diff`, `error`, the `Backend` trait with `FakeBackend`, `PulseBackend` (one thread owns the libpulse threaded mainloop and context; other threads send it jobs) and `watch` (snapshot plus events, reconnects every 30 seconds and re-reports each outage). Panel: `audio_model.rs` (pure: list, icon level, tooltip, `DragGuard`, `coalesce`), `audio_center.rs` (shared state, one worker thread that merges queued volume requests) and `widgets/audio.rs` (speaker icon, popover with Output and Input sections). Tests: volume round trip, filter, diff, fake backend, row model, drag guard, coalescing; `cargo test -p opusdm-audio -- --ignored` runs the live check against the real server. Docs: `docs/usage.md` 3.32.
```

- [ ] **Step 6: Manual check in a real session, then commit**

The hub needs an X session (`./run-x11.sh` opens a nested one, see `docs/superpowers/QA-checklist.md`); the nested server shares the host's PulseAudio, so the audio tray works in it. Check each, and write in `CHANGES.md` which of these were NOT checked:

- [ ] The icon appears before the CPU monitor, next to the device icon; the tooltip shows the volume.
- [ ] Drag the Output slider: the system volume follows (compare with `pactl get-sink-volume @DEFAULT_SINK@`); the slider does not jump while dragging.
- [ ] **Mute** mutes (`pactl get-sink-mute @DEFAULT_SINK@`) and the icon shows the cross; run `pactl set-sink-mute @DEFAULT_SINK@ 0` in a terminal and the panel follows.
- [ ] Click another output device: it becomes the default (`pactl get-default-sink`), the check mark moves.
- [ ] Input section: microphone slider and mute work; no `.monitor` entry in the list.
- [ ] Run `pactl set-sink-volume @DEFAULT_SINK@ 130%`: the popover shows 130% and the slider is at the top.
- [ ] Unplug the USB headset or switch it off while it is the default output: the list updates and exactly one device has the check mark.
- [ ] `systemctl --user restart pipewire-pulse` (or `pulseaudio -k`): the icon hides, then returns by itself within about 30 seconds with a working popover.

Commit:

```bash
git add crates/panel docs/usage.md CHANGES.md Cargo.lock
git commit -m "feat(panel): audio tray with volume, mute and default device"
```

---

## Self-review (done while writing)

- **Spec coverage:** model/filter/volume/backend/pulse/error/diff modules: Tasks 1-3. Widget, popover with two sections, hidden when the server is unreachable, placement on the primary bar next to the device button: Task 5. Retry every 30 s, 5 s error line, ignoring `Changed` mid-drag, last-pending-volume-per-device: Tasks 3, 4, 5. Docs and CHANGES: Task 5.
- **Deviations from the spec, to mirror into it after the work:** the spec lists `watch(sender)` on the `Backend` trait and a `RawSource` filter; the plan follows the existing `opusdm-devices` shape instead (a `watch::spawn()` module function and `filter::is_input(name)`), and it adds a `Session` type because libpulse objects are not `Send`. The volume average comes from libpulse's `avg()` rather than a hand written function. The spec's "last pending value per device" is done by `coalesce` in the worker thread.
- **Type consistency:** `Kind`, `AudioDevice::key()`, `AudioEvent`, `WatchMsg`, `Request`, `Model::{apply, devices_of, default_of, icon_level, tooltip, is_available}` are defined once (Tasks 1, 3, 4) and used with the same names in Task 5.
