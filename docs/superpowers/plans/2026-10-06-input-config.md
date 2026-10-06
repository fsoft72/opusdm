# Input configuration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `opusdm-input-config`, a GTK tool that sets mouse, touchpad and keyboard options live, saves them on request, restores the previous values when closed without saving, and has the hub reapply the saved values at start-up and on hot plug.

**Architecture:** A new crate `crates/input-config` with a GTK-free library (model, store, parsers, command generation, `Backend` trait with an `XBackend` that runs `xinput` / `setxkbmap` / `xset`, a pure `Session` state machine) and a binary (the window). The hub depends on the library and runs a small polling thread. An internal command `OpenInputConfig` and a button in the desktop menu start the tool.

**Tech Stack:** Rust 2021, GTK4 (gtk4-rs 0.10), serde, thiserror, `xinput` / `setxkbmap` / `xset` command line tools.

**Spec:** `docs/superpowers/specs/2026-10-06-input-config-design.md`

## Global Constraints

- Work on branch `opusdm-input`. One commit per task, only the files of that task. NEVER push. NEVER add AI attribution lines to commit messages. NEVER use `--no-verify`.
- Code, comments and docs in English. NEVER write the em dash character, use `-`.
- Every function and method has a doc comment. Constants are UPPER_CASE. Exit conditions at the top of a function, one liners when possible (`if !x { return; }`).
- Text files end with an empty line. Use the project's formatter (`cargo fmt` only if the repo already uses it; check `rustfmt.toml`, do not reformat files you do not change).
- Workspace edition 2021, rust-version 1.91, `gtk4 = "0.10"` (workspace dependency `gtk4.workspace = true`).
- The settings file is `~/.config/opusdm/input.json` (`opusdm_ipc::settings::config_dir()`).
- Ranges: speed -1.0..=1.0, repeat delay 100..=2000 ms, repeat rate 1..=100 per second, at most 4 layouts.
- X11 only. Mouse live behaviour cannot be checked under Xephyr (no libinput devices there): the user checks it on the real session.
- Tests run with `cargo test -p opusdm-input-config` (and `-p opusdm-hub`, `-p opusdm-ipc` for the tasks that touch them).

## Review Focus

- Applying a layout must keep keyboard options the user already has (for example `caps:escape`): only the `grp:` switch option is edited by the UI. Test in Task 3 (`layout_command_keeps_extra_options`) and Task 2 (`xkb_query_keeps_options`).
- A pointer with no libinput properties (Xephyr, a keyboard that also shows up as a pointer) is skipped, never an error. Test in Task 3 (`devices_without_the_property_are_skipped`) and Task 2 (`device_kind_needs_libinput`).
- A hand edited or old `input.json` (speed 5.0, no layouts, missing groups, unknown fields) loads as valid settings; a malformed file is not silently lost. Tests in Task 1.
- Closing without Save when nothing changed runs no command, and after a failed apply the revert still restores the snapshot. Tests in Task 4.
- A pointer plugged in later gets the saved values, and a missing tool or no X display disables the window instead of crashing. Tests in Tasks 3 and 5.
- Save right after moving a slider must not lose the last debounced value. Task 6 (`flush` before commit); checked by hand.

---

## File Structure

New crate `crates/input-config`:

| File | Responsibility |
|---|---|
| `Cargo.toml` | package, lib `opusdm_input_config`, bin `opusdm-input-config` |
| `src/lib.rs` | module list and re-exports |
| `src/error.rs` | `InputError` |
| `src/model.rs` | `InputSettings` and its groups, ranges, `normalized()` |
| `src/store.rs` | load and save `input.json` |
| `src/parse.rs` | parsers for `xinput`, `setxkbmap`, `xset` output and `evdev.lst`; settings from properties |
| `src/change.rs` | `Change`, `diff`, `all_changes` |
| `src/commands.rs` | `PointerDevice`, `commands_for` (pure argv generation) |
| `src/backend.rs` | `Backend` trait, `XBackend`, `Status`, `session_problem` |
| `src/fake.rs` | `FakeBackend` for tests (also used by the hub tests) |
| `src/session.rs` | `Session` (snapshot, change, commit, revert) |
| `src/apply.rs` | `apply_all`, `reapply_if_changed` (used by the hub) |
| `src/main.rs` | GTK application, theme, single instance |
| `src/ui.rs` | the window |

Modified: `Cargo.toml` (workspace member), `crates/ipc/src/function.rs`, `crates/lister/src/function_editor.rs`, `crates/hub/src/{main,functions,children,desktop_menu}.rs`, `crates/hub/Cargo.toml`, `crates/hub/src/input_apply.rs` (new), `scripts/build-release.sh`, `docs/usage.md`, `CHANGES.md`.

---

### Task 1: Crate scaffold, error, model, store

**Files:**
- Create: `crates/input-config/Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/model.rs`, `src/store.rs`, `src/main.rs` (stub)
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces: `InputError`; `MouseSettings { speed: f64, flat_profile: bool, left_handed: bool, natural_scroll: bool }`; `TouchpadSettings { speed: f64, tap_to_click: bool, natural_scroll: bool, disable_while_typing: bool }`; `LayoutEntry { layout: String, variant: String }`; `KeyboardSettings { layouts: Vec<LayoutEntry>, switch_option: String, extra_options: Vec<String>, repeat_delay_ms: u32, repeat_rate: u32 }`; `InputSettings { mouse, touchpad, keyboard }` with `normalized(self) -> Self`; constants `SPEED_MIN`, `SPEED_MAX`, `REPEAT_DELAY_MIN_MS`, `REPEAT_DELAY_MAX_MS`, `REPEAT_RATE_MIN`, `REPEAT_RATE_MAX`, `MAX_LAYOUTS`; `store::input_path() -> PathBuf`, `store::load_from(&Path) -> Option<InputSettings>`, `store::save_to(&Path, &InputSettings) -> Result<(), InputError>`.

- [ ] **Step 1: Create the crate files**

`crates/input-config/Cargo.toml`:

```toml
[package]
name = "opusdm-input-config"
version = "0.5.0"
edition.workspace = true
rust-version.workspace = true

[lib]
name = "opusdm_input_config"
path = "src/lib.rs"

[[bin]]
name = "opusdm-input-config"
path = "src/main.rs"

[dependencies]
opusdm-ipc = { path = "../ipc" }
opusdm-ui = { path = "../ui" }
gtk4.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

Add `"crates/input-config"` to `members` in the root `Cargo.toml` (before `"xtask"`).

`src/lib.rs`:

```rust
//! Library of `opusdm-input-config`: the settings model, the parsers for the
//! X tools, the commands that apply a change and the live / revert session.
//! It has no GTK code, so the hub can reuse it to reapply the saved values.

pub mod error;
pub mod model;
pub mod store;

pub use error::InputError;
pub use model::*;
```

`src/main.rs` (stub, replaced in Task 6):

```rust
//! `opusdm-input-config`: mouse, touchpad and keyboard settings.

fn main() {}
```

`src/error.rs`:

```rust
//! Errors of the input configuration.

use thiserror::Error;

/// Anything that can go wrong reading or applying input settings.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum InputError {
    /// A required command line tool is not installed.
    #[error("{tool} is not installed (package: {package})")]
    MissingTool { tool: String, package: String },
    /// A tool ran and exited with an error.
    #[error("`{command}` failed: {stderr}")]
    CommandFailed { command: String, stderr: String },
    /// The settings file could not be written.
    #[error("cannot write {path}: {reason}")]
    Write { path: String, reason: String },
    /// The session cannot be configured (no X display, Wayland).
    #[error("{0}")]
    Session(String),
}
```

- [ ] **Step 2: Write the failing model and store tests**

`src/model.rs` (types and tests; implementation in step 4 of this task is already below, so first write only the test module and the empty type declarations, then run to see failures). To keep the task bite sized, write the whole file below but start by adding ONLY the `#[cfg(test)] mod tests` block and the `use` line, run `cargo test -p opusdm-input-config` and confirm it fails to compile (types missing), then add the rest.

```rust
//! The input settings model, stored in `input.json`.

use serde::{Deserialize, Serialize};

/// Lowest pointer speed (libinput `Accel Speed`).
pub const SPEED_MIN: f64 = -1.0;
/// Highest pointer speed.
pub const SPEED_MAX: f64 = 1.0;
/// Shortest delay before a held key repeats, in milliseconds.
pub const REPEAT_DELAY_MIN_MS: u32 = 100;
/// Longest delay before a held key repeats, in milliseconds.
pub const REPEAT_DELAY_MAX_MS: u32 = 2000;
/// Slowest key repeat rate, per second.
pub const REPEAT_RATE_MIN: u32 = 1;
/// Fastest key repeat rate, per second.
pub const REPEAT_RATE_MAX: u32 = 100;
/// Most keyboard layouts that can be active together.
pub const MAX_LAYOUTS: usize = 4;

/// Options shared by every mouse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseSettings {
    /// Pointer speed, -1.0 to 1.0.
    pub speed: f64,
    /// Flat acceleration profile (no acceleration curve) instead of adaptive.
    pub flat_profile: bool,
    /// Swap the left and right buttons.
    pub left_handed: bool,
    /// Natural (reversed) wheel scrolling.
    pub natural_scroll: bool,
}

impl Default for MouseSettings {
    /// The libinput defaults.
    fn default() -> Self {
        Self { speed: 0.0, flat_profile: false, left_handed: false, natural_scroll: false }
    }
}

/// Options shared by every touchpad.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TouchpadSettings {
    /// Pointer speed, -1.0 to 1.0.
    pub speed: f64,
    /// A tap is a click.
    pub tap_to_click: bool,
    /// Natural (reversed) two finger scrolling.
    pub natural_scroll: bool,
    /// Ignore the touchpad while typing.
    pub disable_while_typing: bool,
}

impl Default for TouchpadSettings {
    /// The libinput defaults (disable while typing is on).
    fn default() -> Self {
        Self { speed: 0.0, tap_to_click: false, natural_scroll: false, disable_while_typing: true }
    }
}

/// One active keyboard layout and its variant (empty = the default variant).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutEntry {
    /// XKB layout code, for example `it`.
    pub layout: String,
    /// XKB variant code, for example `intl`, or empty.
    pub variant: String,
}

/// Keyboard layouts and key repeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardSettings {
    /// Active layouts, in switching order (1 to `MAX_LAYOUTS`).
    pub layouts: Vec<LayoutEntry>,
    /// The XKB option that switches layout (`grp:alt_shift_toggle`), or empty.
    pub switch_option: String,
    /// Other XKB options already set (`caps:escape`). Kept, never edited.
    pub extra_options: Vec<String>,
    /// Delay before a held key repeats, in milliseconds.
    pub repeat_delay_ms: u32,
    /// Repeats per second.
    pub repeat_rate: u32,
}

impl Default for KeyboardSettings {
    /// US layout with the usual X repeat values.
    fn default() -> Self {
        Self {
            layouts: vec![LayoutEntry { layout: "us".to_string(), variant: String::new() }],
            switch_option: String::new(),
            extra_options: Vec::new(),
            repeat_delay_ms: 500,
            repeat_rate: 33,
        }
    }
}

/// Everything `opusdm-input-config` edits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSettings {
    /// Options shared by every mouse.
    pub mouse: MouseSettings,
    /// Options shared by every touchpad.
    pub touchpad: TouchpadSettings,
    /// Layouts and key repeat.
    pub keyboard: KeyboardSettings,
}

/// Clamp a speed into range; anything that is not a number becomes 0.
fn clamp_speed(value: f64) -> f64 {
    if !value.is_finite() { return 0.0; }
    value.clamp(SPEED_MIN, SPEED_MAX)
}

impl InputSettings {
    /// A copy that is safe to apply: speeds and repeat values clamped, empty
    /// layout codes dropped, at most `MAX_LAYOUTS` layouts and at least one.
    pub fn normalized(mut self) -> Self {
        self.mouse.speed = clamp_speed(self.mouse.speed);
        self.touchpad.speed = clamp_speed(self.touchpad.speed);
        let kb = &mut self.keyboard;
        kb.layouts.retain(|l| !l.layout.trim().is_empty());
        kb.layouts.truncate(MAX_LAYOUTS);
        if kb.layouts.is_empty() {
            kb.layouts = KeyboardSettings::default().layouts;
        }
        kb.repeat_delay_ms = kb.repeat_delay_ms.clamp(REPEAT_DELAY_MIN_MS, REPEAT_DELAY_MAX_MS);
        kb.repeat_rate = kb.repeat_rate.clamp(REPEAT_RATE_MIN, REPEAT_RATE_MAX);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_a_json_round_trip() {
        let json = serde_json::to_string(&InputSettings::default()).unwrap();
        assert_eq!(serde_json::from_str::<InputSettings>(&json).unwrap(), InputSettings::default());
    }

    #[test]
    fn a_partial_old_file_fills_the_rest_with_defaults() {
        let s: InputSettings = serde_json::from_str(r#"{"mouse":{"speed":0.5},"unknown":1}"#).unwrap();
        assert_eq!(s.mouse.speed, 0.5);
        assert!(!s.mouse.left_handed);
        assert_eq!(s.keyboard, KeyboardSettings::default());
    }

    #[test]
    fn normalized_clamps_and_repairs() {
        let mut s = InputSettings::default();
        s.mouse.speed = 5.0;
        s.touchpad.speed = f64::NAN;
        s.keyboard.layouts = vec![LayoutEntry { layout: " ".into(), variant: "x".into() }];
        s.keyboard.repeat_delay_ms = 5;
        s.keyboard.repeat_rate = 1000;
        let n = s.normalized();
        assert_eq!(n.mouse.speed, SPEED_MAX);
        assert_eq!(n.touchpad.speed, 0.0);
        assert_eq!(n.keyboard.layouts, KeyboardSettings::default().layouts);
        assert_eq!(n.keyboard.repeat_delay_ms, REPEAT_DELAY_MIN_MS);
        assert_eq!(n.keyboard.repeat_rate, REPEAT_RATE_MAX);
    }

    #[test]
    fn normalized_keeps_at_most_max_layouts() {
        let mut s = InputSettings::default();
        s.keyboard.layouts = (0..9).map(|i| LayoutEntry { layout: format!("l{i}"), variant: String::new() }).collect();
        assert_eq!(s.normalized().keyboard.layouts.len(), MAX_LAYOUTS);
    }
}
```

- [ ] **Step 3: Write `src/store.rs` with its tests**

```rust
//! Reading and writing `input.json`.

use std::path::{Path, PathBuf};

use crate::{InputError, InputSettings};

/// Tag used in the log lines of the shared JSON helpers.
const APP_TAG: &str = "opusdm-input-config";

/// Path of `input.json` under the OpusDM config directory.
pub fn input_path() -> PathBuf {
    opusdm_ipc::settings::config_dir().join("input.json")
}

/// Load the settings saved at `path`, normalized. `None` when the file is
/// missing or unusable (a malformed file is copied to `input.json.bak` by the
/// shared helper), so callers leave the system alone in that case.
pub fn load_from(path: &Path) -> Option<InputSettings> {
    let settings: InputSettings = opusdm_ipc::settings::read_json(path, APP_TAG, "input settings")?;
    Some(settings.normalized())
}

/// Write `settings` to `path` (atomically, creating the folder).
pub fn save_to(path: &Path, settings: &InputSettings) -> Result<(), InputError> {
    opusdm_ipc::settings::save_json(path, settings)
        .map_err(|e| InputError::Write { path: path.display().to_string(), reason: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("input.json");
        let mut s = InputSettings::default();
        s.mouse.speed = 0.4;
        save_to(&path, &s).unwrap();
        assert_eq!(load_from(&path), Some(s));
    }

    #[test]
    fn a_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(&dir.path().join("input.json")), None);
    }

    #[test]
    fn a_hand_edited_file_is_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        std::fs::write(&path, r#"{"mouse":{"speed":9},"keyboard":{"layouts":[]}}"#).unwrap();
        let s = load_from(&path).unwrap();
        assert_eq!(s.mouse.speed, 1.0);
        assert_eq!(s.keyboard.layouts.len(), 1);
    }

    #[test]
    fn a_malformed_file_is_none_and_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), None);
        assert!(dir.path().join("input.json.bak").exists());
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-input-config`
Expected: all tests in `model` and `store` PASS. If `a_malformed_file_is_none_and_backed_up` fails because the helper names the backup differently, read `crates/ipc/src/settings.rs` (`read_json`) and change only the expected backup file name in the test.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/input-config
git commit -m "feat(input): input-config crate with the settings model and store"
```

---

### Task 2: Parsers

**Files:**
- Create: `crates/input-config/src/parse.rs`
- Modify: `crates/input-config/src/lib.rs` (add `pub mod parse;`)

**Interfaces:**
- Consumes: `MouseSettings`, `TouchpadSettings`, `KeyboardSettings`, `LayoutEntry` (Task 1).
- Produces:
  - `RawDevice { id: u32, name: String }`, `parse_pointer_list(&str) -> Vec<RawDevice>`
  - `type Props = HashMap<String, Vec<String>>`, `parse_props(&str) -> Props`
  - `PointerKind { Mouse, Touchpad }`, `device_kind(&Props) -> Option<PointerKind>`
  - `mouse_from_props(&Props) -> MouseSettings`, `touchpad_from_props(&Props) -> TouchpadSettings`
  - `parse_xkb_query(&str) -> Option<KeyboardLayoutState>` where `KeyboardLayoutState { layouts: Vec<LayoutEntry>, switch_option: String, extra_options: Vec<String> }`
  - `parse_xset_repeat(&str) -> Option<(u32, u32)>` (delay ms, rate)
  - `XkbItem { code, description }`, `XkbVariant { layout, code, description }`, `XkbLists { layouts: Vec<XkbItem>, variants: Vec<XkbVariant>, switch_options: Vec<XkbItem> }`, `parse_evdev_lst(&str) -> XkbLists`, `load_xkb_lists() -> XkbLists`

- [ ] **Step 1: Write the failing tests**

Create `src/parse.rs` with only the test module first (it will not compile), run `cargo test -p opusdm-input-config parse`, then add the implementation of step 3.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "⎡ Virtual core pointer                    \tid=2\t[master pointer  (3)]\n\
⎜   ↳ Virtual core XTEST pointer              \tid=4\t[slave  pointer  (2)]\n\
⎜   ↳ E-Signal Trust Gaming Mouse             \tid=9\t[slave  pointer  (2)]\n\
⎜   ↳ COOLER MASTER CK352 Mechanlcal Keyboard Mouse\tid=12\t[slave  pointer  (2)]\n\
⎣ Virtual core keyboard                   \tid=3\t[master keyboard (2)]\n\
    ↳ Power Button                            \tid=6\t[slave  keyboard (3)]\n";

    const MOUSE_PROPS: &str = "Device 'E-Signal Trust Gaming Mouse':\n\
\tDevice Enabled (197):\t1\n\
\tlibinput Natural Scrolling Enabled (327):\t1\n\
\tlibinput Natural Scrolling Enabled Default (328):\t0\n\
\tlibinput Accel Speed (338):\t0.250000\n\
\tlibinput Accel Speed Default (339):\t0.000000\n\
\tlibinput Accel Profile Enabled (341):\t0, 1, 0\n\
\tlibinput Left Handed Enabled (349):\t1\n\
\tDevice Node (315):\t\"/dev/input/event4\"\n\
\tlibinput Accel Custom Fallback Points (343):\t<no items>\n";

    const TOUCHPAD_PROPS: &str = "Device 'SYNA Touchpad':\n\
\tlibinput Tapping Enabled (300):\t1\n\
\tlibinput Natural Scrolling Enabled (327):\t1\n\
\tlibinput Accel Speed (338):\t-0.500000\n\
\tlibinput Disable While Typing Enabled (360):\t0\n";

    #[test]
    fn pointer_list_keeps_slave_pointers_only() {
        let devices = parse_pointer_list(LIST);
        let ids: Vec<u32> = devices.iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![9, 12]);
        assert_eq!(devices[0].name, "E-Signal Trust Gaming Mouse");
    }

    #[test]
    fn props_are_keyed_by_name_with_their_values() {
        let p = parse_props(MOUSE_PROPS);
        assert_eq!(p["libinput Accel Speed"], vec!["0.250000"]);
        assert_eq!(p["libinput Accel Profile Enabled"], vec!["0", "1", "0"]);
        assert_eq!(p["Device Node"], vec!["\"/dev/input/event4\""]);
    }

    #[test]
    fn device_kind_needs_libinput() {
        assert_eq!(device_kind(&parse_props(MOUSE_PROPS)), Some(PointerKind::Mouse));
        assert_eq!(device_kind(&parse_props(TOUCHPAD_PROPS)), Some(PointerKind::Touchpad));
        assert_eq!(device_kind(&parse_props("Device 'Xephyr pointer':\n\tDevice Enabled (150):\t1\n")), None);
    }

    #[test]
    fn mouse_settings_come_from_the_properties() {
        let m = mouse_from_props(&parse_props(MOUSE_PROPS));
        assert_eq!(m, MouseSettings { speed: 0.25, flat_profile: true, left_handed: true, natural_scroll: true });
    }

    #[test]
    fn touchpad_settings_come_from_the_properties() {
        let t = touchpad_from_props(&parse_props(TOUCHPAD_PROPS));
        assert_eq!(t, TouchpadSettings { speed: -0.5, tap_to_click: true, natural_scroll: true, disable_while_typing: false });
    }

    #[test]
    fn xkb_query_reads_layouts_variants_and_options() {
        let out = "rules:      evdev\nmodel:      pc105\nlayout:     it,us\nvariant:    ,intl\noptions:    grp:alt_shift_toggle,caps:escape\n";
        let k = parse_xkb_query(out).unwrap();
        assert_eq!(k.layouts.len(), 2);
        assert_eq!(k.layouts[1], LayoutEntry { layout: "us".into(), variant: "intl".into() });
        assert_eq!(k.switch_option, "grp:alt_shift_toggle");
        assert_eq!(k.extra_options, vec!["caps:escape"]);
    }

    #[test]
    fn xkb_query_keeps_options() {
        let out = "layout:     it\noptions:    caps:escape\n";
        let k = parse_xkb_query(out).unwrap();
        assert_eq!(k.layouts, vec![LayoutEntry { layout: "it".into(), variant: String::new() }]);
        assert_eq!(k.switch_option, "");
        assert_eq!(k.extra_options, vec!["caps:escape"]);
    }

    #[test]
    fn xkb_query_without_a_layout_is_none() {
        assert_eq!(parse_xkb_query("rules: evdev\n"), None);
    }

    #[test]
    fn xset_repeat_values_are_read() {
        let out = "Keyboard Control:\n  auto repeat:  on    key click percent:  0    LED mask:  00000000\n  auto repeat delay:  500    repeat rate:  33\n";
        assert_eq!(parse_xset_repeat(out), Some((500, 33)));
        assert_eq!(parse_xset_repeat("nothing"), None);
    }

    #[test]
    fn evdev_lst_gives_layouts_variants_and_switch_options() {
        let text = "! model\n  pc105           Generic 105-key PC\n\n! layout\n  us              English (US)\n  it              Italian\n\n! variant\n  intl            us: English (US, intl., with dead keys)\n  winkeys         it: Italian (Windows)\n\n! option\n  grp                  Switching to another layout\n  grp:alt_shift_toggle Alt+Shift\n  caps:escape          Make Caps Lock an additional Esc\n";
        let l = parse_evdev_lst(text);
        assert_eq!(l.layouts.len(), 2);
        assert_eq!(l.layouts[1].code, "it");
        assert_eq!(l.variants[0], XkbVariant { layout: "us".into(), code: "intl".into(), description: "English (US, intl., with dead keys)".into() });
        assert_eq!(l.switch_options.len(), 1);
        assert_eq!(l.switch_options[0].code, "grp:alt_shift_toggle");
        assert_eq!(l.switch_options[0].description, "Alt+Shift");
    }
}
```

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo test -p opusdm-input-config parse`
Expected: FAIL (compile error, items not defined).

- [ ] **Step 3: Write the implementation (above the test module in `src/parse.rs`)**

```rust
//! Parsers for the output of `xinput`, `setxkbmap` and `xset`, and for the
//! XKB `evdev.lst` rules file. Pure functions: no process is started here.

use std::collections::HashMap;

use crate::{KeyboardSettings, LayoutEntry, MouseSettings, TouchpadSettings};

/// Where the XKB rules list lives.
const EVDEV_LST_PATH: &str = "/usr/share/X11/xkb/rules/evdev.lst";
/// Prefix of the XKB options that switch layout.
const SWITCH_OPTION_PREFIX: &str = "grp:";

/// A device line of `xinput list`.
#[derive(Clone, Debug, PartialEq)]
pub struct RawDevice {
    /// The X input device id.
    pub id: u32,
    /// The device name.
    pub name: String,
}

/// The properties of one device: name to values (`"0, 1, 0"` is three values).
pub type Props = HashMap<String, Vec<String>>;

/// What kind of pointer a device is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// A mouse (anything libinput that is not a touchpad).
    Mouse,
    /// A touchpad.
    Touchpad,
}

/// The slave pointers of `xinput list` output (the virtual core devices are left out).
pub fn parse_pointer_list(out: &str) -> Vec<RawDevice> {
    out.lines()
        .filter(|l| l.contains("slave  pointer"))
        .filter_map(|line| {
            let (left, _) = line.split_once('\t')?;
            let name = left.split('\u{21b3}').nth(1)?.trim().to_string();
            let id = line.split("id=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
            Some(RawDevice { id, name })
        })
        .filter(|d| !d.name.starts_with("Virtual core"))
        .collect()
}

/// The properties printed by `xinput list-props`.
pub fn parse_props(out: &str) -> Props {
    let mut props = Props::new();
    for line in out.lines().filter(|l| l.starts_with('\t')) {
        let Some((head, value)) = line.trim_start().split_once("):") else { continue };
        let Some((name, _id)) = head.rsplit_once(" (") else { continue };
        props.insert(name.to_string(), value.split(',').map(|v| v.trim().to_string()).collect());
    }
    props
}

/// Mouse or touchpad, or `None` for a device with no libinput pointer properties.
pub fn device_kind(props: &Props) -> Option<PointerKind> {
    if !props.contains_key("libinput Accel Speed") { return None; }
    if props.contains_key("libinput Tapping Enabled") { return Some(PointerKind::Touchpad); }
    Some(PointerKind::Mouse)
}

/// The first value of property `name` as a number.
fn first_f64(props: &Props, name: &str) -> Option<f64> {
    props.get(name)?.first()?.parse().ok()
}

/// Whether the first value of property `name` is `1`.
fn flag(props: &Props, name: &str) -> Option<bool> {
    Some(props.get(name)?.first()? == "1")
}

/// The mouse settings a mouse reports (defaults for what it does not expose).
pub fn mouse_from_props(props: &Props) -> MouseSettings {
    let default = MouseSettings::default();
    MouseSettings {
        speed: first_f64(props, "libinput Accel Speed").unwrap_or(default.speed),
        flat_profile: props.get("libinput Accel Profile Enabled").and_then(|v| v.get(1)).map(|v| v == "1").unwrap_or(default.flat_profile),
        left_handed: flag(props, "libinput Left Handed Enabled").unwrap_or(default.left_handed),
        natural_scroll: flag(props, "libinput Natural Scrolling Enabled").unwrap_or(default.natural_scroll),
    }
}

/// The touchpad settings a touchpad reports (defaults for what it does not expose).
pub fn touchpad_from_props(props: &Props) -> TouchpadSettings {
    let default = TouchpadSettings::default();
    TouchpadSettings {
        speed: first_f64(props, "libinput Accel Speed").unwrap_or(default.speed),
        tap_to_click: flag(props, "libinput Tapping Enabled").unwrap_or(default.tap_to_click),
        natural_scroll: flag(props, "libinput Natural Scrolling Enabled").unwrap_or(default.natural_scroll),
        disable_while_typing: flag(props, "libinput Disable While Typing Enabled").unwrap_or(default.disable_while_typing),
    }
}

/// The layout part of `setxkbmap -query`.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardLayoutState {
    /// Active layouts with their variants.
    pub layouts: Vec<LayoutEntry>,
    /// The `grp:` option, or empty.
    pub switch_option: String,
    /// Every other option.
    pub extra_options: Vec<String>,
}

/// Parse `setxkbmap -query`. `None` when there is no `layout:` line.
pub fn parse_xkb_query(out: &str) -> Option<KeyboardLayoutState> {
    let value = |key: &str| -> Option<String> {
        out.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_string())
    };
    let layouts_text = value("layout:").filter(|v| !v.is_empty())?;
    let variants_text = value("variant:").unwrap_or_default();
    let variants: Vec<&str> = variants_text.split(',').collect();
    let layouts = layouts_text
        .split(',')
        .enumerate()
        .map(|(i, l)| LayoutEntry { layout: l.trim().to_string(), variant: variants.get(i).map(|v| v.trim().to_string()).unwrap_or_default() })
        .collect();
    let options_text = value("options:").unwrap_or_default();
    let mut switch_option = String::new();
    let mut extra_options = Vec::new();
    for option in options_text.split(',').map(str::trim).filter(|o| !o.is_empty()) {
        if option.starts_with(SWITCH_OPTION_PREFIX) && switch_option.is_empty() {
            switch_option = option.to_string();
        } else {
            extra_options.push(option.to_string());
        }
    }
    Some(KeyboardLayoutState { layouts, switch_option, extra_options })
}

/// The key repeat delay (ms) and rate of `xset q`.
pub fn parse_xset_repeat(out: &str) -> Option<(u32, u32)> {
    let line = out.lines().find(|l| l.contains("auto repeat delay:"))?;
    let number_after = |key: &str| -> Option<u32> { line.split(key).nth(1)?.split_whitespace().next()?.parse().ok() };
    Some((number_after("auto repeat delay:")?, number_after("repeat rate:")?))
}

/// A layout or option with its description.
#[derive(Clone, Debug, PartialEq)]
pub struct XkbItem {
    /// The XKB code.
    pub code: String,
    /// The human description.
    pub description: String,
}

/// A layout variant.
#[derive(Clone, Debug, PartialEq)]
pub struct XkbVariant {
    /// The layout the variant belongs to.
    pub layout: String,
    /// The variant code.
    pub code: String,
    /// The human description.
    pub description: String,
}

/// What `evdev.lst` offers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XkbLists {
    /// All layouts.
    pub layouts: Vec<XkbItem>,
    /// All variants.
    pub variants: Vec<XkbVariant>,
    /// The `grp:` options (ways to switch layout).
    pub switch_options: Vec<XkbItem>,
}

/// Parse the text of `evdev.lst`.
pub fn parse_evdev_lst(text: &str) -> XkbLists {
    let mut lists = XkbLists::default();
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("! ") {
            section = name.trim();
            continue;
        }
        let Some((code, rest)) = line.trim().split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match section {
            "layout" => lists.layouts.push(XkbItem { code: code.to_string(), description: rest.to_string() }),
            "variant" => {
                let Some((layout, description)) = rest.split_once(": ") else { continue };
                lists.variants.push(XkbVariant { layout: layout.to_string(), code: code.to_string(), description: description.to_string() });
            }
            "option" if code.starts_with(SWITCH_OPTION_PREFIX) => {
                lists.switch_options.push(XkbItem { code: code.to_string(), description: rest.to_string() });
            }
            _ => {}
        }
    }
    lists
}

/// Read `evdev.lst` from the system; empty lists when it is missing.
pub fn load_xkb_lists() -> XkbLists {
    std::fs::read_to_string(EVDEV_LST_PATH).map(|t| parse_evdev_lst(&t)).unwrap_or_default()
}

/// Build the keyboard settings from the two tool outputs; `None` when the
/// layout cannot be read. The repeat values fall back to the defaults.
pub fn keyboard_from_outputs(xkb_query: &str, xset_q: &str) -> Option<KeyboardSettings> {
    let layout = parse_xkb_query(xkb_query)?;
    let default = KeyboardSettings::default();
    let (repeat_delay_ms, repeat_rate) = parse_xset_repeat(xset_q).unwrap_or((default.repeat_delay_ms, default.repeat_rate));
    Some(KeyboardSettings {
        layouts: layout.layouts,
        switch_option: layout.switch_option,
        extra_options: layout.extra_options,
        repeat_delay_ms,
        repeat_rate,
    })
}
```

Add `pub mod parse;` to `src/lib.rs`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-input-config parse`
Expected: PASS (10 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/input-config/src/parse.rs crates/input-config/src/lib.rs
git commit -m "feat(input): parsers for xinput, setxkbmap, xset and evdev.lst"
```

---

### Task 3: Changes and command generation

**Files:**
- Create: `crates/input-config/src/change.rs`, `crates/input-config/src/commands.rs`
- Modify: `crates/input-config/src/lib.rs` (add `pub mod change; pub mod commands;`)

**Interfaces:**
- Consumes: Task 1 model, Task 2 `Props`, `PointerKind`.
- Produces:
  - `Change` enum: `MouseSpeed(f64)`, `MouseFlatProfile(bool)`, `MouseLeftHanded(bool)`, `MouseNaturalScroll(bool)`, `TouchpadSpeed(f64)`, `TouchpadTap(bool)`, `TouchpadNaturalScroll(bool)`, `TouchpadDisableWhileTyping(bool)`, `Layout { layouts: Vec<LayoutEntry>, switch_option: String, extra_options: Vec<String> }`, `Repeat { delay_ms: u32, rate: u32 }`
  - `diff(&InputSettings, &InputSettings) -> Vec<Change>`, `all_changes(&InputSettings) -> Vec<Change>`
  - `PointerDevice { id: u32, name: String, kind: PointerKind, props: Props }`, `type Argv = Vec<String>`, `commands_for(&Change, &[PointerDevice]) -> Vec<Argv>`

- [ ] **Step 1: Write the failing tests for `change.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_settings_have_no_changes() {
        let s = InputSettings::default();
        assert!(diff(&s, &s.clone()).is_empty());
    }

    #[test]
    fn only_the_changed_fields_appear() {
        let old = InputSettings::default();
        let mut new = old.clone();
        new.mouse.speed = 0.5;
        new.touchpad.tap_to_click = true;
        new.keyboard.repeat_rate = 40;
        assert_eq!(
            diff(&old, &new),
            vec![Change::MouseSpeed(0.5), Change::TouchpadTap(true), Change::Repeat { delay_ms: 500, rate: 40 }]
        );
    }

    #[test]
    fn a_layout_change_carries_the_whole_layout_state() {
        let old = InputSettings::default();
        let mut new = old.clone();
        new.keyboard.switch_option = "grp:alt_shift_toggle".into();
        new.keyboard.extra_options = vec!["caps:escape".into()];
        let changes = diff(&old, &new);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::Layout { switch_option, extra_options, .. } => {
                assert_eq!(switch_option, "grp:alt_shift_toggle");
                assert_eq!(extra_options, &vec!["caps:escape".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn all_changes_covers_every_field() {
        assert_eq!(all_changes(&InputSettings::default()).len(), 10);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-input-config change`
Expected: FAIL (compile error).

- [ ] **Step 3: Write `change.rs`**

```rust
//! One field of the settings that changed, and the diff between two settings.

use crate::{InputSettings, LayoutEntry};

/// Speeds closer than this are the same value.
const SPEED_EPSILON: f64 = 1e-9;

/// One thing to apply to the system.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Mouse pointer speed.
    MouseSpeed(f64),
    /// Mouse flat acceleration profile on or off.
    MouseFlatProfile(bool),
    /// Mouse buttons swapped.
    MouseLeftHanded(bool),
    /// Mouse natural scrolling.
    MouseNaturalScroll(bool),
    /// Touchpad pointer speed.
    TouchpadSpeed(f64),
    /// Touchpad tap to click.
    TouchpadTap(bool),
    /// Touchpad natural scrolling.
    TouchpadNaturalScroll(bool),
    /// Touchpad disabled while typing.
    TouchpadDisableWhileTyping(bool),
    /// Keyboard layouts (the layout list, the switch option and the other options go together).
    Layout { layouts: Vec<LayoutEntry>, switch_option: String, extra_options: Vec<String> },
    /// Key repeat.
    Repeat { delay_ms: u32, rate: u32 },
}

/// Whether two speeds are the same value.
fn same_speed(a: f64, b: f64) -> bool {
    (a - b).abs() < SPEED_EPSILON
}

/// The changes that turn `old` into `new`, in a fixed order.
pub fn diff(old: &InputSettings, new: &InputSettings) -> Vec<Change> {
    let mut changes = Vec::new();
    if !same_speed(old.mouse.speed, new.mouse.speed) { changes.push(Change::MouseSpeed(new.mouse.speed)); }
    if old.mouse.flat_profile != new.mouse.flat_profile { changes.push(Change::MouseFlatProfile(new.mouse.flat_profile)); }
    if old.mouse.left_handed != new.mouse.left_handed { changes.push(Change::MouseLeftHanded(new.mouse.left_handed)); }
    if old.mouse.natural_scroll != new.mouse.natural_scroll { changes.push(Change::MouseNaturalScroll(new.mouse.natural_scroll)); }
    if !same_speed(old.touchpad.speed, new.touchpad.speed) { changes.push(Change::TouchpadSpeed(new.touchpad.speed)); }
    if old.touchpad.tap_to_click != new.touchpad.tap_to_click { changes.push(Change::TouchpadTap(new.touchpad.tap_to_click)); }
    if old.touchpad.natural_scroll != new.touchpad.natural_scroll { changes.push(Change::TouchpadNaturalScroll(new.touchpad.natural_scroll)); }
    if old.touchpad.disable_while_typing != new.touchpad.disable_while_typing {
        changes.push(Change::TouchpadDisableWhileTyping(new.touchpad.disable_while_typing));
    }
    let (o, n) = (&old.keyboard, &new.keyboard);
    if o.layouts != n.layouts || o.switch_option != n.switch_option || o.extra_options != n.extra_options {
        changes.push(Change::Layout { layouts: n.layouts.clone(), switch_option: n.switch_option.clone(), extra_options: n.extra_options.clone() });
    }
    if o.repeat_delay_ms != n.repeat_delay_ms || o.repeat_rate != n.repeat_rate {
        changes.push(Change::Repeat { delay_ms: n.repeat_delay_ms, rate: n.repeat_rate });
    }
    changes
}

/// Every setting as a change, for applying a saved file to the whole system.
pub fn all_changes(settings: &InputSettings) -> Vec<Change> {
    let k = &settings.keyboard;
    vec![
        Change::MouseSpeed(settings.mouse.speed),
        Change::MouseFlatProfile(settings.mouse.flat_profile),
        Change::MouseLeftHanded(settings.mouse.left_handed),
        Change::MouseNaturalScroll(settings.mouse.natural_scroll),
        Change::TouchpadSpeed(settings.touchpad.speed),
        Change::TouchpadTap(settings.touchpad.tap_to_click),
        Change::TouchpadNaturalScroll(settings.touchpad.natural_scroll),
        Change::TouchpadDisableWhileTyping(settings.touchpad.disable_while_typing),
        Change::Layout { layouts: k.layouts.clone(), switch_option: k.switch_option.clone(), extra_options: k.extra_options.clone() },
        Change::Repeat { delay_ms: k.repeat_delay_ms, rate: k.repeat_rate },
    ]
}
```

Put the test module of step 1 at the bottom. Add `pub mod change;` to `lib.rs`.

- [ ] **Step 4: Run the change tests**

Run: `cargo test -p opusdm-input-config change`
Expected: PASS.

- [ ] **Step 5: Write the failing tests for `commands.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_props;

    fn device(id: u32, kind: PointerKind, props_text: &str) -> PointerDevice {
        PointerDevice { id, name: format!("dev{id}"), kind, props: parse_props(props_text) }
    }

    const MOUSE: &str = "Device 'm':\n\tlibinput Accel Speed (1):\t0.0\n\tlibinput Accel Profile Enabled (2):\t1, 0, 0\n\tlibinput Left Handed Enabled (3):\t0\n\tlibinput Natural Scrolling Enabled (4):\t0\n";
    const PAD: &str = "Device 'p':\n\tlibinput Accel Speed (1):\t0.0\n\tlibinput Tapping Enabled (5):\t0\n\tlibinput Natural Scrolling Enabled (4):\t0\n\tlibinput Disable While Typing Enabled (6):\t1\n";

    fn argv(items: &[&str]) -> Argv {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mouse_speed_goes_to_every_mouse_and_no_touchpad() {
        let devices = [device(9, PointerKind::Mouse, MOUSE), device(10, PointerKind::Mouse, MOUSE), device(11, PointerKind::Touchpad, PAD)];
        let cmds = commands_for(&Change::MouseSpeed(0.5), &devices);
        assert_eq!(cmds, vec![argv(&["xinput", "set-prop", "9", "libinput Accel Speed", "0.50"]), argv(&["xinput", "set-prop", "10", "libinput Accel Speed", "0.50"])]);
    }

    #[test]
    fn flat_profile_sets_all_three_profile_values() {
        let devices = [device(9, PointerKind::Mouse, MOUSE)];
        assert_eq!(commands_for(&Change::MouseFlatProfile(true), &devices), vec![argv(&["xinput", "set-prop", "9", "libinput Accel Profile Enabled", "0", "1", "0"])]);
        assert_eq!(commands_for(&Change::MouseFlatProfile(false), &devices), vec![argv(&["xinput", "set-prop", "9", "libinput Accel Profile Enabled", "1", "0", "0"])]);
    }

    #[test]
    fn touchpad_options_use_their_properties() {
        let devices = [device(11, PointerKind::Touchpad, PAD)];
        assert_eq!(commands_for(&Change::TouchpadTap(true), &devices), vec![argv(&["xinput", "set-prop", "11", "libinput Tapping Enabled", "1"])]);
        assert_eq!(commands_for(&Change::TouchpadDisableWhileTyping(false), &devices), vec![argv(&["xinput", "set-prop", "11", "libinput Disable While Typing Enabled", "0"])]);
    }

    #[test]
    fn devices_without_the_property_are_skipped() {
        let bare = PointerDevice { id: 12, name: "bare".into(), kind: PointerKind::Mouse, props: Default::default() };
        assert!(commands_for(&Change::MouseLeftHanded(true), &[bare]).is_empty());
        assert!(commands_for(&Change::MouseSpeed(0.1), &[]).is_empty());
    }

    #[test]
    fn layout_command_sets_layouts_variants_and_options() {
        let change = Change::Layout {
            layouts: vec![LayoutEntry { layout: "it".into(), variant: String::new() }, LayoutEntry { layout: "us".into(), variant: "intl".into() }],
            switch_option: "grp:alt_shift_toggle".into(),
            extra_options: vec![],
        };
        assert_eq!(
            commands_for(&change, &[]),
            vec![argv(&["setxkbmap", "-layout", "it,us", "-variant", ",intl", "-option", "", "-option", "grp:alt_shift_toggle"])]
        );
    }

    #[test]
    fn layout_command_keeps_extra_options() {
        let change = Change::Layout {
            layouts: vec![LayoutEntry { layout: "it".into(), variant: String::new() }],
            switch_option: String::new(),
            extra_options: vec!["caps:escape".into()],
        };
        assert_eq!(commands_for(&change, &[]), vec![argv(&["setxkbmap", "-layout", "it", "-variant", "", "-option", "", "-option", "caps:escape"])]);
    }

    #[test]
    fn layout_command_with_no_options_only_clears_them() {
        let change = Change::Layout { layouts: vec![LayoutEntry { layout: "it".into(), variant: String::new() }], switch_option: String::new(), extra_options: vec![] };
        assert_eq!(commands_for(&change, &[]), vec![argv(&["setxkbmap", "-layout", "it", "-variant", "", "-option", ""])]);
    }

    #[test]
    fn repeat_uses_xset() {
        assert_eq!(commands_for(&Change::Repeat { delay_ms: 300, rate: 40 }, &[]), vec![argv(&["xset", "r", "rate", "300", "40"])]);
    }
}
```

- [ ] **Step 6: Run to verify failure**

Run: `cargo test -p opusdm-input-config commands`
Expected: FAIL (compile error).

- [ ] **Step 7: Write `commands.rs`**

```rust
//! Turns a `Change` into the command lines that apply it. Pure: nothing is
//! started here, so the generation is unit tested.

use crate::change::Change;
use crate::parse::{PointerKind, Props};
use crate::LayoutEntry;

/// A command line: program first, then its arguments (no shell involved).
pub type Argv = Vec<String>;

/// A pointer device and the libinput properties it exposes.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerDevice {
    /// The X input device id.
    pub id: u32,
    /// The device name.
    pub name: String,
    /// Mouse or touchpad.
    pub kind: PointerKind,
    /// Its properties.
    pub props: Props,
}

/// `xinput set-prop` for every device of `kind` that exposes `property`.
fn set_prop(devices: &[PointerDevice], kind: PointerKind, property: &str, values: &[&str]) -> Vec<Argv> {
    devices
        .iter()
        .filter(|d| d.kind == kind && d.props.contains_key(property))
        .map(|d| {
            let mut argv: Argv = vec!["xinput".into(), "set-prop".into(), d.id.to_string(), property.into()];
            argv.extend(values.iter().map(|v| v.to_string()));
            argv
        })
        .collect()
}

/// `"1"` or `"0"`.
fn bit(on: bool) -> &'static str {
    if on { "1" } else { "0" }
}

/// The `setxkbmap` call that sets layouts, variants and options at once.
/// `-option ""` clears the old options first; the extra options are put back.
fn layout_argv(layouts: &[LayoutEntry], switch_option: &str, extra_options: &[String]) -> Argv {
    let codes: Vec<&str> = layouts.iter().map(|l| l.layout.as_str()).collect();
    let variants: Vec<&str> = layouts.iter().map(|l| l.variant.as_str()).collect();
    let mut argv: Argv = vec!["setxkbmap".into(), "-layout".into(), codes.join(","), "-variant".into(), variants.join(","), "-option".into(), String::new()];
    let mut options: Vec<&str> = extra_options.iter().map(String::as_str).collect();
    if !switch_option.is_empty() { options.push(switch_option); }
    if !options.is_empty() {
        argv.push("-option".into());
        argv.push(options.join(","));
    }
    argv
}

/// The commands that apply `change` to `devices` (and to the keyboard).
pub fn commands_for(change: &Change, devices: &[PointerDevice]) -> Vec<Argv> {
    use PointerKind::{Mouse, Touchpad};
    match change {
        Change::MouseSpeed(v) => set_prop(devices, Mouse, "libinput Accel Speed", &[&format!("{v:.2}")]),
        Change::MouseFlatProfile(flat) => {
            let values = if *flat { ["0", "1", "0"] } else { ["1", "0", "0"] };
            set_prop(devices, Mouse, "libinput Accel Profile Enabled", &values)
        }
        Change::MouseLeftHanded(on) => set_prop(devices, Mouse, "libinput Left Handed Enabled", &[bit(*on)]),
        Change::MouseNaturalScroll(on) => set_prop(devices, Mouse, "libinput Natural Scrolling Enabled", &[bit(*on)]),
        Change::TouchpadSpeed(v) => set_prop(devices, Touchpad, "libinput Accel Speed", &[&format!("{v:.2}")]),
        Change::TouchpadTap(on) => set_prop(devices, Touchpad, "libinput Tapping Enabled", &[bit(*on)]),
        Change::TouchpadNaturalScroll(on) => set_prop(devices, Touchpad, "libinput Natural Scrolling Enabled", &[bit(*on)]),
        Change::TouchpadDisableWhileTyping(on) => set_prop(devices, Touchpad, "libinput Disable While Typing Enabled", &[bit(*on)]),
        Change::Layout { layouts, switch_option, extra_options } => vec![layout_argv(layouts, switch_option, extra_options)],
        Change::Repeat { delay_ms, rate } => vec![vec!["xset".into(), "r".into(), "rate".into(), delay_ms.to_string(), rate.to_string()]],
    }
}
```

Add `pub mod commands;` to `lib.rs`.

- [ ] **Step 8: Run all tests and commit**

Run: `cargo test -p opusdm-input-config`
Expected: PASS.

```bash
git add crates/input-config/src
git commit -m "feat(input): changes, diff and command generation"
```

---

### Task 4: Backend, fake backend and Session

**Files:**
- Create: `crates/input-config/src/backend.rs`, `src/fake.rs`, `src/session.rs`, `src/apply.rs`
- Modify: `crates/input-config/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1 to 3.
- Produces:
  - `GroupStatus { available: bool, reason: String }`, `Status { mouse, touchpad, keyboard: GroupStatus }`
  - trait `Backend { fn status(&self) -> Status; fn read_current(&self) -> InputSettings; fn apply(&self, change: &Change) -> Result<(), InputError>; fn signature(&self) -> String; fn refresh(&self); }`
  - `XBackend::new() -> XBackend`, `session_problem() -> Option<String>`, `session_problem_from(display: Option<&str>, session_type: Option<&str>) -> Option<String>`
  - `FakeBackend` (`new(current: InputSettings)`, `applied() -> Vec<Change>`, `set_failing(bool)`, `set_signature(&str)`, `refresh_count() -> usize`)
  - `Session<B: Backend>`: `open(B) -> Session<B>`, `status() -> Status`, `current() -> &InputSettings`, `snapshot() -> &InputSettings`, `change(&mut self, InputSettings) -> Vec<InputError>`, `commit(&mut self, &Path) -> Result<(), InputError>`, `revert(&mut self) -> Vec<InputError>`
  - `apply_all(&impl Backend, &InputSettings) -> Vec<InputError>`, `reapply_if_changed(&impl Backend, &mut Option<String>, impl FnOnce() -> Option<InputSettings>) -> Vec<InputError>`

- [ ] **Step 1: Write `backend.rs` (trait, `XBackend`, `session_problem`) with tests for the pure parts**

```rust
//! The boundary to the X tools. `Backend` is what the session and the hub
//! use; `XBackend` is the real one (it starts `xinput`, `setxkbmap`, `xset`).

use std::cell::RefCell;
use std::process::Command;

use crate::change::Change;
use crate::commands::{commands_for, PointerDevice};
use crate::parse::{self, device_kind, keyboard_from_outputs, parse_pointer_list, parse_props, PointerKind};
use crate::{InputError, InputSettings, KeyboardSettings};

/// Whether a group of settings can be used, and why not.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupStatus {
    /// The group can be edited.
    pub available: bool,
    /// Why it cannot (empty when available).
    pub reason: String,
}

impl GroupStatus {
    /// An editable group.
    pub fn ok() -> Self {
        Self { available: true, reason: String::new() }
    }

    /// A group that cannot be used, with the reason to show.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self { available: false, reason: reason.into() }
    }
}

/// The state of the three groups.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// Mouse group.
    pub mouse: GroupStatus,
    /// Touchpad group.
    pub touchpad: GroupStatus,
    /// Keyboard group.
    pub keyboard: GroupStatus,
}

impl Status {
    /// Every group editable.
    pub fn all_ok() -> Self {
        Self { mouse: GroupStatus::ok(), touchpad: GroupStatus::ok(), keyboard: GroupStatus::ok() }
    }
}

/// What reads and changes the system input settings.
pub trait Backend {
    /// Which groups can be used.
    fn status(&self) -> Status;
    /// The settings the system has right now.
    fn read_current(&self) -> InputSettings;
    /// Apply one change.
    fn apply(&self, change: &Change) -> Result<(), InputError>;
    /// A string that changes when a pointer is plugged in or removed.
    fn signature(&self) -> String;
    /// Look at the devices again (after `signature` changed).
    fn refresh(&self);
}

/// Why the session cannot be configured, or `None`.
pub fn session_problem_from(display: Option<&str>, session_type: Option<&str>) -> Option<String> {
    if display.map(str::is_empty).unwrap_or(true) {
        return Some("No X display: run opusdm-input-config inside the OpusDM session.".to_string());
    }
    if session_type == Some("wayland") {
        return Some("Input settings work on X11 sessions only.".to_string());
    }
    None
}

/// `session_problem_from` for the current environment.
pub fn session_problem() -> Option<String> {
    session_problem_from(std::env::var("DISPLAY").ok().as_deref(), std::env::var("XDG_SESSION_TYPE").ok().as_deref())
}

/// The Debian package that holds `tool`, for the error message.
fn package_of(tool: &str) -> &'static str {
    match tool {
        "xinput" => "xinput",
        "setxkbmap" => "x11-xkb-utils",
        _ => "x11-xserver-utils",
    }
}

/// Run `argv` and return its stdout.
fn run(argv: &[String]) -> Result<String, InputError> {
    let Some(program) = argv.first() else { return Err(InputError::Session("empty command".to_string())) };
    let output = Command::new(program).args(&argv[1..]).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            InputError::MissingTool { tool: program.clone(), package: package_of(program).to_string() }
        } else {
            InputError::CommandFailed { command: argv.join(" "), stderr: e.to_string() }
        }
    })?;
    if !output.status.success() {
        return Err(InputError::CommandFailed { command: argv.join(" "), stderr: String::from_utf8_lossy(&output.stderr).trim().to_string() });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// `run` for a literal command line.
fn run_str(argv: &[&str]) -> Result<String, InputError> {
    run(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

/// What one look at the system found.
#[derive(Default)]
struct Probe {
    devices: Vec<PointerDevice>,
    pointer_error: Option<String>,
    keyboard: Option<KeyboardSettings>,
    keyboard_error: Option<String>,
}

/// Read the pointers and the keyboard.
fn probe() -> Probe {
    let mut probe = Probe::default();
    match run_str(&["xinput", "list"]) {
        Ok(list) => {
            for raw in parse_pointer_list(&list) {
                let Ok(text) = run_str(&["xinput", "list-props", &raw.id.to_string()]) else { continue };
                let props = parse_props(&text);
                let Some(kind) = device_kind(&props) else { continue };
                probe.devices.push(PointerDevice { id: raw.id, name: raw.name, kind, props });
            }
        }
        Err(e) => probe.pointer_error = Some(e.to_string()),
    }
    let query = run_str(&["setxkbmap", "-query"]);
    let xset = run_str(&["xset", "q"]);
    match (query, xset) {
        (Ok(q), Ok(x)) => match keyboard_from_outputs(&q, &x) {
            Some(k) => probe.keyboard = Some(k),
            None => probe.keyboard_error = Some("cannot read the keyboard layout".to_string()),
        },
        (Err(e), _) | (_, Err(e)) => probe.keyboard_error = Some(e.to_string()),
    }
    probe
}

/// The real backend: runs the X command line tools.
pub struct XBackend {
    probe: RefCell<Probe>,
}

impl XBackend {
    /// Look at the system once and keep what was found.
    pub fn new() -> Self {
        Self { probe: RefCell::new(probe()) }
    }

    /// The group status for devices of `kind`.
    fn pointer_status(&self, kind: PointerKind, none_found: &str) -> GroupStatus {
        let probe = self.probe.borrow();
        if let Some(e) = &probe.pointer_error { return GroupStatus::unavailable(e.clone()); }
        if probe.devices.iter().any(|d| d.kind == kind) { return GroupStatus::ok(); }
        GroupStatus::unavailable(none_found)
    }
}

impl Default for XBackend {
    /// Same as `new`.
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for XBackend {
    /// Mouse and touchpad need a libinput device, the keyboard needs both tools.
    fn status(&self) -> Status {
        let keyboard = match &self.probe.borrow().keyboard_error {
            Some(e) => GroupStatus::unavailable(e.clone()),
            None => GroupStatus::ok(),
        };
        Status {
            mouse: self.pointer_status(PointerKind::Mouse, "No mouse with libinput settings was found."),
            touchpad: self.pointer_status(PointerKind::Touchpad, "No touchpad was found."),
            keyboard,
        }
    }

    /// The first mouse, the first touchpad and the keyboard as the system has them.
    fn read_current(&self) -> InputSettings {
        let probe = self.probe.borrow();
        let first = |kind: PointerKind| probe.devices.iter().find(|d| d.kind == kind);
        InputSettings {
            mouse: first(PointerKind::Mouse).map(|d| parse::mouse_from_props(&d.props)).unwrap_or_default(),
            touchpad: first(PointerKind::Touchpad).map(|d| parse::touchpad_from_props(&d.props)).unwrap_or_default(),
            keyboard: probe.keyboard.clone().unwrap_or_default(),
        }
    }

    /// Run the commands of `change`; the first failure is returned.
    fn apply(&self, change: &Change) -> Result<(), InputError> {
        let commands = commands_for(change, &self.probe.borrow().devices);
        for argv in commands {
            run(&argv)?;
        }
        Ok(())
    }

    /// The pointer ids and names, one per line.
    fn signature(&self) -> String {
        match run_str(&["xinput", "list"]) {
            Ok(list) => parse_pointer_list(&list).iter().map(|d| format!("{}:{}", d.id, d.name)).collect::<Vec<_>>().join("\n"),
            Err(e) => format!("error:{e}"),
        }
    }

    /// Probe the system again.
    fn refresh(&self) {
        *self.probe.borrow_mut() = probe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_display_or_wayland_is_a_problem() {
        assert!(session_problem_from(None, None).is_some());
        assert!(session_problem_from(Some(""), None).is_some());
        assert!(session_problem_from(Some(":0"), Some("wayland")).is_some());
        assert_eq!(session_problem_from(Some(":0"), Some("x11")), None);
        assert_eq!(session_problem_from(Some(":0"), None), None);
    }

    #[test]
    fn a_missing_tool_is_reported_with_its_package() {
        let err = run_str(&["opusdm-no-such-tool"]).unwrap_err();
        assert!(matches!(err, InputError::MissingTool { .. }));
    }

    #[test]
    fn a_failing_command_reports_its_stderr() {
        let err = run_str(&["sh", "-c", "echo boom >&2; exit 3"]).unwrap_err();
        match err {
            InputError::CommandFailed { stderr, .. } => assert_eq!(stderr, "boom"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Write `fake.rs`**

```rust
//! A `Backend` that records what is applied, for tests of the session, the
//! hub logic and the UI helpers.

use std::cell::{Cell, RefCell};

use crate::backend::{Backend, Status};
use crate::change::Change;
use crate::{InputError, InputSettings};

/// Records every applied change; can be told to fail.
pub struct FakeBackend {
    current: InputSettings,
    status: Status,
    applied: RefCell<Vec<Change>>,
    failing: Cell<bool>,
    signature: RefCell<String>,
    refreshes: Cell<usize>,
}

impl FakeBackend {
    /// A backend whose system has `current` and every group available.
    pub fn new(current: InputSettings) -> Self {
        Self {
            current,
            status: Status::all_ok(),
            applied: RefCell::new(Vec::new()),
            failing: Cell::new(false),
            signature: RefCell::new(String::new()),
            refreshes: Cell::new(0),
        }
    }

    /// The changes applied so far.
    pub fn applied(&self) -> Vec<Change> {
        self.applied.borrow().clone()
    }

    /// Make every `apply` fail (or work again).
    pub fn set_failing(&self, failing: bool) {
        self.failing.set(failing);
    }

    /// Set what `signature` returns.
    pub fn set_signature(&self, signature: &str) {
        *self.signature.borrow_mut() = signature.to_string();
    }

    /// How many times `refresh` was called.
    pub fn refresh_count(&self) -> usize {
        self.refreshes.get()
    }
}

impl Backend for FakeBackend {
    /// All groups available.
    fn status(&self) -> Status {
        self.status.clone()
    }

    /// The settings given to `new`.
    fn read_current(&self) -> InputSettings {
        self.current.clone()
    }

    /// Record the change, or fail when told to (the change is still recorded).
    fn apply(&self, change: &Change) -> Result<(), InputError> {
        self.applied.borrow_mut().push(change.clone());
        if self.failing.get() {
            return Err(InputError::CommandFailed { command: "fake".to_string(), stderr: "forced failure".to_string() });
        }
        Ok(())
    }

    /// The value set with `set_signature`.
    fn signature(&self) -> String {
        self.signature.borrow().clone()
    }

    /// Count the call.
    fn refresh(&self) {
        self.refreshes.set(self.refreshes.get() + 1);
    }
}
```

- [ ] **Step 3: Write the failing `Session` tests, then `session.rs`**

```rust
//! The live / revert state machine behind the Input window. No GTK here.

use std::path::Path;

use crate::backend::{Backend, Status};
use crate::change::diff;
use crate::{store, InputError, InputSettings};

/// Settings being edited live: what was there when the window opened (the
/// snapshot), what is applied now, and whether Save was pressed.
pub struct Session<B: Backend> {
    backend: B,
    snapshot: InputSettings,
    current: InputSettings,
    saved: bool,
}

impl<B: Backend> Session<B> {
    /// Start a session: the snapshot is what the system has now.
    pub fn open(backend: B) -> Self {
        let snapshot = backend.read_current().normalized();
        Self { backend, current: snapshot.clone(), snapshot, saved: false }
    }

    /// Which groups can be edited.
    pub fn status(&self) -> Status {
        self.backend.status()
    }

    /// What is applied right now.
    pub fn current(&self) -> &InputSettings {
        &self.current
    }

    /// What the system had when the session opened.
    pub fn snapshot(&self) -> &InputSettings {
        &self.snapshot
    }

    /// Apply the differences between the current settings and `new`. Returns
    /// the errors of the commands that failed; `new` is kept either way, so a
    /// later revert still compares against it.
    pub fn change(&mut self, new: InputSettings) -> Vec<InputError> {
        let new = new.normalized();
        let errors = self.apply_diff(&new);
        self.current = new;
        self.saved = false;
        errors
    }

    /// Write the current settings to `path`; they stay applied.
    pub fn commit(&mut self, path: &Path) -> Result<(), InputError> {
        store::save_to(path, &self.current)?;
        self.saved = true;
        Ok(())
    }

    /// Put the snapshot back, unless the settings were saved.
    pub fn revert(&mut self) -> Vec<InputError> {
        if self.saved { return Vec::new(); }
        let snapshot = self.snapshot.clone();
        let errors = self.apply_diff(&snapshot);
        self.current = snapshot;
        errors
    }

    /// Apply every difference between the current settings and `target`.
    fn apply_diff(&self, target: &InputSettings) -> Vec<InputError> {
        diff(&self.current, target).iter().filter_map(|c| self.backend.apply(c).err()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;
    use crate::fake::FakeBackend;

    fn session() -> Session<FakeBackend> {
        Session::open(FakeBackend::new(InputSettings::default()))
    }

    fn with_speed(speed: f64) -> InputSettings {
        let mut s = InputSettings::default();
        s.mouse.speed = speed;
        s
    }

    #[test]
    fn a_change_applies_only_the_difference() {
        let mut s = session();
        assert!(s.change(with_speed(0.5)).is_empty());
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(0.5)]);
        assert_eq!(s.current().mouse.speed, 0.5);
    }

    #[test]
    fn an_identical_change_runs_nothing() {
        let mut s = session();
        s.change(InputSettings::default());
        assert!(s.backend.applied().is_empty());
    }

    #[test]
    fn revert_restores_the_snapshot() {
        let mut s = session();
        s.change(with_speed(0.5));
        assert!(s.revert().is_empty());
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(0.5), Change::MouseSpeed(0.0)]);
        assert_eq!(s.current(), s.snapshot());
    }

    #[test]
    fn revert_with_no_changes_runs_no_command() {
        let mut s = session();
        assert!(s.revert().is_empty());
        assert!(s.backend.applied().is_empty());
    }

    #[test]
    fn revert_after_a_save_keeps_the_saved_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        let mut s = session();
        s.change(with_speed(0.5));
        s.commit(&path).unwrap();
        assert!(s.revert().is_empty());
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(0.5)]);
        assert_eq!(store::load_from(&path).unwrap().mouse.speed, 0.5);
    }

    #[test]
    fn a_change_after_a_save_can_be_reverted() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = session();
        s.change(with_speed(0.5));
        s.commit(&dir.path().join("input.json")).unwrap();
        s.change(with_speed(0.9));
        s.revert();
        assert_eq!(s.current().mouse.speed, 0.0);
    }

    #[test]
    fn revert_still_restores_after_a_failed_apply() {
        let mut s = session();
        s.backend.set_failing(true);
        assert_eq!(s.change(with_speed(0.5)).len(), 1);
        s.backend.set_failing(false);
        assert!(s.revert().is_empty());
        assert_eq!(s.backend.applied().last(), Some(&Change::MouseSpeed(0.0)));
    }

    #[test]
    fn out_of_range_values_are_clamped_before_applying() {
        let mut s = session();
        s.change(with_speed(7.0));
        assert_eq!(s.backend.applied(), vec![Change::MouseSpeed(1.0)]);
    }

    #[test]
    fn a_failed_save_is_an_error_and_does_not_count_as_saved() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        let mut s = session();
        s.change(with_speed(0.5));
        assert!(s.commit(&blocker.join("input.json")).is_err());
        s.revert();
        assert_eq!(s.current().mouse.speed, 0.0);
    }
}
```

- [ ] **Step 4: Write `apply.rs` with tests**

```rust
//! Applying saved settings to the whole system: at start-up and when a
//! pointer is plugged in. Used by the hub.

use crate::backend::Backend;
use crate::change::all_changes;
use crate::{InputError, InputSettings};

/// Apply every setting of `settings`; returns the errors of the commands that failed.
pub fn apply_all(backend: &impl Backend, settings: &InputSettings) -> Vec<InputError> {
    all_changes(settings).iter().filter_map(|c| backend.apply(c).err()).collect()
}

/// When the pointer signature differs from `last`: look at the devices again,
/// load the saved settings with `saved` and apply them. Does nothing, and
/// reads no file, while the signature is the same.
pub fn reapply_if_changed(backend: &impl Backend, last: &mut Option<String>, saved: impl FnOnce() -> Option<InputSettings>) -> Vec<InputError> {
    let now = backend.signature();
    if last.as_deref() == Some(now.as_str()) { return Vec::new(); }
    *last = Some(now);
    backend.refresh();
    let Some(settings) = saved() else { return Vec::new() };
    apply_all(backend, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeBackend;

    fn backend() -> FakeBackend {
        FakeBackend::new(InputSettings::default())
    }

    #[test]
    fn apply_all_runs_every_change() {
        let b = backend();
        assert!(apply_all(&b, &InputSettings::default()).is_empty());
        assert_eq!(b.applied().len(), 10);
    }

    #[test]
    fn the_first_look_applies_the_saved_settings() {
        let b = backend();
        let mut last = None;
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        assert_eq!(b.applied().len(), 10);
        assert_eq!(b.refresh_count(), 1);
    }

    #[test]
    fn an_unchanged_signature_does_nothing_and_loads_nothing() {
        let b = backend();
        let mut last = None;
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        let before = b.applied().len();
        reapply_if_changed(&b, &mut last, || panic!("must not load the file"));
        assert_eq!(b.applied().len(), before);
    }

    #[test]
    fn a_plugged_in_pointer_gets_the_saved_values_again() {
        let b = backend();
        let mut last = None;
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        b.set_signature("9:new mouse");
        reapply_if_changed(&b, &mut last, || Some(InputSettings::default()));
        assert_eq!(b.applied().len(), 20);
        assert_eq!(b.refresh_count(), 2);
    }

    #[test]
    fn no_saved_file_means_nothing_is_applied() {
        let b = backend();
        let mut last = None;
        assert!(reapply_if_changed(&b, &mut last, || None).is_empty());
        assert!(b.applied().is_empty());
    }
}
```

Update `src/lib.rs` to:

```rust
pub mod apply;
pub mod backend;
pub mod change;
pub mod commands;
pub mod error;
pub mod fake;
pub mod model;
pub mod parse;
pub mod session;
pub mod store;

pub use error::InputError;
pub use model::*;
```

(keep the crate doc comment at the top.)

- [ ] **Step 5: Run the tests**

Run: `cargo test -p opusdm-input-config`
Expected: PASS. `Session.backend` is private; the tests are in the same module so they can read it.

- [ ] **Step 6: Commit**

```bash
git add crates/input-config/src
git commit -m "feat(input): backend, session and the apply helpers"
```

---

### Task 5: Hub integration (apply at start-up, hot plug)

**Files:**
- Create: `crates/hub/src/input_apply.rs`
- Modify: `crates/hub/Cargo.toml`, `crates/hub/src/main.rs`

**Interfaces:**
- Consumes: `opusdm_input_config::{apply::reapply_if_changed, backend::XBackend, store}` (Task 4).
- Produces: `input_apply::start()` (spawns the thread); `input_apply::tick(backend, last, path) -> Vec<InputError>`.

- [ ] **Step 1: Add the dependency**

In `crates/hub/Cargo.toml` `[dependencies]` add:

```toml
opusdm-input-config = { path = "../input-config" }
```

- [ ] **Step 2: Write `input_apply.rs` with its test**

The hub binary has its modules declared in `main.rs` (`mod background;` and so on), so declare `mod input_apply;` there.

```rust
//! Applies the saved input settings (`input.json`) when the hub starts and
//! again whenever a pointer is plugged in or removed.

use std::path::Path;
use std::time::Duration;

use opusdm_input_config::apply::reapply_if_changed;
use opusdm_input_config::backend::{Backend, XBackend};
use opusdm_input_config::{store, InputError};

/// Seconds between two looks at the pointer list.
const POLL_SECS: u64 = 3;

/// One look: reapply the file at `path` when the pointers changed.
pub(crate) fn tick(backend: &impl Backend, last: &mut Option<String>, path: &Path) -> Vec<InputError> {
    reapply_if_changed(backend, last, || store::load_from(path))
}

/// Start the thread that keeps the saved input settings applied. A failure
/// is logged and never stops the hub.
pub(crate) fn start() {
    let spawned = std::thread::Builder::new().name("input-apply".to_string()).spawn(|| {
        let backend = XBackend::new();
        let path = store::input_path();
        let mut last = None;
        loop {
            for e in tick(&backend, &mut last, &path) {
                opusdm_ipc::err_log!("opusdm-hub", "input settings: {e}");
            }
            std::thread::sleep(Duration::from_secs(POLL_SECS));
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start the input settings thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_input_config::fake::FakeBackend;
    use opusdm_input_config::InputSettings;

    #[test]
    fn tick_applies_the_saved_file_and_ignores_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        let b = FakeBackend::new(InputSettings::default());

        let mut last = None;
        assert!(tick(&b, &mut last, &path).is_empty());
        assert!(b.applied().is_empty());

        store::save_to(&path, &InputSettings::default()).unwrap();
        b.set_signature("plugged");
        tick(&b, &mut last, &path);
        assert_eq!(b.applied().len(), 10);
    }
}
```

In `crates/hub/src/main.rs` add `mod input_apply;` with the other `mod` lines and, right after the `seed_default_filetypes` block (before `EmitScriptEvent(Startup)`), add:

```rust
    // Keep the saved mouse, touchpad and keyboard settings applied.
    input_apply::start();
```

- [ ] **Step 3: Run the tests and build**

Run: `cargo test -p opusdm-hub input_apply && cargo build -p opusdm-hub`
Expected: PASS and a clean build.

- [ ] **Step 4: Commit**

```bash
git add crates/hub Cargo.lock
git commit -m "feat(hub): reapply the saved input settings at start-up and on hot plug"
```

---

### Task 6: The window

**Files:**
- Create: `crates/input-config/src/ui.rs`
- Modify: `crates/input-config/src/main.rs`

**Interfaces:**
- Consumes: the whole library (Tasks 1 to 4), `opusdm_ui::{DialogShell, apply_theme_mode, chrome_css, ui_look_css}`.
- Produces: the binary `opusdm-input-config`. `ui::open(app, backend)`, `ui::open_problem(app, message)`.

GTK code is checked by compiling and by hand (Task 8); the logic it relies on is tested in Task 4.

- [ ] **Step 1: Write `main.rs`**

```rust
//! `opusdm-input-config`: mouse, touchpad and keyboard settings. Every change
//! is applied at once; Save keeps them, closing the window any other way puts
//! the previous values back. See
//! `docs/superpowers/specs/2026-10-06-input-config-design.md`.

mod ui;

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_input_config::backend::{session_problem, XBackend};

/// Load the shared OpusDM look (theme mode, fonts).
fn apply_theme() {
    let settings = opusdm_ipc::settings::load_settings("opusdm-input-config");
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    let css = format!("{} {}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings));
    provider.load_from_data(&css);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.InputConfig").build();
    app.connect_activate(|app| {
        // A second launch only raises the open window.
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        apply_theme();
        match session_problem() {
            Some(problem) => ui::open_problem(app, &problem),
            None => ui::open(app, XBackend::new()),
        }
    });
    app.run()
}
```

- [ ] **Step 2: Write `ui.rs`**

```rust
//! The Input window: Mouse, Touchpad and Keyboard tabs, live apply, Save and Cancel.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_input_config::backend::{Backend, GroupStatus, XBackend};
use opusdm_input_config::parse::{load_xkb_lists, XkbLists};
use opusdm_input_config::session::Session;
use opusdm_input_config::{
    store, InputError, InputSettings, LayoutEntry, MAX_LAYOUTS, REPEAT_DELAY_MAX_MS, REPEAT_DELAY_MIN_MS, REPEAT_RATE_MAX, REPEAT_RATE_MIN,
    SPEED_MAX, SPEED_MIN,
};

/// Wait this long after the last slider move before applying.
const DEBOUNCE_MS: u64 = 30;
/// Default window size.
const WINDOW_W: i32 = 520;
const WINDOW_H: i32 = 460;
/// Gap between the widgets of a page.
const PAGE_SPACING: i32 = 10;
/// Padding of a page.
const PAGE_MARGIN: i32 = 12;

/// An edit of the settings, applied to a copy of the current ones.
type Edit = Box<dyn FnOnce(&mut InputSettings)>;

/// What every callback of the window shares.
struct State {
    session: RefCell<Session<XBackend>>,
    status: gtk::Label,
    pending: RefCell<Option<Edit>>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl State {
    /// Show the errors of the last apply in the status line (clear it when none).
    fn show_errors(&self, errors: &[InputError]) {
        self.status.set_text(&errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n"));
    }

    /// Apply `edit` to a copy of the current settings, live.
    fn update(&self, edit: impl FnOnce(&mut InputSettings)) {
        let mut next = self.session.borrow().current().clone();
        edit(&mut next);
        let errors = self.session.borrow_mut().change(next);
        self.show_errors(&errors);
    }

    /// Like `update`, after a short pause: a slider drag sends many values.
    fn update_debounced(self: &Rc<Self>, edit: Edit) {
        *self.pending.borrow_mut() = Some(edit);
        if let Some(id) = self.timer.borrow_mut().take() { id.remove(); }
        let this = self.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(DEBOUNCE_MS), move || {
            this.timer.borrow_mut().take();
            this.flush();
        });
        *self.timer.borrow_mut() = Some(id);
    }

    /// Apply the waiting edit now (Save calls this so the last move is not lost).
    fn flush(&self) {
        if let Some(id) = self.timer.borrow_mut().take() { id.remove(); }
        let edit = self.pending.borrow_mut().take();
        if let Some(edit) = edit { self.update(edit); }
    }

    /// Drop the waiting edit and put the snapshot back (window closed without Save).
    fn discard_and_revert(&self) {
        if let Some(id) = self.timer.borrow_mut().take() { id.remove(); }
        self.pending.borrow_mut().take();
        let errors = self.session.borrow_mut().revert();
        for e in errors {
            opusdm_ipc::err_log!("opusdm-input-config", "revert: {e}");
        }
    }
}

/// A label, a slider and the value, in one row. `on_change` gets the new value.
fn speed_row(label: &str, initial: f64, on_change: impl Fn(f64) + 'static) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);
    let name = gtk::Label::new(Some(label));
    name.set_width_chars(16);
    name.set_xalign(0.0);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, SPEED_MIN, SPEED_MAX, 0.05);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_digits(2);
    scale.set_value(initial);
    scale.connect_value_changed(move |s| on_change(s.value()));
    row.append(&name);
    row.append(&scale);
    row
}

/// A check button that reports its state.
fn check_row(label: &str, initial: bool, on_toggle: impl Fn(bool) + 'static) -> gtk::CheckButton {
    let check = gtk::CheckButton::with_label(label);
    check.set_active(initial);
    check.connect_toggled(move |c| on_toggle(c.is_active()));
    check
}

/// A vertical page. A group that cannot be used shows its reason and is greyed out.
fn page(status: &GroupStatus) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, PAGE_SPACING);
    page.set_margin_top(PAGE_MARGIN);
    page.set_margin_bottom(PAGE_MARGIN);
    page.set_margin_start(PAGE_MARGIN);
    page.set_margin_end(PAGE_MARGIN);
    if !status.available {
        let reason = gtk::Label::new(Some(&status.reason));
        reason.set_wrap(true);
        reason.set_xalign(0.0);
        page.append(&reason);
    }
    page
}

/// Grey out `page` when its group cannot be used.
fn finish_page(page: &gtk::Box, status: &GroupStatus) {
    if !status.available {
        for child in std::iter::successors(page.first_child().and_then(|c| c.next_sibling()), |c| c.next_sibling()) {
            child.set_sensitive(false);
        }
    }
}

/// The Mouse tab.
fn mouse_page(state: &Rc<State>, status: &GroupStatus) -> gtk::Box {
    let page = page(status);
    let m = state.session.borrow().current().mouse.clone();
    let s = state.clone();
    page.append(&speed_row("Pointer speed", m.speed, move |v| s.update_debounced(Box::new(move |all| all.mouse.speed = v))));
    let s = state.clone();
    page.append(&check_row("Flat acceleration (no acceleration curve)", m.flat_profile, move |v| s.update(|all| all.mouse.flat_profile = v)));
    let s = state.clone();
    page.append(&check_row("Left handed (swap the buttons)", m.left_handed, move |v| s.update(|all| all.mouse.left_handed = v)));
    let s = state.clone();
    page.append(&check_row("Natural scrolling", m.natural_scroll, move |v| s.update(|all| all.mouse.natural_scroll = v)));
    finish_page(&page, status);
    page
}

/// The Touchpad tab.
fn touchpad_page(state: &Rc<State>, status: &GroupStatus) -> gtk::Box {
    let page = page(status);
    let t = state.session.borrow().current().touchpad.clone();
    let s = state.clone();
    page.append(&speed_row("Pointer speed", t.speed, move |v| s.update_debounced(Box::new(move |all| all.touchpad.speed = v))));
    let s = state.clone();
    page.append(&check_row("Tap to click", t.tap_to_click, move |v| s.update(|all| all.touchpad.tap_to_click = v)));
    let s = state.clone();
    page.append(&check_row("Natural scrolling", t.natural_scroll, move |v| s.update(|all| all.touchpad.natural_scroll = v)));
    let s = state.clone();
    page.append(&check_row("Disable while typing", t.disable_while_typing, move |v| s.update(|all| all.touchpad.disable_while_typing = v)));
    finish_page(&page, status);
    page
}

/// A searchable drop-down over `labels`.
fn dropdown(labels: &[String], selected: usize) -> gtk::DropDown {
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let dd = gtk::DropDown::from_strings(&refs);
    dd.set_enable_search(true);
    dd.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    dd.set_selected(selected as u32);
    dd
}

/// Fill `rows` with one row per layout: layout, variant and a remove button.
/// Rebuilt after every add, remove or layout change.
fn build_layout_rows(state: &Rc<State>, lists: &Rc<XkbLists>, rows: &gtk::Box) {
    while let Some(child) = rows.first_child() { rows.remove(&child); }
    let layouts = state.session.borrow().current().keyboard.layouts.clone();
    let rebuild = {
        let (state, lists, rows) = (state.clone(), lists.clone(), rows.clone());
        move || {
            let (state, lists, rows) = (state.clone(), lists.clone(), rows.clone());
            // Not inside the signal handler of a widget that is about to be removed.
            glib::idle_add_local_once(move || build_layout_rows(&state, &lists, &rows));
        }
    };
    for (i, entry) in layouts.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);

        let layout_labels: Vec<String> = lists.layouts.iter().map(|l| format!("{} - {}", l.code, l.description)).collect();
        let layout_pos = lists.layouts.iter().position(|l| l.code == entry.layout).unwrap_or(0);
        let layout_dd = dropdown(&layout_labels, layout_pos);
        layout_dd.set_hexpand(true);

        let variants: Vec<(String, String)> = lists.variants.iter().filter(|v| v.layout == entry.layout).map(|v| (v.code.clone(), v.description.clone())).collect();
        let mut variant_labels = vec!["(default)".to_string()];
        variant_labels.extend(variants.iter().map(|(c, d)| format!("{c} - {d}")));
        let variant_pos = variants.iter().position(|(c, _)| *c == entry.variant).map(|p| p + 1).unwrap_or(0);
        let variant_dd = dropdown(&variant_labels, variant_pos);
        variant_dd.set_hexpand(true);

        {
            let (state, lists, rebuild) = (state.clone(), lists.clone(), rebuild.clone());
            layout_dd.connect_selected_notify(move |dd| {
                let Some(item) = lists.layouts.get(dd.selected() as usize) else { return };
                let code = item.code.clone();
                state.update(|all| {
                    if let Some(l) = all.keyboard.layouts.get_mut(i) { *l = LayoutEntry { layout: code, variant: String::new() }; }
                });
                rebuild();
            });
        }
        {
            let state = state.clone();
            let variants = variants.clone();
            variant_dd.connect_selected_notify(move |dd| {
                let variant = match dd.selected() { 0 => String::new(), n => variants.get(n as usize - 1).map(|(c, _)| c.clone()).unwrap_or_default() };
                state.update(|all| {
                    if let Some(l) = all.keyboard.layouts.get_mut(i) { l.variant = variant; }
                });
            });
        }

        let remove = gtk::Button::with_label("Remove");
        remove.set_sensitive(layouts.len() > 1);
        {
            let (state, rebuild) = (state.clone(), rebuild.clone());
            remove.connect_clicked(move |_| {
                state.update(|all| {
                    if i < all.keyboard.layouts.len() { all.keyboard.layouts.remove(i); }
                });
                rebuild();
            });
        }

        row.append(&layout_dd);
        row.append(&variant_dd);
        row.append(&remove);
        rows.append(&row);
    }
}

/// The Keyboard tab.
fn keyboard_page(state: &Rc<State>, status: &GroupStatus, lists: Rc<XkbLists>) -> gtk::Box {
    let page = page(status);
    let k = state.session.borrow().current().keyboard.clone();

    let rows = gtk::Box::new(gtk::Orientation::Vertical, PAGE_SPACING);
    build_layout_rows(state, &lists, &rows);
    page.append(&rows);

    let add = gtk::Button::with_label("Add layout");
    add.set_halign(gtk::Align::Start);
    {
        let (state, lists, rows) = (state.clone(), lists.clone(), rows.clone());
        add.connect_clicked(move |_| {
            if state.session.borrow().current().keyboard.layouts.len() >= MAX_LAYOUTS { return; }
            state.update(|all| all.keyboard.layouts.push(LayoutEntry { layout: "us".to_string(), variant: String::new() }));
            build_layout_rows(&state, &lists, &rows);
        });
    }
    page.append(&add);

    // The switch key: "None" first, then the grp: options; the current one is
    // kept in the list even when evdev.lst does not know it.
    let mut codes = vec![String::new()];
    let mut labels = vec!["None".to_string()];
    for o in &lists.switch_options {
        codes.push(o.code.clone());
        labels.push(o.description.clone());
    }
    if !k.switch_option.is_empty() && !codes.contains(&k.switch_option) {
        codes.push(k.switch_option.clone());
        labels.push(k.switch_option.clone());
    }
    let selected = codes.iter().position(|c| *c == k.switch_option).unwrap_or(0);
    let switch_dd = dropdown(&labels, selected);
    switch_dd.set_hexpand(true);
    {
        let state = state.clone();
        switch_dd.connect_selected_notify(move |dd| {
            let code = codes.get(dd.selected() as usize).cloned().unwrap_or_default();
            state.update(|all| all.keyboard.switch_option = code);
        });
    }
    let switch_row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);
    let switch_label = gtk::Label::new(Some("Switch layout with"));
    switch_label.set_width_chars(16);
    switch_label.set_xalign(0.0);
    switch_row.append(&switch_label);
    switch_row.append(&switch_dd);
    page.append(&switch_row);

    page.append(&spin_row("Repeat delay (ms)", k.repeat_delay_ms, REPEAT_DELAY_MIN_MS, REPEAT_DELAY_MAX_MS, 50, {
        let state = state.clone();
        move |v| state.update(|all| all.keyboard.repeat_delay_ms = v)
    }));
    page.append(&spin_row("Repeat rate (per s)", k.repeat_rate, REPEAT_RATE_MIN, REPEAT_RATE_MAX, 1, {
        let state = state.clone();
        move |v| state.update(|all| all.keyboard.repeat_rate = v)
    }));

    finish_page(&page, status);
    page
}

/// A label and a spin button for a whole number.
fn spin_row(label: &str, initial: u32, min: u32, max: u32, step: u32, on_change: impl Fn(u32) + 'static) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);
    let name = gtk::Label::new(Some(label));
    name.set_width_chars(16);
    name.set_xalign(0.0);
    let spin = gtk::SpinButton::with_range(min as f64, max as f64, step as f64);
    spin.set_value(initial as f64);
    spin.connect_value_changed(move |s| on_change(s.value() as u32));
    row.append(&name);
    row.append(&spin);
    row
}

/// Open the Input window on `backend`.
pub fn open(app: &gtk::Application, backend: XBackend) {
    let session = Session::open(backend);
    let status = session.status();
    let status_label = gtk::Label::new(None);
    status_label.set_wrap(true);
    status_label.set_xalign(0.0);
    let state = Rc::new(State { session: RefCell::new(session), status: status_label.clone(), pending: RefCell::new(None), timer: RefCell::new(None) });
    let lists = Rc::new(load_xkb_lists());

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    notebook.append_page(&mouse_page(&state, &status.mouse), Some(&gtk::Label::new(Some("Mouse"))));
    notebook.append_page(&touchpad_page(&state, &status.touchpad), Some(&gtk::Label::new(Some("Touchpad"))));
    notebook.append_page(&keyboard_page(&state, &status.keyboard, lists), Some(&gtk::Label::new(Some("Keyboard"))));

    let shell = opusdm_ui::DialogShell::standalone(app, "Input").modal(false).size(WINDOW_W, WINDOW_H);
    shell.content().append(&notebook);
    shell.content().append(&status_label);
    let cancel = shell.close_button("Cancel");
    let save = shell.button("Save");
    let _ = cancel;
    let window = shell.window().clone();

    // Any way of closing the window except Save puts the old values back.
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            state.discard_and_revert();
            glib::Propagation::Proceed
        });
    }
    {
        let (state, window) = (state.clone(), window.clone());
        save.connect_clicked(move |_| {
            state.flush();
            let result = state.session.borrow_mut().commit(&store::input_path());
            match result {
                Ok(()) => window.close(),
                Err(e) => state.show_errors(&[e]),
            }
        });
    }
    {
        let window_for_key = window.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, k, _, _| {
            if k != gtk::gdk::Key::Escape { return glib::Propagation::Proceed; }
            window_for_key.close();
            glib::Propagation::Stop
        });
        window.add_controller(key);
    }
    shell.present();
}

/// A window that only explains why the settings cannot be edited.
pub fn open_problem(app: &gtk::Application, message: &str) {
    let shell = opusdm_ui::DialogShell::standalone(app, "Input").modal(false).size(WINDOW_W, 120);
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    shell.content().append(&label);
    shell.close_button("Close");
    shell.present();
}
```

- [ ] **Step 3: Build and fix compile errors**

Run: `cargo build -p opusdm-input-config`
Expected: a clean build. The code above was written against gtk4-rs 0.10 without being compiled; fix API mismatches (for example `DropDown::set_expression`, `glib::Propagation`, `Scale::with_range`) by reading the compiler message and the existing usage in `crates/ui/src/reference_picker.rs` and `crates/runner/src/ui.rs`. Do not change behaviour while fixing. Unused import warnings (`Backend`) must be removed.

- [ ] **Step 4: Run all tests**

Run: `cargo test -p opusdm-input-config`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/input-config/src
git commit -m "feat(input): the Input window with live apply, Save and revert on close"
```

---

### Task 7: Internal command and desktop menu entry

**Files:**
- Modify: `crates/ipc/src/function.rs`, `crates/lister/src/function_editor.rs`, `crates/hub/src/children.rs`, `crates/hub/src/functions.rs`, `crates/hub/src/desktop_menu.rs`

**Interfaces:**
- Consumes: the `InternalCmd` enum and its tests (`crates/ipc/src/function.rs`).
- Produces: `InternalCmd::OpenInputConfig` (label `Input Settings`, CLI word `input-settings`, no parameters); `children::sibling_bin_in(dir: Option<&Path>, name: &str) -> PathBuf`, `children::spawn_tool(name: &str) -> std::io::Result<()>`.

- [ ] **Step 1: Write the failing tests**

In `crates/ipc/src/function.rs`, in the test `hide_commands_have_labels_and_no_parameters`, add `(InternalCmd::OpenInputConfig, "Input Settings"),` to the list. In `crates/ipc/src/cli.rs`, in `commands_are_found_by_name_label_and_alias`, add:

```rust
        assert_eq!(find_command("input-settings"), Some(InternalCmd::OpenInputConfig));
        assert_eq!(find_command("Input Settings"), Some(InternalCmd::OpenInputConfig));
```

In `crates/hub/src/children.rs` add (inside its existing `#[cfg(test)] mod tests`, or create one if none):

```rust
    #[test]
    fn a_tool_is_looked_up_next_to_the_hub_then_on_path() {
        let dir = std::path::Path::new("/opt/opusdm/bin");
        assert_eq!(sibling_bin_in(Some(dir), "opusdm-input-config"), dir.join("opusdm-input-config"));
        assert_eq!(sibling_bin_in(None, "opusdm-input-config"), std::path::PathBuf::from("opusdm-input-config"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-ipc -p opusdm-hub`
Expected: FAIL (compile errors: `OpenInputConfig`, `sibling_bin_in` missing).

- [ ] **Step 3: Add the command**

In `crates/ipc/src/function.rs`:

1. Enum: after `DesktopNext,` add

```rust
    /// Open the Input window (mouse, touchpad, keyboard settings).
    OpenInputConfig,
```

2. `ALL`: change the length `24` to `25` and add `InternalCmd::OpenInputConfig,` after `InternalCmd::DesktopNext,`.
3. `label`: add `InternalCmd::OpenInputConfig => "Input Settings",` after the `DesktopNext` arm.
4. Parameter schema (`params_for`): add `| InternalCmd::OpenInputConfig` to the arm that returns `&[]` (the one ending `| InternalCmd::DesktopNext => &[]`).
5. `crates/lister/src/function_editor.rs`: in the array that lists the internal commands (`InternalCmd::DesktopPrev, InternalCmd::DesktopNext, ...`, around line 620) add `InternalCmd::OpenInputConfig,` after `InternalCmd::DesktopNext,`, and if that array has a fixed length in its type, increase it by one.

Then run `cargo build --workspace` and fix every other non-exhaustive `match` the compiler reports for `InternalCmd` the same way (the compiler lists them).

In `crates/hub/src/children.rs` add:

```rust
/// Path of the tool `name`: next to `dir` (the hub's own folder) when known,
/// otherwise the bare name for a `PATH` lookup.
pub fn sibling_bin_in(dir: Option<&Path>, name: &str) -> PathBuf {
    match dir {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Start the OpusDM tool `name` (a sibling of the running hub), detached.
pub fn spawn_tool(name: &str) -> std::io::Result<()> {
    let exe = std::env::current_exe().ok();
    let bin = sibling_bin_in(exe.as_deref().and_then(Path::parent), name);
    std::process::Command::new(bin).spawn().map(|_| ())
}
```

(add `use std::path::{Path, PathBuf};` if not already imported.)

In `crates/hub/src/functions.rs`:

- In the dispatch `match *cmd` add `InternalCmd::OpenInputConfig => run_open_input_config(run_id, step_idx, events_tx),`.
- Add next to `run_desktop`:

```rust
/// Run an `OpenInputConfig` step: start the Input window. It needs no
/// selection and no parameters.
fn run_open_input_config(run_id: u64, step_idx: usize, events_tx: &Sender<(u64, FnEvent)>) -> Result<(), String> {
    crate::children::spawn_tool("opusdm-input-config").map_err(|e| format!("cannot start opusdm-input-config: {e}"))?;
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: InternalCmd::OpenInputConfig.label().to_string() }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}
```

In `crates/hub/src/desktop_menu.rs`, right after the "Settings" button block (the one calling `launch_theme_config`), add:

```rust
        // Input: mouse, touchpad and keyboard settings.
        {
            let menu_ref = menu.clone();
            let btn = menu_button("Input");
            btn.connect_clicked(move |_| {
                menu_ref.set_visible(false);
                if let Err(e) = opusdm_hub::children::spawn_tool("opusdm-input-config") {
                    opusdm_ipc::err_log!("opusdm-hub", "cannot start opusdm-input-config: {e}");
                }
            });
            sub.append(&btn);
        }
```

The desktop menu is taller by one button: check `DESKTOP_MENU_H` (300) against the real height when you test under Xephyr in Task 8, and raise it only if the menu is cut.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-ipc -p opusdm-hub -p opusdm-lister`
Expected: PASS. If a test counts the commands (`ALL.len()`), update the number to 25.

- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat(ipc,hub): OpenInputConfig internal command and a Desktop menu entry"
```

---

### Task 8: Packaging, docs, change log, manual check

**Files:**
- Modify: `scripts/build-release.sh`, `docs/usage.md`, `CHANGES.md`

- [ ] **Step 1: Ship the binary**

In `scripts/build-release.sh` add `opusdm-input-config` to `BINARIES` and to the layout comment at the top (`bin/` list). `run-x11.sh` already puts `target/release` on `PATH`, so it needs no change.

- [ ] **Step 2: Document it**

Run `grep -n "^### 3.2" docs/usage.md` and add a section `### 3.29 Input Settings (opusdm-input-config)` after the last 3.2x section. Content: what the tool does (mouse, touchpad, keyboard tabs with their options), that changes apply at once, Save keeps them in `~/.config/opusdm/input.json`, closing any other way restores the previous values, how to open it (Desktop menu > Input, the `Input Settings` internal command, `opusdm-input-config`), that the hub reapplies the file at start and when a pointer is plugged in, the limits (X11 only, nothing is restored if the process is killed, a touchpad tab needs a touchpad), the required tools (`xinput`, `setxkbmap`, `xset`; packages `xinput`, `x11-xkb-utils`, `x11-xserver-utils`).

In `CHANGES.md`, at the top of the `## 2026-10-06` list add one bullet: `- Feature (input-config, hub, ipc): **input settings**, a new `opusdm-input-config` binary (crate `crates/input-config`) ...` summarising the tool, the library split (model, store, parsers, commands, `Backend`, `Session`), the hub thread (`input_apply.rs`, 3 s poll), the `OpenInputConfig` internal command, the Desktop menu entry, the tests, and what was checked by hand. Use only `-`, never the em dash. Reference the spec and the usage section.

- [ ] **Step 3: Full test run and release build**

Run: `cargo test --workspace && cargo build --release -p opusdm-input-config`
Expected: all PASS, release build OK.

- [ ] **Step 4: Check by hand under Xephyr (keyboard and window logic)**

Start `./run-x11.sh` (nested X server) or `Xephyr :2 -screen 1280x800 &` then `DISPLAY=:2 openbox &`. Under Xephyr there are no libinput pointers, so the Mouse and Touchpad tabs must show their reason text and be greyed out (this is the "missing group" check). With `DISPLAY=:2 target/release/opusdm-input-config`:

1. Add a layout (for example `it` then `us`), pick a switch key. Verify with `DISPLAY=:2 setxkbmap -query` that layouts and options changed immediately.
2. Set the repeat rate, verify with `DISPLAY=:2 xset q`.
3. Press Cancel, the X button and Esc in three separate runs: `setxkbmap -query` and `xset q` must show the original values again each time.
4. Change something and press Save: the window closes, the values stay, and `~/.config/opusdm/input.json` exists with them. Reopen: the window shows them.
5. With the hub running in Xephyr and `input.json` present, change the layout from a terminal, then restart the hub: the saved layout is applied.
6. Run `DISPLAY= target/release/opusdm-input-config` (no display): it must fail cleanly (GTK cannot open a display). Run it with `XDG_SESSION_TYPE=wayland` on a real display: the window shows the X11 message.
7. Open the desktop menu: the Desktop submenu shows "Input" and the menu is not cut off (raise `DESKTOP_MENU_H` only if it is).

Record what was checked and what was not in the `CHANGES.md` bullet.

- [ ] **Step 5: Ask the user to check the mouse on the real session**

Mouse and touchpad behaviour needs libinput devices, which Xephyr does not have. After the build, tell the user to run `target/release/opusdm-input-config` on the real session and check: speed slider changes the pointer at once, flat profile, left handed, natural scroll, Cancel and the X button restore the values, Save keeps them after relaunching. Do not run the mouse commands on the real display without the user asking.

- [ ] **Step 6: Commit**

```bash
git add scripts/build-release.sh docs/usage.md CHANGES.md
git commit -m "docs(input): usage section, change log and release packaging"
```
