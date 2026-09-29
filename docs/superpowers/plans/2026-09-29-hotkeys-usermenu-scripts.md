# Hotkeys, User Menu and Script Events Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a saved `Function` be run from a key press (local or X11-global), from a user-editable menu in the lister button row, and from system events (startup, shutdown, lister opened/closed, desktop double clicks).

**Architecture:** The hub is the single owner and dispatcher. It loads, sanitizes, persists and broadcasts one `Bindings` value (hotkeys, menu tree, script events), and is the only place that starts a function when no lister asked for it (a headless run with the sentinel requester `u64::MAX`). Listers keep a cached copy of `Bindings`, look local hotkeys up before the built-in key table, render the menu lazily, and edit their own section of `Bindings` in three editor windows.

**Tech Stack:** Rust 2021 (workspace, rust-version 1.91), GTK4 0.10 (`gtk4`, `gio`, `gdk`), `x11rb` 0.13 for `XGrabKey`, `serde`/`serde_json`, `tempfile` for tests.

**Spec:** `docs/superpowers/specs/2026-09-29-hotkeys-usermenu-scripts-design.md`

## Global Constraints

- A hotkey targets a `Function` only. A user hotkey wins over a built-in binding.
- While a text field has focus (`Focus::Editing`) a user hotkey applies only if it has Ctrl, Alt or Super, or is a function key (`F1`..`F35`).
- Two hotkeys with the same normalized key are an error whatever their `global` flag: the editor blocks Save.
- A global hotkey needs at least one of Ctrl, Alt, Super.
- Global hotkeys use `XGrabKey` (not XRecord/XInput2), X11 only, through a connection created lazily on the first non-empty global set. Wayland is unsupported for global grabs.
- Key string format: `Ctrl+Alt+Shift+Super+<gdk key name>` (fixed modifier order), key = (lower-cased keyval, Ctrl/Alt/Shift/Super flags), NumLock and CapsLock ignored.
- Hub-origin runs (global hotkey, script events, desktop clicks) use the requester sentinel `u64::MAX`; their output goes to the hub log via `err_log!`.
- `SHUTDOWN_HOOK_TIMEOUT` = 10 s, `EVENT_COOLDOWN` = 2 s.
- Script events in this cycle: `Startup`, `Shutdown`, `ListerOpened`, `ListerClosed`, `DesktopDoubleClick`, `DesktopMiddleDoubleClick`, `DesktopRightDoubleClick` are emitted; `Hidden` and `Revealed` are defined but never emitted (F-38).
- Project rules (`CLAUDE.md`): English in code and comments; never write the long dash character (use `-`); constants in `UPPER_SNAKE_CASE`; every function and method has a doc comment (rustdoc on public items); exit conditions at the top of a function as one-liners where possible; text files end with an empty line; follow the surrounding code's style.
- Commits: one per task, message in the imperative, no AI attribution lines, never `--no-verify`, do not push. Each task also appends one bullet to `CHANGES.md`.
- Test commands: `cargo test -p <crate>` and `cargo clippy -p <crate> --all-targets`; do not introduce new clippy warnings.

## Review Focus

- A hand-edited `hotkeys.json` with duplicate or unparsable keys must not crash the hub or shadow a binding: the first entry wins and the rest is logged (Task 1 `sanitize` tests, Task 2 hub test).
- A function deleted while bound must be harmless: the run reports "not found" instead of panicking, the menu drops the entry, editors flag it (Task 1 `prune` and `validate` tests, Task 3 unknown-id test).
- Two runs waiting on the same lister at once (a hotkey pressed twice quickly, or `Startup` plus `ListerOpened`) must both complete, not leak one (Task 3 FIFO test).
- A `ListerOpened` hook that itself opens a lister must be throttled, not loop forever (Task 8 guard tests and cooldown test).
- A plain letter bound as a hotkey must never steal a character typed into a text field (Task 4 focus tests).

---

## File Structure

Create:

- `crates/ipc/src/bindings.rs` - data model, key parser, validators, `sanitize`, `prune`, menu tree operations, persistence.
- `crates/hub/src/global_keys.rs` - X11 grab planning (pure) and the grab thread.
- `crates/hub/src/script_events.rs` - `EventGuard` and timing constants.
- `crates/lister/src/binding_ui.rs` - function drop-down helpers shared by the three editors.
- `crates/lister/src/hotkeys.rs` - lister-side hotkey lookup table and key-event helpers.
- `crates/lister/src/hotkey_editor.rs` - hotkey editor window, key capture, "Find Key" requester, `find_key` logic.
- `crates/lister/src/user_menu.rs` - `gio::Menu` construction, menu actions, lazy popup.
- `crates/lister/src/menu_editor.rs` - user menu editor window.
- `crates/lister/src/scripts_editor.rs` - script events editor window.

Modify:

- `crates/ipc/src/lib.rs`, `crates/ipc/src/proto.rs` - module, messages, `Welcome.bindings`.
- `crates/hub/src/server.rs` - bindings ownership, `RunFunctionById`, headless runs, FIFO selection waiters, global keys wiring, script events, `drop_lister`.
- `crates/hub/src/lib.rs`, `crates/hub/src/main.rs`, `crates/hub/src/toolbar.rs` - modules, options, startup event, desktop events.
- `crates/lister/src/client.rs`, `main.rs`, `build.rs`, `window.rs`, `actions.rs`, `keys.rs`, `palette.rs` - plumbing, key dispatch, menu button, commands.
- `docs/usage.md`, `CHANGES.md`, `TODO.md`.

---

### Task 1: `opusdm_ipc::bindings` data model, key parser, validation, persistence

**Files:**
- Create: `crates/ipc/src/bindings.rs`
- Modify: `crates/ipc/src/lib.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `crate::function::FunctionId` (`String`), `crate::settings::{config_dir, load_json, save_json}`.
- Produces (later tasks rely on these exact names):
  - `pub struct Mods { pub ctrl: bool, pub alt: bool, pub shift: bool, pub sup: bool }` (`Debug, Clone, Copy, PartialEq, Eq, Hash, Default`), `Mods::has_command_modifier(self) -> bool`.
  - `pub struct KeyCombo { pub mods: Mods, pub name: String }`, `KeyCombo::normalized_name(&self) -> String`, `KeyCombo::identity(&self) -> (Mods, String)`.
  - `pub fn parse_key(text: &str) -> Result<KeyCombo, String>`, `pub fn format_key(combo: &KeyCombo) -> String`.
  - `pub struct Hotkey { pub key: String, pub function: FunctionId, pub global: bool }`.
  - `pub enum MenuNode { Entry { label, function }, Submenu { label, children }, Separator }`.
  - `pub enum ScriptEvent` (`Copy, Hash`), `ScriptEvent::{ALL, is_emitted, emitted, label}`, `pub struct ScriptBinding { pub event: ScriptEvent, pub function: FunctionId }`.
  - `pub struct Bindings { pub hotkeys: Vec<Hotkey>, pub menu: Vec<MenuNode>, pub scripts: Vec<ScriptBinding> }`.
  - `pub enum BindingError` with `Display`, `hotkey_index()`, `script_index()`, `menu_path()`.
  - `pub fn validate_hotkeys(&[Hotkey], &HashSet<FunctionId>) -> Vec<BindingError>`, `validate_menu(&[MenuNode], &HashSet<FunctionId>)`, `validate_scripts(&[ScriptBinding], &HashSet<FunctionId>)`.
  - `pub fn sanitize(Bindings) -> Bindings`, `pub fn prune(&[MenuNode], &HashSet<FunctionId>) -> Vec<MenuNode>`.
  - `pub fn load_bindings() -> Bindings`, `load_bindings_from(&Path) -> Bindings`, `save_bindings(&Bindings) -> io::Result<()>`, `save_bindings_to(&Path, &Bindings) -> io::Result<()>`.

Design note (a clarification of the spec, not a change of behaviour): editors validate only their own section (`validate_hotkeys`, `validate_menu`, `validate_scripts`), so a dangling function id in one section never blocks saving another. The hub does not reject `SetBindings`; it applies `sanitize`, which drops what can never work (unparsable or duplicate hotkeys, global hotkeys without a modifier, repeated script pairs), keeping the first entry and logging each removal. Dangling function ids are kept and resolved at run time.

- [ ] **Step 1: Write the failing tests**

Create `crates/ipc/src/bindings.rs` containing only the test module below (the implementation follows in Step 3). Put it at the bottom of the file once Step 3 exists; for now the file holds just this so the compile failure is the expected failure.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn known(ids: &[&str]) -> HashSet<FunctionId> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn hk(key: &str, function: &str, global: bool) -> Hotkey {
        Hotkey { key: key.into(), function: function.into(), global }
    }

    #[test]
    fn parse_key_reads_modifiers_and_name() {
        let combo = parse_key("Ctrl+Alt+u").unwrap();
        assert_eq!(combo.mods, Mods { ctrl: true, alt: true, shift: false, sup: false });
        assert_eq!(combo.name, "u");
    }

    #[test]
    fn parse_key_accepts_a_bare_key_and_lowercase_modifiers() {
        let bare = parse_key("F7").unwrap();
        assert_eq!(bare.mods, Mods::default());
        assert_eq!(bare.name, "F7");
        let shifted = parse_key("ctrl+SHIFT+F7").unwrap();
        assert!(shifted.mods.ctrl && shifted.mods.shift && !shifted.mods.alt);
    }

    #[test]
    fn parse_key_rejects_bad_input() {
        for bad in ["", "   ", "Ctrl+", "Ctrl+Ctrl+u", "Hyper+u", "+"] {
            assert!(parse_key(bad).is_err(), "'{bad}' must not parse");
        }
    }

    #[test]
    fn format_key_uses_the_fixed_modifier_order() {
        let combo = parse_key("Super+Shift+Alt+Ctrl+u").unwrap();
        assert_eq!(format_key(&combo), "Ctrl+Alt+Shift+Super+u");
    }

    #[test]
    fn identity_ignores_the_case_of_a_single_letter_only() {
        let a = parse_key("Ctrl+U").unwrap();
        let b = parse_key("ctrl+u").unwrap();
        assert_eq!(a.identity(), b.identity());
        assert_eq!(format_key(&a), "Ctrl+u");
        let f7 = parse_key("F7").unwrap();
        let lower = parse_key("f7").unwrap();
        assert_ne!(f7.identity(), lower.identity());
    }

    #[test]
    fn command_modifier_excludes_shift_alone() {
        assert!(!parse_key("Shift+u").unwrap().mods.has_command_modifier());
        assert!(parse_key("Alt+u").unwrap().mods.has_command_modifier());
        assert!(parse_key("Super+u").unwrap().mods.has_command_modifier());
    }

    #[test]
    fn validate_hotkeys_flags_a_duplicate_whatever_the_global_flag() {
        let list = [hk("Ctrl+u", "a", false), hk("ctrl+U", "b", true)];
        let errors = validate_hotkeys(&list, &known(&["a", "b"]));
        assert_eq!(errors, vec![BindingError::HotkeyDuplicate { index: 1, first: 0 }]);
    }

    #[test]
    fn validate_hotkeys_flags_invalid_key_unknown_function_and_bare_global() {
        let list = [hk("Ctrl+", "a", false), hk("Ctrl+j", "gone", false), hk("j", "a", true)];
        let errors = validate_hotkeys(&list, &known(&["a"]));
        assert!(errors.contains(&BindingError::HotkeyKeyInvalid { index: 0, reason: "missing key name in 'Ctrl+'".into() }));
        assert!(errors.contains(&BindingError::HotkeyUnknownFunction { index: 1 }));
        assert!(errors.contains(&BindingError::HotkeyGlobalNeedsModifier { index: 2 }));
    }

    #[test]
    fn validate_hotkeys_accepts_a_clean_list() {
        let list = [hk("Ctrl+u", "a", false), hk("Ctrl+Alt+u", "a", true)];
        assert!(validate_hotkeys(&list, &known(&["a"])).is_empty());
    }

    #[test]
    fn validate_menu_reports_paths_for_nested_problems() {
        let menu = vec![
            MenuNode::Entry { label: "ok".into(), function: "a".into() },
            MenuNode::Submenu {
                label: "sub".into(),
                children: vec![
                    MenuNode::Entry { label: " ".into(), function: "a".into() },
                    MenuNode::Entry { label: "x".into(), function: "gone".into() },
                ],
            },
        ];
        let errors = validate_menu(&menu, &known(&["a"]));
        assert_eq!(
            errors,
            vec![
                BindingError::MenuEmptyLabel { path: vec![1, 0] },
                BindingError::MenuUnknownFunction { path: vec![1, 1] },
            ]
        );
    }

    #[test]
    fn validate_scripts_flags_unknown_function_unemitted_event_and_duplicates() {
        let scripts = [
            ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
            ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
            ScriptBinding { event: ScriptEvent::Hidden, function: "a".into() },
            ScriptBinding { event: ScriptEvent::Shutdown, function: "gone".into() },
        ];
        let errors = validate_scripts(&scripts, &known(&["a"]));
        assert!(errors.contains(&BindingError::ScriptDuplicate { index: 1, first: 0 }));
        assert!(errors.contains(&BindingError::ScriptEventNotEmitted { index: 2 }));
        assert!(errors.contains(&BindingError::ScriptUnknownFunction { index: 3 }));
    }

    #[test]
    fn sanitize_keeps_the_first_of_a_duplicate_and_canonicalizes_keys() {
        let dirty = Bindings {
            hotkeys: vec![
                hk("ctrl+U", "a", false),
                hk("Ctrl+u", "b", true),
                hk("nonsense+", "c", false),
                hk("u", "d", true),
            ],
            ..Bindings::default()
        };
        let clean = sanitize(dirty);
        assert_eq!(clean.hotkeys, vec![hk("Ctrl+u", "a", false)]);
    }

    #[test]
    fn sanitize_drops_repeated_script_pairs_but_keeps_dangling_ids() {
        let dirty = Bindings {
            scripts: vec![
                ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
                ScriptBinding { event: ScriptEvent::Startup, function: "a".into() },
                ScriptBinding { event: ScriptEvent::Startup, function: "gone".into() },
            ],
            ..Bindings::default()
        };
        let clean = sanitize(dirty);
        assert_eq!(clean.scripts.len(), 2);
    }

    #[test]
    fn prune_drops_missing_functions_and_empty_submenus() {
        let menu = vec![
            MenuNode::Entry { label: "keep".into(), function: "a".into() },
            MenuNode::Entry { label: "drop".into(), function: "gone".into() },
            MenuNode::Separator,
            MenuNode::Submenu {
                label: "empty after prune".into(),
                children: vec![MenuNode::Entry { label: "x".into(), function: "gone".into() }],
            },
            MenuNode::Submenu {
                label: "kept".into(),
                children: vec![MenuNode::Entry { label: "y".into(), function: "a".into() }],
            },
        ];
        let pruned = prune(&menu, &known(&["a"]));
        assert_eq!(pruned.len(), 3);
        assert!(matches!(&pruned[0], MenuNode::Entry { label, .. } if label == "keep"));
        assert!(matches!(pruned[1], MenuNode::Separator));
        assert!(matches!(&pruned[2], MenuNode::Submenu { label, .. } if label == "kept"));
    }

    #[test]
    fn script_event_and_menu_node_use_the_documented_json_shape() {
        assert_eq!(serde_json::to_string(&ScriptEvent::ListerOpened).unwrap(), "\"lister_opened\"");
        assert_eq!(serde_json::to_string(&MenuNode::Separator).unwrap(), "{\"type\":\"separator\"}");
        let entry: MenuNode = serde_json::from_str("{\"type\":\"entry\",\"label\":\"L\",\"function\":\"f\"}").unwrap();
        assert_eq!(entry, MenuNode::Entry { label: "L".into(), function: "f".into() });
    }

    #[test]
    fn only_the_seven_wired_events_are_emitted() {
        let emitted = ScriptEvent::emitted();
        assert_eq!(emitted.len(), 7);
        assert!(!emitted.contains(&ScriptEvent::Hidden));
        assert!(!emitted.contains(&ScriptEvent::Revealed));
    }

    #[test]
    fn bindings_roundtrip_through_the_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let bindings = Bindings {
            hotkeys: vec![hk("Ctrl+u", "a", false)],
            menu: vec![MenuNode::Entry { label: "L".into(), function: "a".into() }, MenuNode::Separator],
            scripts: vec![ScriptBinding { event: ScriptEvent::Startup, function: "a".into() }],
        };
        save_bindings_to(dir.path(), &bindings).unwrap();
        assert!(dir.path().join("hotkeys.json").is_file());
        assert!(dir.path().join("usermenu.json").is_file());
        assert!(dir.path().join("scripts.json").is_file());
        assert_eq!(load_bindings_from(dir.path()), bindings);
    }

    #[test]
    fn a_missing_or_malformed_file_yields_an_empty_section() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_bindings_from(dir.path()), Bindings::default());
        std::fs::write(dir.path().join("hotkeys.json"), b"{ not json").unwrap();
        std::fs::write(
            dir.path().join("scripts.json"),
            b"[{\"event\":\"startup\",\"function\":\"a\"}]",
        )
        .unwrap();
        let loaded = load_bindings_from(dir.path());
        assert!(loaded.hotkeys.is_empty());
        assert_eq!(loaded.scripts.len(), 1);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc bindings`
Expected: FAIL to compile (`cannot find ... Hotkey`, `parse_key`, ...) because the implementation does not exist yet.

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` module in `crates/ipc/src/bindings.rs`:

```rust
//! Hotkeys, user menu and script-event bindings (F-03, F-04, F-05).
//!
//! Three JSON files under `~/.config/opusdm/` (`hotkeys.json`,
//! `usermenu.json`, `scripts.json`) hold ordinary lists that reference saved
//! functions by [`FunctionId`]. The hub loads them into one [`Bindings`],
//! sanitizes it, persists it and broadcasts it to every lister. See
//! `docs/superpowers/specs/2026-09-29-hotkeys-usermenu-scripts-design.md`.

use crate::function::FunctionId;
use crate::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Modifier keys of a key combination. NumLock and CapsLock are never part
/// of a combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    /// Control.
    pub ctrl: bool,
    /// Alt (X11 Mod1).
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Super (X11 Mod4).
    pub sup: bool,
}

impl Mods {
    /// Whether Ctrl, Alt or Super is held. Shift alone does not count: it
    /// only changes the character a key produces.
    pub fn has_command_modifier(self) -> bool {
        self.ctrl || self.alt || self.sup
    }
}

/// One parsed key combination: modifiers plus the GDK key name (`u`, `F7`,
/// `plus`, ...). GDK key values equal X11 keysyms, so the same name is
/// resolved by the lister and by the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    /// Held modifiers.
    pub mods: Mods,
    /// GDK key name as written.
    pub name: String,
}

impl KeyCombo {
    /// The key name with a single ASCII letter lower-cased and every other
    /// name (`F7`, `plus`, `ISO_Left_Tab`) unchanged.
    pub fn normalized_name(&self) -> String {
        if self.name.len() == 1 && self.name.is_ascii() {
            return self.name.to_ascii_lowercase();
        }
        self.name.clone()
    }

    /// What two combinations are compared by (duplicate detection, lookup).
    pub fn identity(&self) -> (Mods, String) {
        (self.mods, self.normalized_name())
    }
}

/// Parse `Ctrl+Alt+u` style text. Modifiers are case-insensitive and may
/// come in any order; the last `+`-separated part is the key name.
pub fn parse_key(text: &str) -> Result<KeyCombo, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty key".to_string());
    }
    let mut parts: Vec<&str> = text.split('+').collect();
    let name = parts.pop().unwrap_or_default();
    if name.is_empty() {
        return Err(format!("missing key name in '{text}'"));
    }
    let mut mods = Mods::default();
    for part in parts {
        let slot = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => &mut mods.ctrl,
            "alt" => &mut mods.alt,
            "shift" => &mut mods.shift,
            "super" => &mut mods.sup,
            other => return Err(format!("unknown modifier '{other}' in '{text}'")),
        };
        if *slot {
            return Err(format!("repeated modifier '{part}' in '{text}'"));
        }
        *slot = true;
    }
    Ok(KeyCombo { mods, name: name.to_string() })
}

/// The canonical text for `combo`: modifiers in the fixed order Ctrl, Alt,
/// Shift, Super, then the normalized key name.
pub fn format_key(combo: &KeyCombo) -> String {
    let mut out = String::new();
    if combo.mods.ctrl {
        out.push_str("Ctrl+");
    }
    if combo.mods.alt {
        out.push_str("Alt+");
    }
    if combo.mods.shift {
        out.push_str("Shift+");
    }
    if combo.mods.sup {
        out.push_str("Super+");
    }
    out.push_str(&combo.normalized_name());
    out
}

/// One key bound to a function. `key` is the canonical text of a
/// [`KeyCombo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Hotkey {
    /// Canonical key text, e.g. `Ctrl+Alt+u`.
    pub key: String,
    /// The function to run.
    pub function: FunctionId,
    /// Grab the key system-wide (X11 only) instead of only in listers.
    pub global: bool,
}

/// One node of the user menu tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MenuNode {
    /// A leaf that runs a function.
    Entry { label: String, function: FunctionId },
    /// A named group of further nodes.
    Submenu { label: String, children: Vec<MenuNode> },
    /// A visual separator.
    Separator,
}

/// A system event a function can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptEvent {
    /// The hub started.
    Startup,
    /// The hub is quitting.
    Shutdown,
    /// A lister registered with the hub.
    ListerOpened,
    /// A lister went away.
    ListerClosed,
    /// Primary double click on the desktop background.
    DesktopDoubleClick,
    /// Middle double click on the desktop.
    DesktopMiddleDoubleClick,
    /// Secondary (right) double click on the desktop.
    DesktopRightDoubleClick,
    /// The environment was hidden (not emitted until F-38).
    Hidden,
    /// The environment was revealed (not emitted until F-38).
    Revealed,
}

impl ScriptEvent {
    /// Every variant, in editor display order.
    pub const ALL: [ScriptEvent; 9] = [
        ScriptEvent::Startup,
        ScriptEvent::Shutdown,
        ScriptEvent::ListerOpened,
        ScriptEvent::ListerClosed,
        ScriptEvent::DesktopDoubleClick,
        ScriptEvent::DesktopMiddleDoubleClick,
        ScriptEvent::DesktopRightDoubleClick,
        ScriptEvent::Hidden,
        ScriptEvent::Revealed,
    ];

    /// Whether the hub currently emits this event.
    pub fn is_emitted(self) -> bool {
        !matches!(self, ScriptEvent::Hidden | ScriptEvent::Revealed)
    }

    /// The events the hub emits, in display order.
    pub fn emitted() -> Vec<ScriptEvent> {
        Self::ALL.iter().copied().filter(|e| e.is_emitted()).collect()
    }

    /// Human-readable name for the editor.
    pub fn label(self) -> &'static str {
        match self {
            ScriptEvent::Startup => "Startup",
            ScriptEvent::Shutdown => "Shutdown",
            ScriptEvent::ListerOpened => "Lister opened",
            ScriptEvent::ListerClosed => "Lister closed",
            ScriptEvent::DesktopDoubleClick => "Desktop double click",
            ScriptEvent::DesktopMiddleDoubleClick => "Desktop middle double click",
            ScriptEvent::DesktopRightDoubleClick => "Desktop right double click",
            ScriptEvent::Hidden => "Hidden",
            ScriptEvent::Revealed => "Revealed",
        }
    }
}

/// A function bound to an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptBinding {
    /// The event.
    pub event: ScriptEvent,
    /// The function to run when it fires.
    pub function: FunctionId,
}

/// The three binding lists together: wire format, in-memory state, and the
/// content of the three files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Bindings {
    /// `hotkeys.json`.
    pub hotkeys: Vec<Hotkey>,
    /// `usermenu.json`.
    pub menu: Vec<MenuNode>,
    /// `scripts.json`.
    pub scripts: Vec<ScriptBinding>,
}

/// One reason a section of [`Bindings`] cannot be saved as it is. Messages
/// (via `Display`) never contain the row position: the editors show them
/// on the offending row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
    /// The key text does not parse.
    HotkeyKeyInvalid { index: usize, reason: String },
    /// Same normalized key as the earlier hotkey `first`.
    HotkeyDuplicate { index: usize, first: usize },
    /// The function id is not a saved function.
    HotkeyUnknownFunction { index: usize },
    /// A global hotkey without Ctrl, Alt or Super.
    HotkeyGlobalNeedsModifier { index: usize },
    /// A menu label that is empty or blank.
    MenuEmptyLabel { path: Vec<usize> },
    /// A menu entry whose function id is not a saved function.
    MenuUnknownFunction { path: Vec<usize> },
    /// A script binding whose function id is not a saved function.
    ScriptUnknownFunction { index: usize },
    /// A script binding on an event the hub does not emit yet.
    ScriptEventNotEmitted { index: usize },
    /// The same (event, function) pair as the earlier binding `first`.
    ScriptDuplicate { index: usize, first: usize },
}

impl BindingError {
    /// The hotkey row this error belongs to, if it is a hotkey error.
    pub fn hotkey_index(&self) -> Option<usize> {
        match self {
            BindingError::HotkeyKeyInvalid { index, .. }
            | BindingError::HotkeyDuplicate { index, .. }
            | BindingError::HotkeyUnknownFunction { index }
            | BindingError::HotkeyGlobalNeedsModifier { index } => Some(*index),
            _ => None,
        }
    }

    /// The script row this error belongs to, if it is a script error.
    pub fn script_index(&self) -> Option<usize> {
        match self {
            BindingError::ScriptUnknownFunction { index }
            | BindingError::ScriptEventNotEmitted { index }
            | BindingError::ScriptDuplicate { index, .. } => Some(*index),
            _ => None,
        }
    }

    /// The menu node path this error belongs to, if it is a menu error.
    pub fn menu_path(&self) -> Option<&[usize]> {
        match self {
            BindingError::MenuEmptyLabel { path } | BindingError::MenuUnknownFunction { path } => Some(path),
            _ => None,
        }
    }
}

impl fmt::Display for BindingError {
    /// The message shown on the offending editor row.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindingError::HotkeyKeyInvalid { reason, .. } => write!(f, "invalid key: {reason}"),
            BindingError::HotkeyDuplicate { first, .. } => {
                write!(f, "this key is already used by hotkey {}", first + 1)
            }
            BindingError::HotkeyUnknownFunction { .. } => write!(f, "the function no longer exists"),
            BindingError::HotkeyGlobalNeedsModifier { .. } => {
                write!(f, "a global hotkey needs Ctrl, Alt or Super")
            }
            BindingError::MenuEmptyLabel { .. } => write!(f, "the label is empty"),
            BindingError::MenuUnknownFunction { .. } => write!(f, "the function no longer exists"),
            BindingError::ScriptUnknownFunction { .. } => write!(f, "the function no longer exists"),
            BindingError::ScriptEventNotEmitted { .. } => write!(f, "this event is not emitted yet"),
            BindingError::ScriptDuplicate { first, .. } => {
                write!(f, "already bound in row {}", first + 1)
            }
        }
    }
}

/// Every problem in a hotkey list, in row order.
pub fn validate_hotkeys(hotkeys: &[Hotkey], known: &HashSet<FunctionId>) -> Vec<BindingError> {
    let mut errors = Vec::new();
    let mut seen: HashMap<(Mods, String), usize> = HashMap::new();
    for (index, hotkey) in hotkeys.iter().enumerate() {
        match parse_key(&hotkey.key) {
            Err(reason) => errors.push(BindingError::HotkeyKeyInvalid { index, reason }),
            Ok(combo) => {
                match seen.get(&combo.identity()) {
                    Some(&first) => errors.push(BindingError::HotkeyDuplicate { index, first }),
                    None => {
                        seen.insert(combo.identity(), index);
                    }
                }
                if hotkey.global && !combo.mods.has_command_modifier() {
                    errors.push(BindingError::HotkeyGlobalNeedsModifier { index });
                }
            }
        }
        if !known.contains(&hotkey.function) {
            errors.push(BindingError::HotkeyUnknownFunction { index });
        }
    }
    errors
}

/// Every problem in a menu tree, depth first.
pub fn validate_menu(menu: &[MenuNode], known: &HashSet<FunctionId>) -> Vec<BindingError> {
    let mut errors = Vec::new();
    walk_menu(menu, &mut Vec::new(), known, &mut errors);
    errors
}

/// Recursive helper of [`validate_menu`]; `path` is the position of `nodes`.
fn walk_menu(nodes: &[MenuNode], path: &mut Vec<usize>, known: &HashSet<FunctionId>, errors: &mut Vec<BindingError>) {
    for (index, node) in nodes.iter().enumerate() {
        path.push(index);
        match node {
            MenuNode::Entry { label, function } => {
                if label.trim().is_empty() {
                    errors.push(BindingError::MenuEmptyLabel { path: path.clone() });
                }
                if !known.contains(function) {
                    errors.push(BindingError::MenuUnknownFunction { path: path.clone() });
                }
            }
            MenuNode::Submenu { label, children } => {
                if label.trim().is_empty() {
                    errors.push(BindingError::MenuEmptyLabel { path: path.clone() });
                }
                walk_menu(children, path, known, errors);
            }
            MenuNode::Separator => {}
        }
        path.pop();
    }
}

/// Every problem in a script-binding list, in row order.
pub fn validate_scripts(scripts: &[ScriptBinding], known: &HashSet<FunctionId>) -> Vec<BindingError> {
    let mut errors = Vec::new();
    let mut seen: HashMap<(ScriptEvent, &str), usize> = HashMap::new();
    for (index, binding) in scripts.iter().enumerate() {
        match seen.get(&(binding.event, binding.function.as_str())) {
            Some(&first) => errors.push(BindingError::ScriptDuplicate { index, first }),
            None => {
                seen.insert((binding.event, binding.function.as_str()), index);
            }
        }
        if !binding.event.is_emitted() {
            errors.push(BindingError::ScriptEventNotEmitted { index });
        }
        if !known.contains(&binding.function) {
            errors.push(BindingError::ScriptUnknownFunction { index });
        }
    }
    errors
}

/// The hub's tolerant clean-up of a [`Bindings`] it received or loaded: drop
/// what can never work, keep the first of any duplicate, canonicalize hotkey
/// text, and log each removal. Dangling function ids are kept (they are
/// resolved at run time and flagged by the editors).
pub fn sanitize(bindings: Bindings) -> Bindings {
    let mut hotkeys = Vec::new();
    let mut seen: HashSet<(Mods, String)> = HashSet::new();
    for hotkey in bindings.hotkeys {
        let combo = match parse_key(&hotkey.key) {
            Ok(combo) => combo,
            Err(reason) => {
                crate::err_log!("opusdm", "dropping hotkey '{}': {reason}", hotkey.key);
                continue;
            }
        };
        if hotkey.global && !combo.mods.has_command_modifier() {
            crate::err_log!("opusdm", "dropping global hotkey '{}': it needs Ctrl, Alt or Super", hotkey.key);
            continue;
        }
        if !seen.insert(combo.identity()) {
            crate::err_log!("opusdm", "dropping hotkey '{}': the key is already bound", hotkey.key);
            continue;
        }
        hotkeys.push(Hotkey { key: format_key(&combo), ..hotkey });
    }
    let mut scripts: Vec<ScriptBinding> = Vec::new();
    for binding in bindings.scripts {
        if scripts.contains(&binding) {
            crate::err_log!("opusdm", "dropping repeated script binding for {:?}", binding.event);
            continue;
        }
        scripts.push(binding);
    }
    Bindings { hotkeys, menu: bindings.menu, scripts }
}

/// `menu` without entries whose function is not in `known` and without the
/// submenus that end up empty. Separators are kept.
pub fn prune(menu: &[MenuNode], known: &HashSet<FunctionId>) -> Vec<MenuNode> {
    menu.iter()
        .filter_map(|node| match node {
            MenuNode::Entry { function, .. } => known.contains(function).then(|| node.clone()),
            MenuNode::Submenu { label, children } => {
                let kept = prune(children, known);
                (!kept.is_empty()).then(|| MenuNode::Submenu { label: label.clone(), children: kept })
            }
            MenuNode::Separator => Some(MenuNode::Separator),
        })
        .collect()
}

/// Path to `hotkeys.json` under [`config_dir`].
pub fn hotkeys_path() -> PathBuf {
    config_dir().join("hotkeys.json")
}

/// Path to `usermenu.json` under [`config_dir`].
pub fn usermenu_path() -> PathBuf {
    config_dir().join("usermenu.json")
}

/// Path to `scripts.json` under [`config_dir`].
pub fn scripts_path() -> PathBuf {
    config_dir().join("scripts.json")
}

/// Load the three files from the standard config directory. A missing or
/// malformed file yields an empty section.
pub fn load_bindings() -> Bindings {
    load_bindings_from(&config_dir())
}

/// Testable core of [`load_bindings`].
pub fn load_bindings_from(dir: &Path) -> Bindings {
    Bindings {
        hotkeys: load_json(&dir.join("hotkeys.json"), "opusdm", "hotkeys"),
        menu: load_json(&dir.join("usermenu.json"), "opusdm", "user menu"),
        scripts: load_json(&dir.join("scripts.json"), "opusdm", "script events"),
    }
}

/// Save the three files under the standard config directory.
pub fn save_bindings(bindings: &Bindings) -> io::Result<()> {
    save_bindings_to(&config_dir(), bindings)
}

/// Testable core of [`save_bindings`].
pub fn save_bindings_to(dir: &Path, bindings: &Bindings) -> io::Result<()> {
    save_json(&dir.join("hotkeys.json"), &bindings.hotkeys)?;
    save_json(&dir.join("usermenu.json"), &bindings.menu)?;
    save_json(&dir.join("scripts.json"), &bindings.scripts)
}
```

- [ ] **Step 4: Register the module**

In `crates/ipc/src/lib.rs` add `pub mod bindings;` (alphabetical, before `pub mod desktop;`) and, after the existing `pub use` lines, `pub use bindings::Bindings;`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p opusdm-ipc bindings`
Expected: PASS (all tests in `bindings::tests`). Then `cargo clippy -p opusdm-ipc --all-targets` shows no new warning.

- [ ] **Step 6: Commit**

```bash
printf -- '- Feature (F-03/F-04/F-05, step 1): new `opusdm_ipc::bindings` module - hotkey/menu/script-event data model, `Ctrl+Alt+u` key parser, per-section validators, tolerant `sanitize`, `prune`, and persistence to `hotkeys.json`, `usermenu.json`, `scripts.json`.\n' >> CHANGES.md
git add crates/ipc/src/bindings.rs crates/ipc/src/lib.rs CHANGES.md
git commit -m "feat(ipc): add bindings data model, key parser, validation and persistence"
```

---

### Task 2: Protocol messages, hub ownership of `Bindings`, lister cache

**Files:**
- Modify: `crates/ipc/src/proto.rs`
- Modify: `crates/hub/src/server.rs`, `crates/hub/src/main.rs`
- Modify: `crates/lister/src/client.rs`, `crates/lister/src/main.rs`, `crates/lister/src/build.rs`, `crates/lister/src/window.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: everything Task 1 produces.
- Produces:
  - `ToHub::SetBindings(Bindings)`, `ToLister::BindingsUpdate(Bindings)`, `Welcome.bindings: Bindings` (`#[serde(default)]`).
  - `pub struct ServerOptions { pub initial_bindings: Option<Bindings> }` with `Default` (empty bindings, no disk read) and `ServerOptions::production()` (`initial_bindings: None`), `pub fn spawn_server_with(listener: UnixListener, on_spawn: SpawnFn, opts: ServerOptions) -> ServerHandle`. `spawn_server(listener, on_spawn)` keeps its signature and uses `ServerOptions::default()`.
  - `Client::connect(...) -> io::Result<(Client, ListerId, Roles, Settings, Bindings)>`.
  - `HubConnection.bindings: RefCell<Bindings>` in the lister.

- [ ] **Step 1: Write the failing tests**

In `crates/ipc/src/proto.rs` tests, extend `every_tohub_variant_roundtrips` (add after the `SelectionReply` line):

```rust
        roundtrip_tohub(ToHub::SetBindings(crate::bindings::Bindings {
            hotkeys: vec![crate::bindings::Hotkey { key: "Ctrl+u".into(), function: "f".into(), global: true }],
            ..crate::bindings::Bindings::default()
        }));
```

and `every_tolister_variant_roundtrips` (add at the end of that test body):

```rust
        roundtrip_tolister(ToLister::BindingsUpdate(crate::bindings::Bindings::default()));
```

and add `bindings: crate::bindings::Bindings::default(),` to the `Welcome` literal in `welcome_roundtrips`.

In `crates/hub/src/server.rs` tests add (imports `Bindings`, `Hotkey` come from `opusdm_ipc::bindings`):

```rust
    #[test]
    fn set_bindings_sanitizes_persists_and_broadcasts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.bindings, opusdm_ipc::Bindings::default());

        let dirty = opusdm_ipc::Bindings {
            hotkeys: vec![
                opusdm_ipc::bindings::Hotkey { key: "ctrl+U".into(), function: "f1".into(), global: false },
                opusdm_ipc::bindings::Hotkey { key: "Ctrl+u".into(), function: "f2".into(), global: false },
            ],
            ..opusdm_ipc::Bindings::default()
        };
        send(&mut a, &ToHub::SetBindings(dirty)).unwrap();

        let got: ToLister = recv(&mut a).unwrap().unwrap();
        let ToLister::BindingsUpdate(b) = got else { panic!("expected BindingsUpdate, got {got:?}") };
        assert_eq!(b.hotkeys.len(), 1, "the duplicate must be dropped, first wins");
        assert_eq!(b.hotkeys[0].key, "Ctrl+u");
        assert_eq!(b.hotkeys[0].function, "f1");

        let written = wait_for_file(&cfg.path().join("opusdm").join("hotkeys.json"));
        let back: Vec<opusdm_ipc::bindings::Hotkey> = serde_json::from_str(&written).unwrap();
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn welcome_carries_the_bindings_loaded_from_disk() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let on_disk = opusdm_ipc::Bindings {
            hotkeys: vec![opusdm_ipc::bindings::Hotkey { key: "Ctrl+j".into(), function: "f".into(), global: false }],
            ..opusdm_ipc::Bindings::default()
        };
        opusdm_ipc::bindings::save_bindings(&on_disk).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), ServerOptions::production());

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.bindings, on_disk);
    }
```

In `crates/lister/src/client.rs` tests: add `bindings: opusdm_ipc::Bindings::default(),` to both `Welcome` literals, change both destructures of `Client::connect(...)` to a 5-tuple (`let (mut client, id, roles, _settings, _bindings) = ...`), and add:

```rust
    #[test]
    fn connect_returns_the_bindings_from_welcome() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let sent = opusdm_ipc::Bindings {
            hotkeys: vec![opusdm_ipc::bindings::Hotkey { key: "Ctrl+u".into(), function: "f".into(), global: false }],
            ..opusdm_ipc::Bindings::default()
        };
        let to_send = sent.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _got: ToHub = recv(&mut s).unwrap().unwrap();
            send(&mut s, &Welcome {
                lister_id: ListerId(1), source: None, dests: vec![],
                source_locked: false,
                settings: opusdm_ipc::Settings::default(),
                bindings: to_send,
            }).unwrap();
        });
        let (_client, _id, _roles, _settings, bindings) =
            Client::connect(&path, 1, std::path::Path::new("/")).unwrap();
        assert_eq!(bindings, sent);
        server.join().unwrap();
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc proto` and `cargo test -p opusdm-hub set_bindings`
Expected: FAIL to compile (`SetBindings`, `BindingsUpdate`, `bindings` field, `ServerOptions` not found).

- [ ] **Step 3: Protocol messages (`crates/ipc/src/proto.rs`)**

Add to `ToHub` (after `SelectionReply`):

```rust
    /// Replace the hotkeys, user menu and script events. The hub sanitizes,
    /// persists to `hotkeys.json`/`usermenu.json`/`scripts.json` and
    /// broadcasts `ToLister::BindingsUpdate` to every Lister. No reply.
    SetBindings(crate::bindings::Bindings),
```

Add to `ToLister` (after `FunctionDone`):

```rust
    /// The bindings changed: the full new `Bindings`, replacing the ones
    /// from `Welcome`.
    BindingsUpdate(crate::bindings::Bindings),
```

Add to `Welcome` (after `settings`):

```rust
    /// The current hotkeys, user menu and script events.
    #[serde(default)]
    pub bindings: crate::bindings::Bindings,
```

- [ ] **Step 4: Hub (`crates/hub/src/server.rs`)**

Imports: add `use opusdm_ipc::bindings::{self, Bindings};` next to the other `opusdm_ipc` imports.

Add after the `HubCmd` enum and `ServerHandle` (before `spawn_server`):

```rust
/// Knobs the tests leave off and `main` turns on.
pub struct ServerOptions {
    /// Bindings to start with. `None` loads them from disk (production). The
    /// default is empty bindings, so a test never reads, or acts on, a
    /// developer's real hotkeys, menu and script events.
    pub initial_bindings: Option<Bindings>,
}

impl Default for ServerOptions {
    /// Options for tests: no disk read.
    fn default() -> Self {
        Self { initial_bindings: Some(Bindings::default()) }
    }
}

impl ServerOptions {
    /// Options for the real hub: load the bindings from disk.
    pub fn production() -> Self {
        Self { initial_bindings: None }
    }
}
```

Rename the existing `pub fn spawn_server(listener, on_spawn)` body into `spawn_server_with` and keep `spawn_server` as a wrapper:

```rust
/// Start the socket server on `listener` with test-friendly options. Returns
/// channels to/from the GTK side.
pub fn spawn_server(listener: UnixListener, on_spawn: SpawnFn) -> ServerHandle {
    spawn_server_with(listener, on_spawn, ServerOptions::default())
}

/// Start the socket server on `listener`. Returns channels to/from the GTK side.
pub fn spawn_server_with(listener: UnixListener, on_spawn: SpawnFn, opts: ServerOptions) -> ServerHandle {
    // ... existing body of spawn_server, except the owner thread line:
    //     .spawn(move || owner_loop(owner_rx, owner_tx, evt_tx, on_spawn, opts))
}
```

`owner_loop` gets a fifth parameter `opts: ServerOptions`. In `Hub` add the field `current_bindings: Bindings,` (doc: "Loaded once at startup; updated in place by `ToHub::SetBindings`.") and initialise it in the `Hub { ... }` literal:

```rust
        current_bindings: match opts.initial_bindings {
            Some(b) => b,
            None => bindings::sanitize(bindings::load_bindings()),
        },
```

Persistence: add `Bindings(Bindings),` to `enum PersistMsg` and this arm to the persist thread's `match msg`:

```rust
                    PersistMsg::Bindings(b) => {
                        if let Err(e) = bindings::save_bindings(&b) {
                            opusdm_ipc::err_log!("hub", "cannot save bindings: {e}");
                        }
                    }
```

In the `Register` arm add `bindings: self.current_bindings.clone(),` after `settings: ...` in the `Welcome` literal. Add a new arm in `lister_msg` (after `SetSettings`):

```rust
            ToHub::SetBindings(new_bindings) => {
                self.current_bindings = bindings::sanitize(new_bindings);
                let _ = self.persist_tx.send(PersistMsg::Bindings(self.current_bindings.clone()));
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::BindingsUpdate(self.current_bindings.clone()).into());
                }
            }
```

`crates/hub/src/main.rs`: replace `server::spawn_server(` with `server::spawn_server_with(` and add `server::ServerOptions::production(),` as the third argument (after the `Box::new(...)` closure).

- [ ] **Step 5: Lister plumbing**

`crates/lister/src/client.rs`: change `connect`'s return type to `io::Result<(Client, ListerId, Roles, Settings, Bindings)>` (add `Bindings` to the `use opusdm_ipc::{...}` list), destructure `let Welcome { lister_id: id, source, dests, source_locked, settings, bindings } = welcome;` and return `Ok((Client { ... }, id, roles, settings, bindings))`.

`crates/lister/src/main.rs`: `let (client, lister_id, roles, settings, bindings) = match client::Client::connect(...)` and call `build::run(client, lister_id, roles, settings, bindings, start_path)`.

`crates/lister/src/build.rs`: add `bindings: Bindings` to `run`'s parameters (after `settings`), to `StartState` (field `bindings: Bindings`), to `build_window`'s parameters (call: `build_window(app, st.client.clone(), st.id, st.roles.clone(), st.settings.clone(), st.bindings.clone())`), and initialise the new `HubConnection` field in the `Ui { hub: HubConnection { ... } }` literal with `bindings: RefCell::new(bindings),`. Import `opusdm_ipc::Bindings`.

`crates/lister/src/window.rs`: add to `HubConnection`:

```rust
    /// Latest hotkeys, user menu and script events. Updated wholesale from
    /// `Welcome` and `BindingsUpdate`.
    pub bindings: RefCell<opusdm_ipc::Bindings>,
```

and add this arm to `apply_inbound`'s match (before `ToLister::Pong => {}`):

```rust
            ToLister::BindingsUpdate(new_bindings) => {
                *self.hub.bindings.borrow_mut() = new_bindings;
            }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p opusdm-ipc && cargo test -p opusdm-hub && cargo test -p opusdm-lister`
Expected: PASS, including `set_bindings_sanitizes_persists_and_broadcasts`, `welcome_carries_the_bindings_loaded_from_disk`, `connect_returns_the_bindings_from_welcome`. `cargo clippy -p opusdm-hub -p opusdm-lister --all-targets` shows no new warning.

- [ ] **Step 7: Commit**

```bash
printf -- '- Feature (F-03/F-04/F-05, step 2): `ToHub::SetBindings` / `ToLister::BindingsUpdate` / `Welcome.bindings`. The hub loads, sanitizes, persists and broadcasts `Bindings` (`ServerOptions` keeps tests off the real config); listers cache them in `HubConnection.bindings`.\n' >> CHANGES.md
git add crates/ipc/src/proto.rs crates/hub/src/server.rs crates/hub/src/main.rs crates/lister/src/client.rs crates/lister/src/main.rs crates/lister/src/build.rs crates/lister/src/window.rs CHANGES.md
git commit -m "feat: hub owns and broadcasts bindings, listers cache them"
```

---

### Task 3: `RunFunctionById`, headless runs, FIFO selection waiters

**Files:**
- Modify: `crates/ipc/src/proto.rs`
- Modify: `crates/hub/src/server.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `opusdm_ipc::function::{load_functions, validate, FnStep, SelectionCtx}`, `crate::functions::{spawn_executor, FnEvent}`.
- Produces:
  - `ToHub::RunFunctionById { id: FunctionId }`.
  - `HubCmd::RunFunctionHeadless(FunctionId)`.
  - In `Hub`: `const HUB_REQUESTER: u64 = u64::MAX;`, `fn start_function_run(&mut self, requester: u64, source: Option<u64>, steps: Vec<FnStep>) -> u64`, `fn run_function_by_id(&mut self, requester: u64, source: Option<u64>, id: &str) -> Option<u64>` (returns the started run id), `fn run_headless(&mut self, id: &str) -> Option<u64>`, `fn function_failed(&mut self, requester: u64, error: String)`.
  - `selection_waiters: HashMap<u64, VecDeque<u64>>` (per-connection FIFO of run ids).

- [ ] **Step 1: Write the failing tests**

Add to the `server.rs` tests module (helper first, then tests). `Function`/`FnStep` come from `opusdm_ipc`:

```rust
    /// A saved function whose only step appends a line to `marker`.
    fn marker_function(name: &str, marker: &std::path::Path) -> opusdm_ipc::Function {
        let mut f = opusdm_ipc::Function::new(name);
        f.steps = vec![FnStep::External {
            argv: vec!["sh".into(), "-c".into(), format!("echo x >> {}", marker.display())],
            cwd_source: true,
        }];
        f
    }

    /// Poll until `marker` has at least `lines` lines, or panic.
    fn wait_for_lines(marker: &std::path::Path, lines: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let count = std::fs::read_to_string(marker).map(|s| s.lines().count()).unwrap_or(0);
            if count >= lines {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("{marker:?} has {count} lines, wanted {lines}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn run_function_by_id_with_an_unknown_id_reports_failure() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::RunFunctionById { id: "nope".into() }).unwrap();

        let got: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(got, ToLister::FunctionFailed { step_idx: 0, .. }), "got {got:?}");
    }

    #[test]
    fn run_function_by_id_runs_a_saved_function() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("touch", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        send(&mut a, &ToHub::RunFunctionById { id: f.id.clone() }).unwrap();
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn headless_run_without_listers_executes_immediately() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("touch", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, _path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn headless_run_queries_the_source_lister() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("touch", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
        let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

        handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn two_runs_waiting_on_one_lister_both_complete_in_order() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker_a = out.path().join("a");
        let marker_b = out.path().join("b");
        let fa = marker_function("a", &marker_a);
        let fb = marker_function("b", &marker_b);
        opusdm_ipc::function::save_functions(&[fa.clone(), fb.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        // Both requests reach the hub before the lister answers either.
        send(&mut a, &ToHub::RunFunctionById { id: fa.id.clone() }).unwrap();
        send(&mut a, &ToHub::RunFunctionById { id: fb.id.clone() }).unwrap();
        for _ in 0..2 {
            let q: ToLister = recv(&mut a).unwrap().unwrap();
            assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
        }
        for _ in 0..2 {
            send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
        }

        wait_for_lines(&marker_a, 1);
        wait_for_lines(&marker_b, 1);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-hub run_function_by_id headless two_runs`
Expected: FAIL to compile (`RunFunctionById`, `RunFunctionHeadless` not found).

- [ ] **Step 3: Message and command**

`crates/ipc/src/proto.rs`, in `ToHub` after `RunFunction`:

```rust
    /// Run a saved Function by id: the hub loads it from `functions.json`,
    /// validates it and runs it with this Lister as SOURCE. Used by hotkeys
    /// and the user menu; `RunFunction { steps }` stays for unsaved edits.
    RunFunctionById { id: crate::function::FunctionId },
```

Also add `roundtrip_tohub(ToHub::RunFunctionById { id: "fn-1".into() });` to `every_tohub_variant_roundtrips`.

`server.rs`: add to `HubCmd`:

```rust
    /// Run a saved Function on the hub's own initiative (global hotkey,
    /// script event): SOURCE is the current SOURCE Lister, or an empty
    /// selection when there is none.
    RunFunctionHeadless(opusdm_ipc::function::FunctionId),
```

- [ ] **Step 4: Hub run machinery**

In `server.rs`: add `use std::collections::{BTreeSet, HashMap, VecDeque};` (extend the existing `std::collections` import) and, next to `SpawnFn`:

```rust
/// Requester id of a Function run the hub started itself (no Lister asked
/// for it). Same sentinel value as the desktop-originated job requester.
const HUB_REQUESTER: u64 = u64::MAX;

/// The home directory, or `/` when `HOME` is unset: the working directory of
/// a run that has no SOURCE Lister.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
```

In the `Hub` struct change the field to

```rust
    /// Which pending runs a Lister connection's next `SelectionReply`s are
    /// for, oldest first: a Lister answers `QuerySelection`s in order, so
    /// two runs waiting on it at once are matched by a FIFO, not a single slot.
    selection_waiters: HashMap<u64, VecDeque<u64>>,
```

(the `Hub { ... }` literal keeps `selection_waiters: HashMap::new(),`). In `gtk_cmd`, replace the local `const GTK_REQUESTER: u64 = u64::MAX;` and its uses with `HUB_REQUESTER`, and add the arm:

```rust
            HubCmd::RunFunctionHeadless(id) => {
                self.run_headless(&id);
            }
```

In `lister_msg`, change the `RunFunction` arm to `self.start_function_run(cid, Some(cid), steps);` and add:

```rust
            ToHub::RunFunctionById { id } => {
                self.run_function_by_id(cid, Some(cid), &id);
            }
```

Replace `start_function_run` (`server.rs`, the method starting at "`ToHub::RunFunction` from `cid`: record a pending run") with:

```rust
    /// Start a Function run for `requester`. With `source == Some(cid)` the
    /// hub asks `cid` (SOURCE) and, if one exists, the first DEST Lister other
    /// than `cid` for their selection first. With `None` there is no Lister to
    /// ask: the run starts at once with an empty selection in `$HOME`.
    /// Returns the run id.
    fn start_function_run(&mut self, requester: u64, source: Option<u64>, steps: Vec<FnStep>) -> u64 {
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        let Some(source_cid) = source else {
            self.function_requesters.insert(run_id, requester);
            let empty = SelectionCtx { paths: Vec::new(), cwd: home_dir() };
            crate::functions::spawn_executor(run_id, steps, empty, None, self.function_events_tx.clone());
            return run_id;
        };
        // Exclude the source itself: a Lister that currently holds a DEST role
        // can still be the one requesting the run. Picking it as its own
        // `dest_cid` would make `selection_reply` only ever fill `source_sel`
        // (the two branches are `if`/`else if` on the same `cid`), so
        // `dest_sel` - and the run - could never complete.
        let dest_cid = self.roles.dests().iter().map(|d| d.0).find(|&d| d != source_cid);
        self.pending_functions.insert(
            run_id,
            PendingFunctionRun {
                requester_cid: requester,
                steps,
                source_cid,
                dest_cid,
                source_sel: None,
                dest_sel: None,
            },
        );
        self.query_selection(source_cid, run_id);
        if let Some(d) = dest_cid {
            self.query_selection(d, run_id);
        }
        run_id
    }

    /// Queue `run_id` on `cid`'s FIFO of pending selection requests and ask
    /// the Lister for its selection.
    fn query_selection(&mut self, cid: u64, run_id: u64) {
        self.selection_waiters.entry(cid).or_default().push_back(run_id);
        if let Some(c) = self.conns.get_mut(&cid) {
            let _ = c.tx.send(ToLister::QuerySelection.into());
        }
    }

    /// Load the saved function `id`, validate it and start it for `requester`
    /// (see [`Self::start_function_run`]). A missing or invalid function is
    /// reported through [`Self::function_failed`]. Returns the run id.
    fn run_function_by_id(&mut self, requester: u64, source: Option<u64>, id: &str) -> Option<u64> {
        let Some(function) = opusdm_ipc::function::load_functions().into_iter().find(|f| f.id == id) else {
            self.function_failed(requester, format!("function '{id}' not found (it may have been deleted)"));
            return None;
        };
        let problems = opusdm_ipc::function::validate(&function);
        if let Some(first) = problems.first() {
            self.function_failed(requester, format!("function '{}' is invalid: {}", function.name, first.message));
            return None;
        }
        Some(self.start_function_run(requester, source, function.steps))
    }

    /// Run the saved function `id` on the hub's own initiative: SOURCE is the
    /// Lister holding the SOURCE role (if it is registered), else nothing.
    fn run_headless(&mut self, id: &str) -> Option<u64> {
        let source = self
            .roles
            .source()
            .map(|l| l.0)
            .filter(|cid| self.conns.get(cid).is_some_and(|c| c.registered));
        self.run_function_by_id(HUB_REQUESTER, source, id)
    }

    /// Tell `requester` a run could not start. A hub-originated run has no
    /// Lister to tell, so the failure goes to the log.
    fn function_failed(&mut self, requester: u64, error: String) {
        if requester == HUB_REQUESTER {
            opusdm_ipc::err_log!("hub", "function run failed: {error}");
            return;
        }
        if let Some(c) = self.conns.get_mut(&requester) {
            let _ = c.tx.send(ToLister::FunctionFailed { step_idx: 0, error }.into());
        }
    }
```

Replace `selection_reply`'s opening lines (up to and including `let Some(run) = self.pending_functions.get_mut(&run_id) else { return };`) with:

```rust
        let Some(queue) = self.selection_waiters.get_mut(&cid) else { return };
        let Some(run_id) = queue.pop_front() else { return };
        if queue.is_empty() {
            self.selection_waiters.remove(&cid);
        }
        let Some(run) = self.pending_functions.get_mut(&run_id) else { return };
```

(the rest of the method is unchanged). Replace `function_event` with:

```rust
    /// A `FnEvent` arrived from a running Function's executor thread; turn
    /// it into the matching `ToLister` and send it to whichever connection
    /// asked for this run. A hub-originated run has no such connection: its
    /// failures are logged instead.
    fn function_event(&mut self, run_id: u64, ev: crate::functions::FnEvent) {
        let Some(&requester) = self.function_requesters.get(&run_id) else { return };
        let terminal = matches!(ev, crate::functions::FnEvent::Failed { .. } | crate::functions::FnEvent::AllDone);
        if terminal {
            self.function_requesters.remove(&run_id);
        }
        if requester == HUB_REQUESTER {
            match ev {
                crate::functions::FnEvent::Failed { step_idx, error } => {
                    opusdm_ipc::err_log!("hub", "function run {run_id} failed at step {step_idx}: {error}");
                }
                crate::functions::FnEvent::Output { step_idx, line } => {
                    opusdm_ipc::debug_log!("hub", "function run {run_id} step {step_idx}: {line}");
                }
                _ => {}
            }
            return;
        }
        let msg = match ev {
            crate::functions::FnEvent::Output { step_idx, line } => ToLister::FunctionOutput { step_idx, line },
            crate::functions::FnEvent::StepDone { step_idx, exit_code } => {
                ToLister::FunctionStepDone { step_idx, exit_code }
            }
            crate::functions::FnEvent::Failed { step_idx, error } => ToLister::FunctionFailed { step_idx, error },
            crate::functions::FnEvent::AllDone => ToLister::FunctionDone,
        };
        if let Some(c) = self.conns.get_mut(&requester) {
            let _ = c.tx.send(msg.into());
        }
    }
```

Replace `drop_function_state` with:

```rust
    /// Clean up any Function-run bookkeeping referencing `cid`, called when
    /// its connection drops. A pending run waiting on `cid` for one of its
    /// two selections can now never complete (its other side, if any, would
    /// otherwise wait forever); a running Function's events for a
    /// since-dropped requester are also dropped instead of accumulating.
    fn drop_function_state(&mut self, cid: u64) {
        let doomed: Vec<u64> = self
            .pending_functions
            .iter()
            .filter(|(_, run)| run.source_cid == cid || run.dest_cid == Some(cid))
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in doomed {
            self.pending_functions.remove(&run_id);
            for queue in self.selection_waiters.values_mut() {
                queue.retain(|&r| r != run_id);
            }
        }
        self.selection_waiters.remove(&cid);
        self.selection_waiters.retain(|_, queue| !queue.is_empty());
        self.function_requesters.retain(|_, requester| *requester != cid);
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p opusdm-hub`
Expected: PASS, including the five new tests and the two existing `run_function_*` tests. `cargo clippy -p opusdm-hub --all-targets` shows no new warning.

- [ ] **Step 6: Commit**

```bash
printf -- '- Feature (F-03/F-04/F-05, step 3): `ToHub::RunFunctionById` and hub-originated ("headless") Function runs (`HubCmd::RunFunctionHeadless`, requester sentinel `u64::MAX`, SOURCE = current SOURCE Lister or an empty selection in $HOME). `selection_waiters` is now a per-Lister FIFO so two runs waiting on one Lister both complete.\n' >> CHANGES.md
git add crates/ipc/src/proto.rs crates/hub/src/server.rs CHANGES.md
git commit -m "feat(hub): run saved functions by id and from the hub itself"
```

---

### Task 4: Local hotkeys in the lister (lookup table and dispatch)

**Files:**
- Create: `crates/lister/src/hotkeys.rs`
- Modify: `crates/lister/src/main.rs`, `crates/lister/src/window.rs`, `crates/lister/src/build.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `opusdm_ipc::bindings::{parse_key, Hotkey, KeyCombo, Mods}`, `crate::keys::Focus`, `ToHub::RunFunctionById` (Task 3).
- Produces:
  - `pub fn mods_from_state(state: gdk::ModifierType) -> Mods`.
  - `pub fn combo_from_event(key: gdk::Key, state: gdk::ModifierType) -> Option<KeyCombo>` (`None` for modifier-only presses).
  - `pub struct HotkeyMap` (`Default`), `HotkeyMap::from_hotkeys(&[Hotkey]) -> HotkeyMap`, `HotkeyMap::lookup(&self, key: gdk::Key, state: gdk::ModifierType, focus: Focus) -> Option<&FunctionId>`.
  - `HubConnection.hotkeys: RefCell<HotkeyMap>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/lister/src/hotkeys.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn hk(key: &str, function: &str, global: bool) -> Hotkey {
        Hotkey { key: key.into(), function: function.into(), global }
    }

    fn ctrl_alt() -> gdk::ModifierType {
        gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
    }

    #[test]
    fn a_local_hotkey_is_found_by_key_and_modifiers() {
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+Alt+u", "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::u, ctrl_alt(), Focus::List), Some(&"f1".to_string()));
        assert_eq!(map.lookup(gdk::Key::u, gdk::ModifierType::CONTROL_MASK, Focus::List), None);
        assert_eq!(map.lookup(gdk::Key::i, ctrl_alt(), Focus::List), None);
    }

    #[test]
    fn the_shifted_keyval_of_a_letter_still_matches() {
        // Shift+u arrives as keyval `U` with the shift mask set.
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+Shift+u", "f1", false)]);
        let state = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK;
        assert_eq!(map.lookup(gdk::Key::U, state, Focus::List), Some(&"f1".to_string()));
    }

    #[test]
    fn global_and_unparsable_hotkeys_are_left_out_and_the_first_duplicate_wins() {
        let map = HotkeyMap::from_hotkeys(&[
            hk("Ctrl+Alt+g", "global", true),
            hk("Ctrl+Alt+???", "bad", false),
            hk("Ctrl+Alt+u", "first", false),
            hk("ctrl+alt+U", "second", false),
        ]);
        assert_eq!(map.lookup(gdk::Key::g, ctrl_alt(), Focus::List), None);
        assert_eq!(map.lookup(gdk::Key::u, ctrl_alt(), Focus::List), Some(&"first".to_string()));
    }

    #[test]
    fn a_hand_written_uppercase_letter_still_matches() {
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+U", "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::u, gdk::ModifierType::CONTROL_MASK, Focus::List), Some(&"f1".to_string()));
    }

    #[test]
    fn a_plain_letter_never_fires_while_typing() {
        let map = HotkeyMap::from_hotkeys(&[hk("j", "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::j, gdk::ModifierType::empty(), Focus::List), Some(&"f1".to_string()));
        assert_eq!(map.lookup(gdk::Key::j, gdk::ModifierType::empty(), Focus::Editing), None);
    }

    #[test]
    fn a_command_modifier_or_a_function_key_fires_while_typing() {
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+j", "ctrl", false), hk("F5", "fkey", false), hk("Shift+j", "shift", false)]);
        assert_eq!(map.lookup(gdk::Key::j, gdk::ModifierType::CONTROL_MASK, Focus::Editing), Some(&"ctrl".to_string()));
        assert_eq!(map.lookup(gdk::Key::F5, gdk::ModifierType::empty(), Focus::Editing), Some(&"fkey".to_string()));
        assert_eq!(map.lookup(gdk::Key::J, gdk::ModifierType::SHIFT_MASK, Focus::Editing), None, "Shift alone is still typing");
    }

    #[test]
    fn combo_from_event_lowercases_letters_and_reads_modifiers() {
        let combo = combo_from_event(gdk::Key::U, gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK).unwrap();
        assert_eq!(combo.name, "u");
        assert!(combo.mods.ctrl && combo.mods.shift && !combo.mods.alt && !combo.mods.sup);
    }

    #[test]
    fn combo_from_event_ignores_modifier_only_presses() {
        for key in [gdk::Key::Control_L, gdk::Key::Alt_R, gdk::Key::Shift_L, gdk::Key::Super_L, gdk::Key::Caps_Lock, gdk::Key::Num_Lock] {
            assert_eq!(combo_from_event(key, gdk::ModifierType::empty()), None, "{key:?} is only a modifier");
        }
    }

    #[test]
    fn combo_text_roundtrips_through_the_map() {
        let combo = combo_from_event(gdk::Key::F7, gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK).unwrap();
        let text = opusdm_ipc::bindings::format_key(&combo);
        assert_eq!(text, "Alt+Shift+F7");
        let map = HotkeyMap::from_hotkeys(&[hk(&text, "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::F7, gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK, Focus::List), Some(&"f1".to_string()));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-lister hotkeys`
Expected: FAIL to compile (`HotkeyMap`, `combo_from_event` not found; the module is not in `main.rs` yet, so first add `mod hotkeys;` to `crates/lister/src/main.rs` in alphabetical position after `mod glyphs;`).

- [ ] **Step 3: Implement `hotkeys.rs`**

Put this above the test module:

```rust
//! Local user hotkeys (F-03): the lookup table a Lister consults before its
//! built-in key table, and the helpers that turn a GDK key event into the
//! canonical [`KeyCombo`] the bindings file stores.

use crate::keys::Focus;
use gtk4::gdk;
use opusdm_ipc::bindings::{parse_key, Hotkey, KeyCombo, Mods};
use opusdm_ipc::function::FunctionId;
use std::collections::HashMap;

/// The Ctrl/Alt/Shift/Super flags held in `state`. NumLock and CapsLock are
/// ignored on purpose.
pub fn mods_from_state(state: gdk::ModifierType) -> Mods {
    Mods {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        alt: state.contains(gdk::ModifierType::ALT_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        sup: state.contains(gdk::ModifierType::SUPER_MASK),
    }
}

/// Whether `key` is a modifier (or lock) key pressed on its own.
fn is_modifier_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
            | gdk::Key::Shift_Lock
    )
}

/// Whether `key` is `F1`..`F35`, a key that keeps working while typing.
fn is_function_key(key: gdk::Key) -> bool {
    let Some(name) = key.name() else { return false };
    let Some(digits) = name.strip_prefix('F') else { return false };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// The combination a key event stands for, with the key lower-cased the same
/// way the lookup does. `None` for a modifier-only press.
pub fn combo_from_event(key: gdk::Key, state: gdk::ModifierType) -> Option<KeyCombo> {
    if is_modifier_key(key) {
        return None;
    }
    let name = key.to_lower().name()?.to_string();
    Some(KeyCombo { mods: mods_from_state(state), name })
}

/// Local (non-global) user hotkeys, keyed by (modifiers, lower-cased key).
#[derive(Default)]
pub struct HotkeyMap {
    map: HashMap<(Mods, gdk::Key), FunctionId>,
}

impl HotkeyMap {
    /// Build the table from a hotkey list. Global hotkeys are grabbed by the
    /// hub and never reach the Lister, so they are left out; entries whose key
    /// text does not parse or name a known key are skipped; the first of two
    /// entries on the same key wins.
    pub fn from_hotkeys(hotkeys: &[Hotkey]) -> Self {
        let mut map = HashMap::new();
        for hotkey in hotkeys.iter().filter(|h| !h.global) {
            let Ok(combo) = parse_key(&hotkey.key) else { continue };
            let Some(key) = gdk::Key::from_name(&combo.name) else { continue };
            map.entry((combo.mods, key.to_lower())).or_insert_with(|| hotkey.function.clone());
        }
        Self { map }
    }

    /// The function bound to this key event, if any. While a text field has
    /// the focus only a key with Ctrl, Alt or Super, or a function key, counts:
    /// a bare letter must stay a character.
    pub fn lookup(&self, key: gdk::Key, state: gdk::ModifierType, focus: Focus) -> Option<&FunctionId> {
        let mods = mods_from_state(state);
        let key = key.to_lower();
        if focus == Focus::Editing && !(mods.has_command_modifier() || is_function_key(key)) {
            return None;
        }
        self.map.get(&(mods, key))
    }
}
```

- [ ] **Step 4: Wire it into the window and the key controller**

`crates/lister/src/window.rs`: add to `HubConnection`:

```rust
    /// Lookup table of the local user hotkeys, rebuilt whenever `bindings`
    /// changes.
    pub hotkeys: RefCell<crate::hotkeys::HotkeyMap>,
```

and change the `BindingsUpdate` arm added in Task 2 to:

```rust
            ToLister::BindingsUpdate(new_bindings) => {
                *self.hub.hotkeys.borrow_mut() = crate::hotkeys::HotkeyMap::from_hotkeys(&new_bindings.hotkeys);
                *self.hub.bindings.borrow_mut() = new_bindings;
            }
```

`crates/lister/src/build.rs`: in the `HubConnection { ... }` literal add `hotkeys: RefCell::new(crate::hotkeys::HotkeyMap::from_hotkeys(&bindings.hotkeys)),` **before** the line that moves `bindings` (`bindings: RefCell::new(bindings),`). Replace the body of the key controller closure (the block that starts `key_ctrl.connect_key_pressed(move |_, keyval, _, state| {`) with:

```rust
        key_ctrl.connect_key_pressed(move |_, keyval, _, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            // Single-letter bindings must not steal characters from a text
            // field, so where the focus sits decides how a key is read.
            let focus = if crate::window::is_editing(&ui_for_keys.window) {
                crate::keys::Focus::Editing
            } else {
                crate::keys::Focus::List
            };
            // A user hotkey (F-03) is looked up first, so it wins over a
            // built-in binding.
            let user_function = ui_for_keys.hub.hotkeys.borrow().lookup(keyval, state, focus).cloned();
            if let Some(id) = user_function {
                let _ = ui_for_keys.hub.client.send(&opusdm_ipc::ToHub::RunFunctionById { id });
                return glib::Propagation::Stop;
            }
            let Some(action) = crate::keys::resolve(keyval, ctrl, shift, focus) else {
                return glib::Propagation::Proceed;
            };
            ui_for_keys.run_key_action(action);
            glib::Propagation::Stop
        });
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p opusdm-lister`
Expected: PASS (all `hotkeys::tests` plus the existing suite). `cargo clippy -p opusdm-lister --all-targets` shows no new warning.

- [ ] **Step 6: Commit**

```bash
printf -- '- Feature (F-03, step 4): local user hotkeys - the Lister keeps a `HotkeyMap` built from `Bindings`, consults it before `keys::resolve` (the user wins over built-ins), and never fires a bare letter while a text field has focus.\n' >> CHANGES.md
git add crates/lister/src/hotkeys.rs crates/lister/src/main.rs crates/lister/src/window.rs crates/lister/src/build.rs CHANGES.md
git commit -m "feat(lister): local user hotkeys run functions"
```

---

### Task 5: Hotkey editor and "Find Key"

**Files:**
- Create: `crates/lister/src/binding_ui.rs`, `crates/lister/src/hotkey_editor.rs`
- Modify: `crates/lister/src/keys.rs`, `palette.rs`, `main.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `bindings::{validate_hotkeys, BindingError, format_key, parse_key, Hotkey, KeyCombo}`, `hotkeys::combo_from_event`, `keys::{resolve, Action, Focus, Motion}`, `opusdm_ui::{DialogShell, requester_entry}`, `Ui.hub.bindings`.
- Produces:
  - `binding_ui::function_dropdown(functions: &[Function], selected: &str) -> gtk::DropDown`, `binding_ui::selected_function(&gtk::DropDown, &[Function]) -> Option<FunctionId>`.
  - `hotkey_editor::{KeyMeaning, find_key, open_hotkey_editor, open_find_key}`.
  - `CommandId::{ManageHotkeys, FindKey}`, `palette::builtin_commands() -> &'static [CommandItem]`.

- [ ] **Step 1: Write the failing tests**

In `crates/lister/src/keys.rs` tests add:

```rust
    #[test]
    fn hotkey_commands_exist_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageHotkeys));
        assert!(known.contains(&CommandId::FindKey));
    }
```

Create `crates/lister/src/hotkey_editor.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::bindings::{parse_key, Hotkey};
    use opusdm_ipc::Function;

    fn combo(text: &str) -> KeyCombo {
        parse_key(text).unwrap()
    }

    fn function(id: &str, name: &str) -> Function {
        let mut f = Function::new(name);
        f.id = id.to_string();
        f
    }

    #[test]
    fn a_user_hotkey_is_reported_with_its_function_name_and_global_flag() {
        let hotkeys = [Hotkey { key: "Ctrl+Alt+u".into(), function: "f1".into(), global: true }];
        let functions = [function("f1", "Copy and notify")];
        assert_eq!(
            find_key(&hotkeys, &functions, &combo("ctrl+alt+U")),
            KeyMeaning::UserFunction { name: "Copy and notify".into(), global: true }
        );
    }

    #[test]
    fn a_dangling_hotkey_is_reported_as_a_missing_function() {
        let hotkeys = [Hotkey { key: "Ctrl+Alt+u".into(), function: "gone".into(), global: false }];
        assert_eq!(
            find_key(&hotkeys, &[], &combo("Ctrl+Alt+u")),
            KeyMeaning::UserFunction { name: "(missing function)".into(), global: false }
        );
    }

    #[test]
    fn a_built_in_key_is_reported_with_the_palette_title() {
        match find_key(&[], &[], &combo("F7")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("Make Directory"), "{title}"),
            other => panic!("{other:?}"),
        }
        match find_key(&[], &[], &combo("Ctrl+c")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("Clipboard"), "{title}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn shift_turns_a_letter_into_its_uppercase_binding() {
        match find_key(&[], &[], &combo("g")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("first"), "{title}"),
            other => panic!("{other:?}"),
        }
        match find_key(&[], &[], &combo("Shift+g")) {
            KeyMeaning::Builtin { title } => assert!(title.contains("last"), "{title}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unbound_key_is_unassigned() {
        assert_eq!(find_key(&[], &[], &combo("Ctrl+Alt+u")), KeyMeaning::Unassigned);
        assert_eq!(find_key(&[], &[], &combo("Alt+c")), KeyMeaning::Unassigned, "Alt never reaches a built-in");
    }

    #[test]
    fn a_user_hotkey_wins_over_a_built_in_in_find_key() {
        let hotkeys = [Hotkey { key: "F7".into(), function: "f1".into(), global: false }];
        let functions = [function("f1", "Mine")];
        assert_eq!(
            find_key(&hotkeys, &functions, &combo("F7")),
            KeyMeaning::UserFunction { name: "Mine".into(), global: false }
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-lister hotkey`
Expected: FAIL to compile (`find_key`, `KeyMeaning`, `CommandId::ManageHotkeys` not found; add `mod binding_ui; mod hotkey_editor;` to `main.rs` first, in alphabetical positions).

- [ ] **Step 3: Shared drop-down helpers (`crates/lister/src/binding_ui.rs`)**

```rust
//! Small widgets shared by the hotkey, user-menu and script-event editors.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::function::{Function, FunctionId};

/// A drop-down listing every function by name, with `selected` (a function
/// id) pre-selected. Nothing is selected when the id is not in the list: a
/// dangling reference, which the editors flag as an error.
pub fn function_dropdown(functions: &[Function], selected: &str) -> gtk::DropDown {
    let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&names);
    let position = functions
        .iter()
        .position(|f| f.id == selected)
        .map(|p| p as u32)
        .unwrap_or(gtk::INVALID_LIST_POSITION);
    dropdown.set_selected(position);
    dropdown
}

/// The id of the function chosen in `dropdown`, or `None` when nothing is
/// selected.
pub fn selected_function(dropdown: &gtk::DropDown, functions: &[Function]) -> Option<FunctionId> {
    functions.get(dropdown.selected() as usize).map(|f| f.id.clone())
}
```

- [ ] **Step 4: Palette and keys**

`keys.rs`: add to `CommandId` after `RunFunction`:

```rust
    /// Open the hotkey editor.
    ManageHotkeys,
    /// Ask for a key combination and say what it does.
    FindKey,
```

`palette.rs`: add to the `COMMANDS` array after the `ManageFunctions` entry:

```rust
        CommandItem { id: CommandId::ManageHotkeys, title: Cow::Borrowed("Edit Hotkeys..."), category: Cow::Borrowed("Functions"), shortcut: None },
        CommandItem { id: CommandId::FindKey, title: Cow::Borrowed("Find Key..."), category: Cow::Borrowed("Functions"), shortcut: None },
```

change `const COMMANDS: &[CommandItem] = &[` to `static COMMANDS: &[CommandItem] = &[` (a reference to a `const` holding `Cow` values is not promoted to `'static`, a `static` is; `COMMANDS.to_vec()` in `get_commands` keeps working), and add this function after `get_commands`:

```rust
/// The static built-in commands, without the dynamic per-function entries
/// (no disk read): what "Find Key" names a built-in key by.
pub fn builtin_commands() -> &'static [CommandItem] {
    COMMANDS
}
```

and add these arms to `execute_command` (after `ManageFunctions`):

```rust
        CommandId::ManageHotkeys => {
            crate::hotkey_editor::open_hotkey_editor(ui);
        }
        CommandId::FindKey => {
            crate::hotkey_editor::open_find_key(ui);
        }
```

- [ ] **Step 5: Implement the editor (`crates/lister/src/hotkey_editor.rs`)**

Put this above the test module:

```rust
//! The hotkey editor (F-03): one row per binding (key, function, Global),
//! key capture, live validation, and the "Find Key" requester. See
//! `docs/superpowers/specs/2026-09-29-hotkeys-usermenu-scripts-design.md`.

use crate::keys::{Action, Focus, Motion};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, format_key, parse_key, BindingError, Hotkey, KeyCombo};
use opusdm_ipc::function::{self, Function, FunctionId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// What a key combination does, as "Find Key" reports it.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyMeaning {
    /// A user hotkey runs a function.
    UserFunction { name: String, global: bool },
    /// A built-in binding, named by its palette title or a short description.
    Builtin { title: String },
    /// Nothing is bound.
    Unassigned,
}

/// What `combo` does: a user hotkey first (it wins over built-ins), then a
/// built-in binding, else nothing.
pub fn find_key(hotkeys: &[Hotkey], functions: &[Function], combo: &KeyCombo) -> KeyMeaning {
    let identity = combo.identity();
    for hotkey in hotkeys {
        let Ok(bound) = parse_key(&hotkey.key) else { continue };
        if bound.identity() != identity {
            continue;
        }
        let name = functions
            .iter()
            .find(|f| f.id == hotkey.function)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "(missing function)".to_string());
        return KeyMeaning::UserFunction { name, global: hotkey.global };
    }
    match builtin_title(combo) {
        Some(title) => KeyMeaning::Builtin { title },
        None => KeyMeaning::Unassigned,
    }
}

/// The built-in binding `combo` triggers in the file list, if any. Alt and
/// Super never reach the built-in table. Shift turns a letter into its
/// uppercase keyval (`g` moves to the first row, `G` to the last).
fn builtin_title(combo: &KeyCombo) -> Option<String> {
    if combo.mods.alt || combo.mods.sup {
        return None;
    }
    let name = combo.normalized_name();
    let shifted_letter = combo.mods.shift && name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic());
    let key = if shifted_letter { gdk::Key::from_name(name.to_ascii_uppercase())? } else { gdk::Key::from_name(&name)? };
    let action = crate::keys::resolve(key, combo.mods.ctrl, combo.mods.shift, Focus::List)?;
    Some(action_title(&action))
}

/// A short description of a built-in action.
fn action_title(action: &Action) -> String {
    match action {
        Action::Command(id) => crate::palette::builtin_commands()
            .iter()
            .find(|c| &c.id == id)
            .map(|c| c.title.to_string())
            .unwrap_or_else(|| format!("{id:?}")),
        Action::Move(Motion::Down) => "Move the cursor down".to_string(),
        Action::Move(Motion::Up) => "Move the cursor up".to_string(),
        Action::Move(Motion::First) => "Move the cursor to the first row".to_string(),
        Action::Move(Motion::Last) => "Move the cursor to the last row".to_string(),
        Action::Enter => "Open the row under the cursor".to_string(),
        Action::ToggleSelectAndAdvance => "Toggle the selection and move down".to_string(),
        Action::SwapRoleFocus => "Switch to the SOURCE/DEST Lister".to_string(),
        Action::OpenPalette => "Open the command palette".to_string(),
        Action::RevealFilter => "Show the filter bar".to_string(),
        Action::ToggleTerminal => "Toggle the terminal drawer".to_string(),
        Action::PreviewOrSize => "Quick Look, or directory size".to_string(),
        Action::Escape => "Dismiss the filter, terminal or selection".to_string(),
    }
}

/// The set of saved function ids, for validation.
fn known_ids(functions: &[Function]) -> HashSet<FunctionId> {
    functions.iter().map(|f| f.id.clone()).collect()
}

/// Everything the editor's callbacks share.
struct State {
    hotkeys: RefCell<Vec<Hotkey>>,
    functions: Vec<Function>,
    rows_box: gtk::Box,
    save_btn: gtk::Button,
    window: gtk::Window,
}

/// Open the "Edit Hotkeys" window, working on a copy of the Lister's cached
/// hotkeys. Save sends only the hotkey section back to the hub.
pub fn open_hotkey_editor(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit Hotkeys").size(640, 420).scrolled();
    let root = shell.content();

    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.append(&rows_box);
    let add_btn = gtk::Button::with_label("Add Hotkey");
    add_btn.set_halign(gtk::Align::Start);
    root.append(&add_btn);

    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(State {
        hotkeys: RefCell::new(ui.hub.bindings.borrow().hotkeys.clone()),
        functions: function::load_functions(),
        rows_box,
        save_btn: save_btn.clone(),
        window: shell.window().clone(),
    });

    {
        let state = state.clone();
        add_btn.connect_clicked(move |_| {
            state.hotkeys.borrow_mut().push(Hotkey::default());
            rebuild(&state);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        save_btn.connect_clicked(move |_| {
            let mut updated = ui.hub.bindings.borrow().clone();
            updated.hotkeys = state.hotkeys.borrow().clone();
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::SetBindings(updated));
            crate::actions::flash(&ui, "hotkeys saved");
        });
    }

    rebuild(&state);
    shell.present();
}

/// Repaint every row from the state and re-run the validation that gates
/// Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.rows_box.first_child() {
        state.rows_box.remove(&child);
    }
    let hotkeys = state.hotkeys.borrow().clone();
    let errors = bindings::validate_hotkeys(&hotkeys, &known_ids(&state.functions));
    for (index, hotkey) in hotkeys.iter().enumerate() {
        state.rows_box.append(&build_row(state, index, hotkey, &errors));
    }
    state.save_btn.set_sensitive(errors.is_empty());
}

/// One editable row: key button, function drop-down, Global check box,
/// remove button, then this row's errors and built-in note underneath.
fn build_row(state: &Rc<State>, index: usize, hotkey: &Hotkey, errors: &[BindingError]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let key_text = if hotkey.key.is_empty() { "Press a key...".to_string() } else { hotkey.key.clone() };
    let key_btn = gtk::Button::with_label(&key_text);
    key_btn.set_size_request(190, -1);
    {
        let state = state.clone();
        key_btn.connect_clicked(move |_| capture_key(&state, index));
    }
    line.append(&key_btn);

    let dropdown = crate::binding_ui::function_dropdown(&state.functions, &hotkey.function);
    dropdown.set_hexpand(true);
    {
        let state = state.clone();
        dropdown.connect_selected_notify(move |d| {
            let Some(id) = crate::binding_ui::selected_function(d, &state.functions) else { return };
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                if h.function == id {
                    return;
                }
                h.function = id;
            }
            rebuild(&state);
        });
    }
    line.append(&dropdown);

    let global = gtk::CheckButton::with_label("Global");
    global.set_active(hotkey.global);
    {
        let state = state.clone();
        global.connect_toggled(move |c| {
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                h.global = c.is_active();
            }
            rebuild(&state);
        });
    }
    line.append(&global);

    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
    remove.add_css_class("flat");
    {
        let state = state.clone();
        remove.connect_clicked(move |_| {
            let mut hotkeys = state.hotkeys.borrow_mut();
            if index < hotkeys.len() {
                hotkeys.remove(index);
            }
            drop(hotkeys);
            rebuild(&state);
        });
    }
    line.append(&remove);
    row.append(&line);

    for error in errors.iter().filter(|e| e.hotkey_index() == Some(index)) {
        row.append(&note_label(&error.to_string(), true));
    }
    if let Ok(combo) = parse_key(&hotkey.key) {
        if let KeyMeaning::Builtin { title } = find_key(&[], &[], &combo) {
            row.append(&note_label(&format!("Overrides the built-in binding: {title}"), false));
        }
    }
    row
}

/// A small left-aligned label under a row; `is_error` adds the error style.
fn note_label(text: &str, is_error: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_start(6);
    if is_error {
        label.add_css_class("error");
    } else {
        label.add_css_class("dim-label");
    }
    label
}

/// Ask for a key combination in a small modal window and store its canonical
/// text in hotkey `index`. Escape cancels (so Escape itself cannot be bound).
fn capture_key(state: &Rc<State>, index: usize) {
    let shell = opusdm_ui::DialogShell::new(&state.window, "Press a key combination").size(340, 120).resizable(false);
    shell.content().append(&gtk::Label::new(Some("Press the key combination now.\nEscape cancels.")));
    shell.close_button("Cancel");
    let window = shell.window().clone();

    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let state = state.clone();
        let window = window.clone();
        controller.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Escape {
                window.close();
                return glib::Propagation::Stop;
            }
            let Some(combo) = crate::hotkeys::combo_from_event(key, mods) else {
                return glib::Propagation::Stop;
            };
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                h.key = format_key(&combo);
            }
            window.close();
            rebuild(&state);
            glib::Propagation::Stop
        });
    }
    window.add_controller(controller);
    shell.present();
}

/// Open the "Find Key" requester: press a combination, read what it does.
pub fn open_find_key(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Find Key").size(400, 160);
    let prompt = gtk::Label::new(Some("Press a key combination"));
    let result = gtk::Label::new(None);
    result.set_wrap(true);
    shell.content().append(&prompt);
    shell.content().append(&result);
    shell.close_button("Close");
    let window = shell.window().clone();

    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ui = ui.clone();
        controller.connect_key_pressed(move |_, key, _, mods| {
            let Some(combo) = crate::hotkeys::combo_from_event(key, mods) else {
                return glib::Propagation::Stop;
            };
            prompt.set_text(&format_key(&combo));
            let meaning = find_key(&ui.hub.bindings.borrow().hotkeys, &function::load_functions(), &combo);
            result.set_text(&match meaning {
                KeyMeaning::UserFunction { name, global } => {
                    format!("Runs the function \"{name}\"{}", if global { " (global hotkey)" } else { "" })
                }
                KeyMeaning::Builtin { title } => format!("Built-in: {title}"),
                KeyMeaning::Unassigned => "Not assigned".to_string(),
            });
            glib::Propagation::Stop
        });
    }
    window.add_controller(controller);
    shell.present();
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p opusdm-lister`
Expected: PASS (six `hotkey_editor` tests, `hotkey_commands_exist_in_the_palette`, and the existing suite; the exhaustive `execute_command` match compiles only with both new arms). `cargo clippy -p opusdm-lister --all-targets` shows no new warning.

- [ ] **Step 7: Commit**

```bash
printf -- '- Feature (F-03, step 5): "Edit Hotkeys..." editor (key capture, function drop-down, Global flag, inline errors, Save gated by `validate_hotkeys`) and "Find Key..." requester in the Command Palette.\n' >> CHANGES.md
git add crates/lister/src/binding_ui.rs crates/lister/src/hotkey_editor.rs crates/lister/src/keys.rs crates/lister/src/palette.rs crates/lister/src/main.rs CHANGES.md
git commit -m "feat(lister): hotkey editor and Find Key requester"
```

---

### Task 6: Global hotkeys in the hub (`XGrabKey`)

**Files:**
- Create: `crates/hub/src/global_keys.rs`
- Modify: `crates/hub/src/lib.rs`, `crates/hub/src/server.rs`, `crates/hub/src/main.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `bindings::{parse_key, Hotkey, Mods}`, `HubCmd::RunFunctionHeadless`, `ServerOptions` (Task 2).
- Produces:
  - `global_keys::{Grab, keymap_from_mapping, plan_grabs, x_modifier_mask, lock_variants, strip_locks, lookup, GlobalKeys}`.
  - `ServerOptions.global_keys: bool` (default `false`, `production()` sets `true`).
  - In `Hub`: `apply_global_keys(&mut self)`, called at startup and after every `SetBindings`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hub/src/global_keys.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::bindings::Hotkey;

    fn hk(key: &str, function: &str) -> Hotkey {
        Hotkey { key: key.into(), function: function.into(), global: true }
    }

    #[test]
    fn keymap_maps_each_keysym_to_its_lowest_keycode() {
        // Keycodes 8 and 9, two keysyms each (unshifted, shifted).
        let map = keymap_from_mapping(8, 2, &[0x75, 0x55, 0x75, 0x2b]);
        assert_eq!(map.get(&0x75), Some(&8), "the lowest keycode wins for a repeated keysym");
        assert_eq!(map.get(&0x55), Some(&8));
        assert_eq!(map.get(&0x2b), Some(&9));
    }

    #[test]
    fn keymap_skips_unused_slots() {
        let map = keymap_from_mapping(8, 2, &[0x75, 0, 0, 0]);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn modifier_masks_follow_the_x11_bits() {
        let combo = opusdm_ipc::bindings::parse_key("Ctrl+Alt+Shift+Super+u").unwrap();
        assert_eq!(x_modifier_mask(combo.mods), 0x1 | 0x4 | 0x8 | 0x40);
        let plain = opusdm_ipc::bindings::parse_key("Ctrl+u").unwrap();
        assert_eq!(x_modifier_mask(plain.mods), 0x4);
    }

    #[test]
    fn a_grab_is_made_for_every_lock_combination() {
        let variants = lock_variants(0x4);
        assert_eq!(variants.len(), 4);
        assert!(variants.contains(&0x4));
        assert!(variants.contains(&(0x4 | CAPS_LOCK_MASK)));
        assert!(variants.contains(&(0x4 | NUM_LOCK_MASK)));
        assert!(variants.contains(&(0x4 | CAPS_LOCK_MASK | NUM_LOCK_MASK)));
    }

    #[test]
    fn locks_and_buttons_are_stripped_from_an_event_state() {
        let state = 0x4 | CAPS_LOCK_MASK | NUM_LOCK_MASK | 0x100; // Ctrl + locks + button 1
        assert_eq!(strip_locks(state), 0x4);
    }

    #[test]
    fn plan_grabs_resolves_keys_and_reports_the_rest() {
        let keymap = keymap_from_mapping(30, 1, &[0x75]); // keycode 30 is `u`
        let globals = [hk("Ctrl+Alt+u", "f1"), hk("Ctrl+Alt+zzzz", "f2"), hk("Ctrl+Alt+F7", "f3"), hk("bad+", "f4")];
        let (grabs, unresolved) = plan_grabs(&globals, &keymap);
        assert_eq!(grabs, vec![Grab { keycode: 30, mods: 0x4 | 0x8, function: "f1".into() }]);
        assert_eq!(unresolved, vec!["Ctrl+Alt+zzzz".to_string(), "Ctrl+Alt+F7".to_string(), "bad+".to_string()]);
    }

    #[test]
    fn lookup_ignores_locks_and_needs_an_exact_modifier_match() {
        let mut table = std::collections::HashMap::new();
        table.insert((30u8, 0x4u16 | 0x8), "f1".to_string());
        assert_eq!(lookup(&table, 30, 0x4 | 0x8 | NUM_LOCK_MASK), Some(&"f1".to_string()));
        assert_eq!(lookup(&table, 30, 0x4), None);
        assert_eq!(lookup(&table, 31, 0x4 | 0x8), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-hub global_keys`
Expected: FAIL to compile (add `pub mod global_keys;` to `crates/hub/src/lib.rs` first; then `keymap_from_mapping` etc. are not found).

- [ ] **Step 3: Implement `global_keys.rs`**

Put this above the test module:

```rust
//! System-global hotkeys (F-03), X11 only.
//!
//! A passive `XGrabKey` on the root window makes the X server deliver a
//! configured key combination to this process whatever window has the
//! focus. One connection, created lazily, owns the grabs; a thread blocks
//! on it and forwards each hit as a function id. Wayland has no equivalent
//! and is unsupported: if the X connection cannot be opened the hub logs it
//! once and global hotkeys stay inert.

use opusdm_ipc::bindings::{parse_key, Hotkey, Mods};
use opusdm_ipc::function::FunctionId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, ConnectionExt as _, GrabMode, ModMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// X11 CapsLock modifier bit (`LockMask`).
pub const CAPS_LOCK_MASK: u16 = 1 << 1;

/// X11 NumLock modifier bit (`Mod2Mask` on a standard keyboard).
pub const NUM_LOCK_MASK: u16 = 1 << 4;

/// The modifier bits that identify a combination: Shift, Control, Mod1 (Alt)
/// and Mod4 (Super). Locks and pointer buttons are masked out.
pub const RELEVANT_MODS: u16 = 0x1 | 0x4 | 0x8 | 0x40;

/// One key grab: a keycode with a modifier mask, and the function it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grab {
    /// X keycode.
    pub keycode: u8,
    /// Modifier mask without lock bits.
    pub mods: u16,
    /// The function to run.
    pub function: FunctionId,
}

/// The X modifier mask of `mods` (no lock bits).
pub fn x_modifier_mask(mods: Mods) -> u16 {
    let mut mask = 0;
    if mods.shift {
        mask |= 0x1;
    }
    if mods.ctrl {
        mask |= 0x4;
    }
    if mods.alt {
        mask |= 0x8;
    }
    if mods.sup {
        mask |= 0x40;
    }
    mask
}

/// `base` combined with every NumLock/CapsLock state, because X matches a
/// grab against the exact modifier state including locks.
pub fn lock_variants(base: u16) -> [u16; 4] {
    [base, base | CAPS_LOCK_MASK, base | NUM_LOCK_MASK, base | CAPS_LOCK_MASK | NUM_LOCK_MASK]
}

/// `state` reduced to the modifier bits that identify a combination.
pub fn strip_locks(state: u16) -> u16 {
    state & RELEVANT_MODS
}

/// Reverse of the server's keyboard mapping: keysym to the lowest keycode
/// producing it (in any shift level). `keysyms` holds `keysyms_per_keycode`
/// entries per keycode, starting at `min_keycode`; `0` marks an unused slot.
pub fn keymap_from_mapping(min_keycode: u8, keysyms_per_keycode: u8, keysyms: &[u32]) -> HashMap<u32, u8> {
    let mut map = HashMap::new();
    let per = usize::from(keysyms_per_keycode).max(1);
    for (offset, slots) in keysyms.chunks(per).enumerate() {
        let keycode = min_keycode.saturating_add(offset as u8);
        for &keysym in slots.iter().filter(|&&s| s != 0) {
            map.entry(keysym).or_insert(keycode);
        }
    }
    map
}

/// The keysym for a GDK key name (GDK key values equal X keysyms), lower-cased
/// so a letter resolves to its unshifted keysym.
fn keysym_for(name: &str) -> Option<u32> {
    use gtk4::glib::translate::IntoGlib;
    gtk4::gdk::Key::from_name(name).map(|key| key.to_lower().into_glib())
}

/// Turn the global hotkeys into grabs. The second list holds the key texts
/// that could not be resolved (bad text, unknown key name, or a key this
/// keyboard does not have), for the log.
pub fn plan_grabs(globals: &[Hotkey], keymap: &HashMap<u32, u8>) -> (Vec<Grab>, Vec<String>) {
    let mut grabs = Vec::new();
    let mut unresolved = Vec::new();
    for hotkey in globals {
        let resolved = parse_key(&hotkey.key).ok().and_then(|combo| {
            let keycode = *keymap.get(&keysym_for(&combo.name)?)?;
            Some(Grab { keycode, mods: x_modifier_mask(combo.mods), function: hotkey.function.clone() })
        });
        match resolved {
            Some(grab) => grabs.push(grab),
            None => unresolved.push(hotkey.key.clone()),
        }
    }
    (grabs, unresolved)
}

/// The function bound to a key press, ignoring lock bits in `state`.
pub fn lookup(table: &HashMap<(u8, u16), FunctionId>, keycode: u8, state: u16) -> Option<&FunctionId> {
    table.get(&(keycode, strip_locks(state)))
}

/// The lazily created X connection, its keyboard map and the live grab table.
pub struct GlobalKeys {
    conn: Arc<RustConnection>,
    root: xproto::Window,
    keymap: HashMap<u32, u8>,
    table: Arc<Mutex<HashMap<(u8, u16), FunctionId>>>,
}

impl GlobalKeys {
    /// Connect to X11, read the keyboard map and start the thread that calls
    /// `on_key` with the function id of every grabbed key press. `None` (after
    /// one log line) when there is no X connection.
    pub fn start(on_key: Box<dyn Fn(FunctionId) + Send>) -> Option<GlobalKeys> {
        let (conn, screen_num) = match x11rb::connect(None) {
            Ok(pair) => pair,
            Err(e) => {
                opusdm_ipc::err_log!("hub", "global hotkeys unavailable (no X11 connection): {e}");
                return None;
            }
        };
        let conn = Arc::new(conn);
        let setup = conn.setup();
        let root = setup.roots[screen_num].root;
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let mapping = match conn.get_keyboard_mapping(min, max - min + 1).map_err(|e| e.to_string()).and_then(|c| c.reply().map_err(|e| e.to_string())) {
            Ok(mapping) => mapping,
            Err(e) => {
                opusdm_ipc::err_log!("hub", "global hotkeys unavailable (cannot read the keyboard map): {e}");
                return None;
            }
        };
        let keymap = keymap_from_mapping(min, mapping.keysyms_per_keycode, &mapping.keysyms);
        let table: Arc<Mutex<HashMap<(u8, u16), FunctionId>>> = Arc::new(Mutex::new(HashMap::new()));

        let event_conn = conn.clone();
        let event_table = table.clone();
        let spawned = std::thread::Builder::new().name("opusdm-global-keys".into()).spawn(move || loop {
            match event_conn.wait_for_event() {
                Ok(Event::KeyPress(ev)) => {
                    let id = {
                        let table = event_table.lock().unwrap();
                        lookup(&table, ev.detail, u16::from(ev.state)).cloned()
                    };
                    if let Some(id) = id {
                        on_key(id);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    opusdm_ipc::err_log!("hub", "global hotkey thread stopping: {e}");
                    break;
                }
            }
        });
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("hub", "cannot spawn the global hotkey thread: {e}");
            return None;
        }
        Some(GlobalKeys { conn, root, keymap, table })
    }

    /// Replace every grab with the ones `globals` asks for. A key that cannot
    /// be resolved, or that another client already grabbed (`BadAccess`), is
    /// logged and skipped; the other grabs stay active.
    pub fn set(&self, globals: &[Hotkey]) {
        // Keycode 0 is AnyKey: drop every grab this connection made.
        let _ = self.conn.ungrab_key(0u8, self.root, ModMask::ANY);
        let (grabs, unresolved) = plan_grabs(globals, &self.keymap);
        for key in unresolved {
            opusdm_ipc::err_log!("hub", "global hotkey '{key}' does not match a key on this keyboard");
        }
        let mut table = HashMap::new();
        for grab in grabs {
            let mut grabbed = false;
            for mods in lock_variants(grab.mods) {
                let result = self
                    .conn
                    .grab_key(false, self.root, ModMask::from(mods), grab.keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                    .map_err(|e| e.to_string())
                    .and_then(|cookie| cookie.check().map_err(|e| e.to_string()));
                match result {
                    Ok(()) => grabbed = true,
                    Err(e) => opusdm_ipc::err_log!(
                        "hub",
                        "cannot grab keycode {} with modifiers {mods:#x} (already taken?): {e}",
                        grab.keycode
                    ),
                }
            }
            if grabbed {
                table.insert((grab.keycode, grab.mods), grab.function);
            }
        }
        *self.table.lock().unwrap() = table;
        let _ = self.conn.flush();
    }
}
```

- [ ] **Step 4: Wire it into the server**

`server.rs`:

1. `ServerOptions` gains `pub global_keys: bool,` (doc: "Grab the global hotkeys over X11. Off in tests, which must never touch a real X server.") with `global_keys: false` in `Default` and `global_keys: true` in `production()`.
2. `Hub` gains the fields
```rust
    /// Whether global hotkeys may be grabbed (off in tests).
    global_keys_enabled: bool,
    /// Sender back into the owner loop, used by the global-key thread.
    owner_tx: Sender<OwnerMsg>,
    /// The X11 grabs, created lazily on the first non-empty global set.
    global_keys: Option<crate::global_keys::GlobalKeys>,
    /// The X11 connection failed once; do not retry or log again.
    x11_failed: bool,
```
initialised in the `Hub { ... }` literal with `global_keys_enabled: opts.global_keys, owner_tx: owner_tx.clone(), global_keys: None, x11_failed: false,` (compute `let global_keys_enabled = opts.global_keys;` before `opts.initial_bindings` is moved, or read the bool first).
3. Add the method:
```rust
    /// Bring the X11 grabs in line with the global hotkeys of
    /// `current_bindings`. The X connection is created on the first non-empty
    /// set; if it cannot be opened the hub gives up on global hotkeys for the
    /// rest of the session.
    fn apply_global_keys(&mut self) {
        if !self.global_keys_enabled {
            return;
        }
        let globals: Vec<opusdm_ipc::bindings::Hotkey> =
            self.current_bindings.hotkeys.iter().filter(|h| h.global).cloned().collect();
        if self.global_keys.is_none() {
            if globals.is_empty() || self.x11_failed {
                return;
            }
            let tx = self.owner_tx.clone();
            let started = crate::global_keys::GlobalKeys::start(Box::new(move |id| {
                let _ = tx.send(OwnerMsg::FromGtk(HubCmd::RunFunctionHeadless(id)));
            }));
            match started {
                Some(keys) => self.global_keys = Some(keys),
                None => {
                    self.x11_failed = true;
                    return;
                }
            }
        }
        if let Some(keys) = &self.global_keys {
            keys.set(&globals);
        }
    }
```
4. Call `self.apply_global_keys();` at the end of the `ToHub::SetBindings` arm, and `hub.apply_global_keys();` in `owner_loop` right after the `Hub { ... }` literal (before `for msg in rx`).

`main.rs` needs no change (`ServerOptions::production()` already sets `global_keys: true`).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p opusdm-hub`
Expected: PASS (seven `global_keys` tests, the Task 2 and 3 tests, and the rest). `cargo clippy -p opusdm-hub --all-targets` shows no new warning.

- [ ] **Step 6: Manual acceptance (Xephyr)**

Run `./xephyr-sandbox.sh`. In the sandbox: create function "beep" (External `notify-send hotkey`), bind it in "Edit Hotkeys..." to `Ctrl+Alt+u` with Global checked, Save. Focus a non-OpusDM window (for example an `xterm`) and press `Ctrl+Alt+u`: the notification appears. Then untick Global, Save, press it in the xterm: nothing happens; press it in a lister: the notification appears. Expected result recorded in the commit message body is not needed; note any deviation in `CHANGES.md`.

- [ ] **Step 7: Commit**

```bash
printf -- '- Feature (F-03, step 6): system-global hotkeys - the hub grabs them with `XGrabKey` on a lazily created X11 connection (all NumLock/CapsLock variants), runs the bound function headless, logs keys another client already owns, and stays inert without X11 (Wayland unsupported).\n' >> CHANGES.md
git add crates/hub/src/global_keys.rs crates/hub/src/lib.rs crates/hub/src/server.rs CHANGES.md
git commit -m "feat(hub): global hotkeys via XGrabKey"
```

---

### Task 7: User menu (rendering, actions, editor)

**Files:**
- Modify: `crates/ipc/src/bindings.rs` (tree operations)
- Create: `crates/lister/src/user_menu.rs`, `crates/lister/src/menu_editor.rs`
- Modify: `crates/lister/src/window.rs`, `build.rs`, `actions.rs`, `keys.rs`, `palette.rs`, `main.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `bindings::{MenuNode, prune, validate_menu, BindingError}`, `binding_ui::{function_dropdown, selected_function}`, `ToHub::RunFunctionById`.
- Produces:
  - `bindings::{children_mut, add_node, remove_node, move_node, rename_node}`.
  - `user_menu::{build_menu, build_root_menu, install}`, `menu_editor::open_menu_editor`.
  - `Ui.user_menu_btn: gtk::MenuButton`, `CommandId::ManageUserMenu`.

- [ ] **Step 1: Write the failing tests**

Append to `bindings.rs`'s test module:

```rust
    fn entry(label: &str) -> MenuNode {
        MenuNode::Entry { label: label.into(), function: "a".into() }
    }

    fn sub(label: &str, children: Vec<MenuNode>) -> MenuNode {
        MenuNode::Submenu { label: label.into(), children }
    }

    #[test]
    fn add_node_appends_at_the_root_or_inside_a_submenu() {
        let mut tree = vec![entry("a"), sub("s", vec![])];
        assert!(add_node(&mut tree, &[], entry("b")));
        assert!(add_node(&mut tree, &[1], entry("inner")));
        assert_eq!(tree.len(), 3);
        assert!(matches!(&tree[1], MenuNode::Submenu { children, .. } if children.len() == 1));
    }

    #[test]
    fn add_node_refuses_a_path_that_is_not_a_submenu() {
        let mut tree = vec![entry("a")];
        assert!(!add_node(&mut tree, &[0], entry("b")), "an entry has no children");
        assert!(!add_node(&mut tree, &[5], entry("b")), "no such node");
    }

    #[test]
    fn remove_node_deletes_by_path() {
        let mut tree = vec![entry("a"), sub("s", vec![entry("x"), entry("y")])];
        assert!(remove_node(&mut tree, &[1, 0]));
        assert!(matches!(&tree[1], MenuNode::Submenu { children, .. } if children.len() == 1));
        assert!(remove_node(&mut tree, &[0]));
        assert_eq!(tree.len(), 1);
        assert!(!remove_node(&mut tree, &[9]));
        assert!(!remove_node(&mut tree, &[]));
    }

    #[test]
    fn move_node_swaps_with_a_neighbour_and_stops_at_the_edges() {
        let mut tree = vec![entry("a"), entry("b"), entry("c")];
        assert!(move_node(&mut tree, &[1], -1));
        assert!(matches!(&tree[0], MenuNode::Entry { label, .. } if label == "b"));
        assert!(!move_node(&mut tree, &[0], -1), "already first");
        assert!(move_node(&mut tree, &[1], 1));
        assert!(matches!(&tree[2], MenuNode::Entry { label, .. } if label == "a"));
        assert!(!move_node(&mut tree, &[2], 1), "already last");
    }

    #[test]
    fn move_node_works_inside_a_submenu() {
        let mut tree = vec![sub("s", vec![entry("x"), entry("y")])];
        assert!(move_node(&mut tree, &[0, 1], -1));
        assert!(matches!(&tree[0], MenuNode::Submenu { children, .. }
            if matches!(&children[0], MenuNode::Entry { label, .. } if label == "y")));
    }

    #[test]
    fn rename_node_changes_entries_and_submenus_but_not_separators() {
        let mut tree = vec![entry("a"), sub("s", vec![]), MenuNode::Separator];
        assert!(rename_node(&mut tree, &[0], "A"));
        assert!(rename_node(&mut tree, &[1], "S"));
        assert!(!rename_node(&mut tree, &[2], "x"));
        assert!(matches!(&tree[0], MenuNode::Entry { label, .. } if label == "A"));
        assert!(matches!(&tree[1], MenuNode::Submenu { label, .. } if label == "S"));
    }
```

Create `crates/lister/src/user_menu.rs` with only this test module first (`gio::Menu` needs no display):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::gio::prelude::*;

    fn entry(label: &str, function: &str) -> MenuNode {
        MenuNode::Entry { label: label.into(), function: function.into() }
    }

    fn section_len(menu: &gio::Menu, index: i32) -> i32 {
        menu.item_link(index, "section").expect("a section").n_items()
    }

    #[test]
    fn separators_split_the_menu_into_sections() {
        let tree = vec![
            entry("a", "f1"),
            MenuNode::Separator,
            entry("b", "f2"),
            MenuNode::Submenu { label: "c".into(), children: vec![entry("d", "f3")] },
        ];
        let menu = build_menu(&tree);
        assert_eq!(menu.n_items(), 2);
        assert_eq!(section_len(&menu, 0), 1);
        assert_eq!(section_len(&menu, 1), 2);
    }

    #[test]
    fn empty_sections_and_leading_separators_leave_no_gap() {
        let tree = vec![MenuNode::Separator, MenuNode::Separator, entry("a", "f1"), MenuNode::Separator];
        let menu = build_menu(&tree);
        assert_eq!(menu.n_items(), 1);
        assert_eq!(section_len(&menu, 0), 1);
    }

    #[test]
    fn the_root_menu_always_ends_with_the_edit_section() {
        let empty = build_root_menu(&[]);
        assert_eq!(empty.n_items(), 1);
        assert_eq!(section_len(&empty, 0), 1);
        let filled = build_root_menu(&[entry("a", "f1")]);
        assert_eq!(filled.n_items(), 2);
    }

    #[test]
    fn an_entry_carries_the_run_action_and_its_function_id() {
        let menu = build_menu(&[entry("a", "fn-42")]);
        let section = menu.item_link(0, "section").unwrap();
        let action = section.item_attribute_value(0, "action", None).and_then(|v| v.get::<String>());
        let target = section.item_attribute_value(0, "target", None).and_then(|v| v.get::<String>());
        assert_eq!(action.as_deref(), Some("win.usermenu-run"));
        assert_eq!(target.as_deref(), Some("fn-42"));
    }
}
```

In `keys.rs` tests add:

```rust
    #[test]
    fn user_menu_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageUserMenu));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc bindings::tests::add_node` and `cargo test -p opusdm-lister user_menu`
Expected: FAIL to compile (add `mod user_menu;` to `main.rs` after `mod tools;`; `add_node`, `build_menu` etc. are not found).

- [ ] **Step 3: Menu tree operations (`crates/ipc/src/bindings.rs`)**

Add above `sanitize`:

```rust
/// The child list at `parent` (an empty path is the root list), or `None`
/// when the path does not lead to a submenu.
pub fn children_mut<'a>(tree: &'a mut Vec<MenuNode>, parent: &[usize]) -> Option<&'a mut Vec<MenuNode>> {
    let Some((&first, rest)) = parent.split_first() else { return Some(tree) };
    match tree.get_mut(first)? {
        MenuNode::Submenu { children, .. } => children_mut(children, rest),
        _ => None,
    }
}

/// Append `node` to the list at `parent`. `false` when `parent` is not a
/// submenu path.
pub fn add_node(tree: &mut Vec<MenuNode>, parent: &[usize], node: MenuNode) -> bool {
    let Some(list) = children_mut(tree, parent) else { return false };
    list.push(node);
    true
}

/// Remove the node at `path`. `false` when there is no such node.
pub fn remove_node(tree: &mut Vec<MenuNode>, path: &[usize]) -> bool {
    let Some((&index, parent)) = path.split_last() else { return false };
    let Some(list) = children_mut(tree, parent) else { return false };
    if index >= list.len() {
        return false;
    }
    list.remove(index);
    true
}

/// Swap the node at `path` with its neighbour `delta` places away (`-1` up,
/// `1` down). `false` when it is already at that edge or does not exist.
pub fn move_node(tree: &mut Vec<MenuNode>, path: &[usize], delta: isize) -> bool {
    let Some((&index, parent)) = path.split_last() else { return false };
    let Some(list) = children_mut(tree, parent) else { return false };
    let Some(target) = index.checked_add_signed(delta) else { return false };
    if index >= list.len() || target >= list.len() {
        return false;
    }
    list.swap(index, target);
    true
}

/// Set the label of the entry or submenu at `path`. `false` for a separator
/// or a path that does not exist.
pub fn rename_node(tree: &mut Vec<MenuNode>, path: &[usize], new_label: &str) -> bool {
    let Some((&index, parent)) = path.split_last() else { return false };
    let Some(list) = children_mut(tree, parent) else { return false };
    match list.get_mut(index) {
        Some(MenuNode::Entry { label, .. }) | Some(MenuNode::Submenu { label, .. }) => {
            *label = new_label.to_string();
            true
        }
        _ => false,
    }
}
```

- [ ] **Step 4: Menu rendering (`crates/lister/src/user_menu.rs`)**

Put this above the test module:

```rust
//! The lister's user menu (F-04): a "Menu" button whose popup is built from
//! the cached `Bindings` every time it opens, so an edit made in any Lister
//! (or a function deleted meanwhile) shows up with no explicit refresh.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, MenuNode};
use opusdm_ipc::function::{self, FunctionId};
use std::collections::HashSet;
use std::rc::Rc;

/// Window action that runs a menu entry; its parameter is the function id.
const ACTION_RUN: &str = "usermenu-run";

/// Window action that opens the menu editor.
const ACTION_EDIT: &str = "usermenu-edit";

/// The sections of `nodes`: a separator closes the current section, entries
/// and submenus go into it, empty sections are dropped.
fn build_sections(nodes: &[MenuNode]) -> Vec<gio::Menu> {
    let mut sections = Vec::new();
    let mut section = gio::Menu::new();
    for node in nodes {
        match node {
            MenuNode::Entry { label, function } => {
                let item = gio::MenuItem::new(Some(label), None);
                item.set_action_and_target_value(Some(&format!("win.{ACTION_RUN}")), Some(&function.to_variant()));
                section.append_item(&item);
            }
            MenuNode::Submenu { label, children } => {
                section.append_submenu(Some(label), &build_menu(children));
            }
            MenuNode::Separator => {
                if section.n_items() > 0 {
                    sections.push(std::mem::replace(&mut section, gio::Menu::new()));
                }
            }
        }
    }
    if section.n_items() > 0 {
        sections.push(section);
    }
    sections
}

/// A `gio::Menu` of `nodes`, one section per separator-delimited group.
pub fn build_menu(nodes: &[MenuNode]) -> gio::Menu {
    let menu = gio::Menu::new();
    for section in build_sections(nodes) {
        menu.append_section(None, &section);
    }
    menu
}

/// The full popup model: the user's menu followed by a fixed section holding
/// "Edit User Menu...", so an empty menu still offers a way to fill it.
pub fn build_root_menu(nodes: &[MenuNode]) -> gio::Menu {
    let root = build_menu(nodes);
    let edit = gio::Menu::new();
    edit.append(Some("Edit User Menu..."), Some(&format!("win.{ACTION_EDIT}")));
    root.append_section(None, &edit);
    root
}

/// The set of saved function ids.
fn known_ids() -> HashSet<FunctionId> {
    function::load_functions().into_iter().map(|f| f.id).collect()
}

/// Register the two window actions and make the "Menu" button build its
/// popup lazily from the cached bindings. Holds only a weak reference to
/// `ui`, so the button and the window do not keep each other alive.
pub fn install(ui: &Rc<Ui>) {
    let run = gio::SimpleAction::new(ACTION_RUN, Some(glib::VariantTy::STRING));
    {
        let ui = Rc::downgrade(ui);
        run.connect_activate(move |_, parameter| {
            let (Some(ui), Some(id)) = (ui.upgrade(), parameter.and_then(|p| p.get::<String>())) else { return };
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::RunFunctionById { id });
        });
    }
    ui.window.add_action(&run);

    let edit = gio::SimpleAction::new(ACTION_EDIT, None);
    {
        let ui = Rc::downgrade(ui);
        edit.connect_activate(move |_, _| {
            let Some(ui) = ui.upgrade() else { return };
            crate::menu_editor::open_menu_editor(&ui);
        });
    }
    ui.window.add_action(&edit);

    let weak = Rc::downgrade(ui);
    ui.user_menu_btn.set_create_popup_func(move |button| {
        let Some(ui) = weak.upgrade() else { return };
        let tree = bindings::prune(&ui.hub.bindings.borrow().menu, &known_ids());
        button.set_menu_model(Some(&build_root_menu(&tree)));
    });
}
```

- [ ] **Step 5: Window, button row, palette**

`window.rs`: add to `Ui` (after `function_log`):

```rust
    /// The "Menu" drop-down of the button row; its popup is built lazily by
    /// `user_menu::install`.
    pub user_menu_btn: gtk::MenuButton,
```

`build.rs`: before the `Ui { ... }` literal create the button, and put it in the literal:

```rust
    let user_menu_btn = gtk::MenuButton::new();
    user_menu_btn.set_label("Menu");
    user_menu_btn.set_tooltip_text(Some("User menu"));
    user_menu_btn.add_css_class("opus-btn");
```

(field `user_menu_btn: user_menu_btn.clone(),`), and right after the existing `crate::actions::attach(&ui);` add `crate::user_menu::install(&ui);`.

`actions.rs`, in `attach`, after `row.append(&term_btn);` add `row.append(&ui.user_menu_btn);`.

`keys.rs`: add `ManageUserMenu,` to `CommandId` (doc: "Open the user menu editor."). `palette.rs`: add the entry `CommandItem { id: CommandId::ManageUserMenu, title: Cow::Borrowed("Edit User Menu..."), category: Cow::Borrowed("Functions"), shortcut: None },` and the arm

```rust
        CommandId::ManageUserMenu => {
            crate::menu_editor::open_menu_editor(ui);
        }
```

Add `mod menu_editor;` to `main.rs` (after `mod keys;`).

- [ ] **Step 6: The editor (`crates/lister/src/menu_editor.rs`)**

```rust
//! The user menu editor (F-04): the menu tree as indented rows with move,
//! rename, remove and add controls. Modeled on
//! `crates/desktop-config/src/menu_ui.rs`, editing functions instead of apps.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, BindingError, MenuNode};
use opusdm_ipc::function::{self, Function, FunctionId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// Everything the editor's callbacks share.
struct State {
    tree: RefCell<Vec<MenuNode>>,
    functions: Vec<Function>,
    body: gtk::Box,
    save_btn: gtk::Button,
    window: gtk::Window,
    ui: Rc<Ui>,
}

/// Open the "Edit User Menu" window, working on a copy of the Lister's cached
/// menu. Save sends only the menu section back to the hub.
pub fn open_menu_editor(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit User Menu").size(600, 480).scrolled();
    let root = shell.content();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.append(&body);

    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(State {
        tree: RefCell::new(ui.hub.bindings.borrow().menu.clone()),
        functions: function::load_functions(),
        body,
        save_btn: save_btn.clone(),
        window: shell.window().clone(),
        ui: ui.clone(),
    });

    root.prepend(&add_buttons(&state, Vec::new()));

    {
        let state = state.clone();
        save_btn.connect_clicked(move |_| {
            let mut updated = state.ui.hub.bindings.borrow().clone();
            updated.menu = state.tree.borrow().clone();
            let _ = state.ui.hub.client.send(&opusdm_ipc::ToHub::SetBindings(updated));
            crate::actions::flash(&state.ui, "user menu saved");
        });
    }

    rebuild(&state);
    shell.present();
}

/// The set of saved function ids.
fn known_ids(functions: &[Function]) -> HashSet<FunctionId> {
    functions.iter().map(|f| f.id.clone()).collect()
}

/// Repaint the whole tree and re-run the validation that gates Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.body.first_child() {
        state.body.remove(&child);
    }
    let tree = state.tree.borrow().clone();
    let errors = bindings::validate_menu(&tree, &known_ids(&state.functions));
    build_level(state, &tree, &mut Vec::new(), &errors);
    state.save_btn.set_sensitive(errors.is_empty());
}

/// Append the rows of `nodes` (and, depth first, their children); `path` is
/// the position of `nodes` in the tree.
fn build_level(state: &Rc<State>, nodes: &[MenuNode], path: &mut Vec<usize>, errors: &[BindingError]) {
    for (index, node) in nodes.iter().enumerate() {
        path.push(index);
        state.body.append(&build_row(state, node, path, errors));
        if let MenuNode::Submenu { children, .. } = node {
            build_level(state, children, path, errors);
        }
        path.pop();
    }
}

/// One row: an indented description, move/rename/remove buttons, add buttons
/// for a submenu, and this node's errors underneath.
fn build_row(state: &Rc<State>, node: &MenuNode, path: &[usize], errors: &[BindingError]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.set_margin_start(18 * (path.len() as i32 - 1));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let text = match node {
        MenuNode::Entry { label, function } => {
            let target = state
                .functions
                .iter()
                .find(|f| &f.id == function)
                .map(|f| f.name.as_str())
                .unwrap_or("(missing function)");
            format!("{label}  ->  {target}")
        }
        MenuNode::Submenu { label, .. } => format!("{label}  (submenu)"),
        MenuNode::Separator => "--------".to_string(),
    };
    let label = gtk::Label::new(Some(&text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    line.append(&label);

    if !matches!(node, MenuNode::Separator) {
        line.append(&rename_button(state, node, path));
    }
    line.append(&tree_button(state, "go-up-symbolic", path, |t, p| bindings::move_node(t, p, -1)));
    line.append(&tree_button(state, "go-down-symbolic", path, |t, p| bindings::move_node(t, p, 1)));
    line.append(&tree_button(state, "list-remove-symbolic", path, bindings::remove_node));
    if matches!(node, MenuNode::Submenu { .. }) {
        line.append(&add_buttons(state, path.to_vec()));
    }
    row.append(&line);

    for error in errors.iter().filter(|e| e.menu_path() == Some(path)) {
        let note = gtk::Label::new(Some(&error.to_string()));
        note.set_xalign(0.0);
        note.add_css_class("error");
        row.append(&note);
    }
    row
}

/// A flat icon button that applies `action` to the node at `path`, then
/// repaints when it changed anything.
fn tree_button(
    state: &Rc<State>,
    icon: &str,
    path: &[usize],
    action: impl Fn(&mut Vec<MenuNode>, &[usize]) -> bool + 'static,
) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("flat");
    let state = state.clone();
    let path = path.to_vec();
    button.connect_clicked(move |_| {
        let changed = action(&mut state.tree.borrow_mut(), &path);
        if changed {
            rebuild(&state);
        }
    });
    button
}

/// The rename button of an entry or submenu row.
fn rename_button(state: &Rc<State>, node: &MenuNode, path: &[usize]) -> gtk::Button {
    let current = match node {
        MenuNode::Entry { label, .. } | MenuNode::Submenu { label, .. } => label.clone(),
        MenuNode::Separator => String::new(),
    };
    let button = gtk::Button::from_icon_name("document-edit-symbolic");
    button.add_css_class("flat");
    let state = state.clone();
    let path = path.to_vec();
    button.connect_clicked(move |_| {
        let state = state.clone();
        let path = path.clone();
        opusdm_ui::requester_entry(&state.window.clone(), "Rename", &current, &[("Rename", true), ("Cancel", false)], move |ok, text| {
            if !ok || text.trim().is_empty() {
                return;
            }
            bindings::rename_node(&mut state.tree.borrow_mut(), &path, text.trim());
            rebuild(&state);
        });
    });
    button
}

/// "+ Entry", "+ Submenu" and "+ Separator" buttons that add into the list at
/// `parent` (an empty path is the root list).
fn add_buttons(state: &Rc<State>, parent: Vec<usize>) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let entry_btn = gtk::Button::with_label("+ Entry");
    let submenu_btn = gtk::Button::with_label("+ Submenu");
    let separator_btn = gtk::Button::with_label("+ Separator");
    for button in [&entry_btn, &submenu_btn, &separator_btn] {
        button.add_css_class("flat");
        bar.append(button);
    }
    {
        let state = state.clone();
        let parent = parent.clone();
        entry_btn.connect_clicked(move |_| prompt_add_entry(&state, parent.clone()));
    }
    {
        let state = state.clone();
        let parent = parent.clone();
        submenu_btn.connect_clicked(move |_| {
            let state = state.clone();
            let parent = parent.clone();
            opusdm_ui::requester_entry(&state.window.clone(), "New Submenu", "", &[("Create", true), ("Cancel", false)], move |ok, text| {
                if !ok || text.trim().is_empty() {
                    return;
                }
                let node = MenuNode::Submenu { label: text.trim().to_string(), children: Vec::new() };
                bindings::add_node(&mut state.tree.borrow_mut(), &parent, node);
                rebuild(&state);
            });
        });
    }
    {
        let state = state.clone();
        separator_btn.connect_clicked(move |_| {
            bindings::add_node(&mut state.tree.borrow_mut(), &parent, MenuNode::Separator);
            rebuild(&state);
        });
    }
    bar
}

/// Ask for a function and a label, then add an entry to the list at
/// `parent`. The label starts as the chosen function's name and follows the
/// drop-down until the user edits it.
fn prompt_add_entry(state: &Rc<State>, parent: Vec<usize>) {
    if state.functions.is_empty() {
        crate::actions::flash(&state.ui, "create a function first (Manage Functions...)");
        return;
    }
    let shell = opusdm_ui::DialogShell::new(&state.window, "Add Menu Entry");
    let dropdown = crate::binding_ui::function_dropdown(&state.functions, &state.functions[0].id);
    let label_entry = gtk::Entry::new();
    label_entry.set_text(&state.functions[0].name);
    shell.content().append(&dropdown);
    shell.content().append(&label_entry);
    {
        let functions = state.functions.clone();
        let label_entry = label_entry.clone();
        dropdown.connect_selected_notify(move |d| {
            if let Some(f) = functions.get(d.selected() as usize) {
                label_entry.set_text(&f.name);
            }
        });
    }
    let add_btn = shell.button("Add");
    shell.close_button("Cancel");
    let window = shell.window().clone();
    let state = state.clone();
    add_btn.connect_clicked(move |_| {
        let label = label_entry.text().trim().to_string();
        let Some(function) = crate::binding_ui::selected_function(&dropdown, &state.functions) else { return };
        if label.is_empty() {
            return;
        }
        bindings::add_node(&mut state.tree.borrow_mut(), &parent, MenuNode::Entry { label, function });
        rebuild(&state);
        window.destroy();
    });
    shell.present();
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p opusdm-ipc && cargo test -p opusdm-lister`
Expected: PASS (six new tree tests, four `user_menu` tests, `user_menu_command_exists_in_the_palette`, the rest). `cargo clippy -p opusdm-ipc -p opusdm-lister --all-targets` shows no new warning.

- [ ] **Step 8: Manual acceptance (Xephyr)**

In `./xephyr-sandbox.sh`, with two listers open: "Menu" > "Edit User Menu...", add a submenu "Tools", add entries into it (function drop-down), a separator and a top-level entry, Save. Both listers' "Menu" buttons show the nested tree; running an entry runs the function with the clicked lister as SOURCE. Delete a function in "Manage Functions...": its entry disappears from the popup without a restart and shows as an error in the editor.

- [ ] **Step 9: Commit**

```bash
printf -- '- Feature (F-04, step 7): user menu - a "Menu" drop-down in the lister button row (popup built lazily from the cached bindings, entries run functions, "Edit User Menu..." always present) and a tree editor with add/rename/move/remove; tree operations live in `opusdm_ipc::bindings`.\n' >> CHANGES.md
git add crates/ipc/src/bindings.rs crates/lister/src/user_menu.rs crates/lister/src/menu_editor.rs crates/lister/src/window.rs crates/lister/src/build.rs crates/lister/src/actions.rs crates/lister/src/keys.rs crates/lister/src/palette.rs crates/lister/src/main.rs CHANGES.md
git commit -m "feat(lister): user menu with editor"
```

---

### Task 8: Script events (emission, guards, editor)

**Files:**
- Create: `crates/hub/src/script_events.rs`, `crates/lister/src/scripts_editor.rs`
- Modify: `crates/hub/src/lib.rs`, `server.rs`, `main.rs`, `toolbar.rs`
- Modify: `crates/lister/src/keys.rs`, `palette.rs`, `main.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `bindings::{ScriptEvent, ScriptBinding, validate_scripts}`, `Hub::run_headless` (Task 3), `crate::functions::{spawn_executor, FnEvent}`.
- Produces:
  - `script_events::{EVENT_COOLDOWN, INFLIGHT_MAX_AGE, SHUTDOWN_HOOK_TIMEOUT, EventGuard}` with `EventGuard::{new, try_begin(&mut self, ScriptEvent, Instant) -> bool, add_run(&mut self, ScriptEvent, u64, Instant), finish(&mut self, u64)}`.
  - `HubCmd::EmitScriptEvent(ScriptEvent)`.
  - In `Hub`: `emit_script_event`, `run_shutdown_hooks`, `drop_lister`, `event_guard` field; `drop_conn` returns `bool`.
  - `CommandId::ManageScripts`, `scripts_editor::open_scripts_editor`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hub/src/script_events.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_first_emission_of_an_event_is_allowed() {
        let mut guard = EventGuard::new();
        assert!(guard.try_begin(ScriptEvent::Startup, Instant::now()));
    }

    #[test]
    fn a_second_emission_inside_the_cooldown_is_dropped() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        assert!(!guard.try_begin(ScriptEvent::ListerOpened, t0 + EVENT_COOLDOWN - Duration::from_millis(1)));
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0 + EVENT_COOLDOWN));
    }

    #[test]
    fn other_events_are_not_throttled_by_one_event() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        assert!(guard.try_begin(ScriptEvent::ListerClosed, t0));
    }

    #[test]
    fn a_run_still_in_flight_blocks_the_next_emission_after_the_cooldown() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        guard.add_run(ScriptEvent::ListerOpened, 7, t0);
        let later = t0 + EVENT_COOLDOWN + Duration::from_secs(1);
        assert!(!guard.try_begin(ScriptEvent::ListerOpened, later), "run 7 has not finished");
        guard.finish(7);
        assert!(guard.try_begin(ScriptEvent::ListerOpened, later));
    }

    #[test]
    fn a_leaked_run_stops_blocking_after_the_max_age() {
        let mut guard = EventGuard::new();
        let t0 = Instant::now();
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0));
        guard.add_run(ScriptEvent::ListerOpened, 7, t0);
        assert!(guard.try_begin(ScriptEvent::ListerOpened, t0 + INFLIGHT_MAX_AGE + Duration::from_secs(1)));
    }

    #[test]
    fn finishing_an_unknown_run_is_harmless() {
        let mut guard = EventGuard::new();
        guard.finish(99);
    }
}
```

Add to the `server.rs` tests module (it reuses `marker_function`, `wait_for_lines` from Task 3):

```rust
    /// Server options for an event test: these bindings, no disk read.
    fn with_bindings(bindings: opusdm_ipc::Bindings) -> ServerOptions {
        ServerOptions { initial_bindings: Some(bindings), ..ServerOptions::default() }
    }

    fn script(event: opusdm_ipc::bindings::ScriptEvent, function: &str) -> opusdm_ipc::bindings::ScriptBinding {
        opusdm_ipc::bindings::ScriptBinding { event, function: function.into() }
    }

    #[test]
    fn startup_event_runs_the_bound_function() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, _path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Startup, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));
        handle.cmd_tx.send(HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup)).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn an_event_runs_only_the_functions_bound_to_it() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, _path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerClosed, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));
        handle.cmd_tx.send(HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "a Startup event must not run a ListerClosed hook");
    }

    #[test]
    fn lister_opened_runs_once_and_is_throttled_inside_the_cooldown() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerOpened, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        wait_for_lines(&marker, 1);

        // A second lister right away is the loop a self-spawning hook would
        // create: it must be throttled, not run again.
        let mut b = connect(&path);
        send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into() }).unwrap();
        let _w: Welcome = recv(&mut b).unwrap().unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let lines = std::fs::read_to_string(&marker).unwrap().lines().count();
        assert_eq!(lines, 1, "the second ListerOpened inside the cooldown must be dropped");
    }

    #[test]
    fn lister_closed_runs_the_bound_function() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerClosed, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();
        send(&mut a, &ToHub::Goodbye).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn shutdown_hooks_finish_before_the_listers_are_told_to_quit() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");
        let f = marker_function("hook", &marker);
        opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

        let (_td, listener, path) = bind_temp();
        let bindings = opusdm_ipc::Bindings {
            scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Shutdown, &f.id)],
            ..opusdm_ipc::Bindings::default()
        };
        let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        handle.cmd_tx.send(HubCmd::Shutdown).unwrap();
        let got: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(got, ToLister::Shutdown), "got {got:?}");
        let lines = std::fs::read_to_string(&marker).map(|s| s.lines().count()).unwrap_or(0);
        assert_eq!(lines, 1, "the Shutdown hook must have finished before the Shutdown message");
    }
```

In `keys.rs` tests add:

```rust
    #[test]
    fn script_events_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageScripts));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-hub script_events startup_event lister_opened lister_closed shutdown_hooks`
Expected: FAIL to compile (add `pub mod script_events;` to `crates/hub/src/lib.rs`; `EventGuard`, `HubCmd::EmitScriptEvent` not found).

- [ ] **Step 3: The guard (`crates/hub/src/script_events.rs`)**

Put this above the test module:

```rust
//! Loop protection for script events (F-05). A hook on `ListerOpened` that
//! itself opens a lister would fire `ListerOpened` again, forever. Two
//! guards break the loop: an event is dropped while runs of its previous
//! emission are still going, and the same event is never emitted twice
//! within [`EVENT_COOLDOWN`].

pub use opusdm_ipc::bindings::ScriptEvent;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Minimum time between two emissions of the same event.
pub const EVENT_COOLDOWN: Duration = Duration::from_secs(2);

/// A run still "in flight" after this long is treated as finished, so a run
/// that leaked (its Lister vanished) cannot block an event forever.
pub const INFLIGHT_MAX_AGE: Duration = Duration::from_secs(60);

/// How long the hub waits for `Shutdown` hooks before quitting anyway.
pub const SHUTDOWN_HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// Tracks the last emission and the in-flight runs of every event.
#[derive(Default)]
pub struct EventGuard {
    last: HashMap<ScriptEvent, Instant>,
    inflight: HashMap<u64, (ScriptEvent, Instant)>,
}

impl EventGuard {
    /// An empty guard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `event` may be emitted at `now`. On `true` the emission is
    /// recorded (it starts the cooldown). `false` when the previous emission
    /// is younger than [`EVENT_COOLDOWN`] or one of its runs is still in
    /// flight and younger than [`INFLIGHT_MAX_AGE`].
    pub fn try_begin(&mut self, event: ScriptEvent, now: Instant) -> bool {
        if let Some(&last) = self.last.get(&event) {
            if now.saturating_duration_since(last) < EVENT_COOLDOWN {
                return false;
            }
        }
        let busy = self
            .inflight
            .values()
            .any(|&(e, started)| e == event && now.saturating_duration_since(started) < INFLIGHT_MAX_AGE);
        if busy {
            return false;
        }
        self.last.insert(event, now);
        true
    }

    /// Record that run `run_id` was started for `event` at `now`.
    pub fn add_run(&mut self, event: ScriptEvent, run_id: u64, now: Instant) {
        self.inflight.insert(run_id, (event, now));
    }

    /// Record that run `run_id` ended (or can never complete).
    pub fn finish(&mut self, run_id: u64) {
        self.inflight.remove(&run_id);
    }
}
```

- [ ] **Step 4: Hub emission (`server.rs`)**

1. Imports: `use crate::script_events::{EventGuard, ScriptEvent, SHUTDOWN_HOOK_TIMEOUT};`, `use opusdm_ipc::function::FunctionId;` (extend the existing `function` import), `use std::time::Instant;`.
2. `HubCmd` gains `EmitScriptEvent(ScriptEvent),` (doc: "Emit a script event: run every function bound to it (from `main` for `Startup`, from the desktop for the double clicks).").
3. `Hub` gains `event_guard: EventGuard,` (literal: `event_guard: EventGuard::new(),`).
4. `drop_conn` returns `bool`: change its signature to `-> bool`; `let Some(c) = conns.remove(&cid) else { return false };`, `if !c.registered { return false; }`, and `true` as the last expression (after the `AllListersGone` block).
5. Add to `impl Hub`:

```rust
    /// Drop Lister connection `cid`: forget its Function-run state, tear the
    /// connection down and, when a registered Lister went away, emit
    /// `ListerClosed`.
    fn drop_lister(&mut self, cid: u64) {
        self.drop_function_state(cid);
        let was_registered = drop_conn(cid, &mut self.conns, &mut self.free_conns, &mut self.roles, &self.evt_tx);
        if was_registered {
            self.emit_script_event(ScriptEvent::ListerClosed);
        }
    }

    /// The functions bound to `event`, in binding order.
    fn functions_for(&self, event: ScriptEvent) -> Vec<FunctionId> {
        self.current_bindings
            .scripts
            .iter()
            .filter(|s| s.event == event)
            .map(|s| s.function.clone())
            .collect()
    }

    /// Run every function bound to `event` headless. The functions start in
    /// binding order and are not awaited one after another. The emission is
    /// dropped (and logged) by [`EventGuard`] when the same event fired
    /// under `EVENT_COOLDOWN` ago or its previous runs are still going.
    fn emit_script_event(&mut self, event: ScriptEvent) {
        let functions = self.functions_for(event);
        if functions.is_empty() {
            return;
        }
        let now = Instant::now();
        if !self.event_guard.try_begin(event, now) {
            opusdm_ipc::err_log!("hub", "script event {event:?} dropped: it fired again too soon or its previous run is still going");
            return;
        }
        for id in functions {
            if let Some(run_id) = self.run_headless(&id) {
                self.event_guard.add_run(event, run_id, now);
            }
        }
    }

    /// Run the `Shutdown` hooks and wait for them, up to
    /// `SHUTDOWN_HOOK_TIMEOUT`. The Listers are closing, so there is no
    /// `QuerySelection`: each hook runs with an empty selection in `$HOME`.
    fn run_shutdown_hooks(&mut self) {
        let ids = self.functions_for(ScriptEvent::Shutdown);
        if ids.is_empty() {
            return;
        }
        let saved = opusdm_ipc::function::load_functions();
        let deadline = Instant::now() + SHUTDOWN_HOOK_TIMEOUT;
        let mut receivers = Vec::new();
        for id in ids {
            let Some(function) = saved.iter().find(|f| f.id == id) else {
                opusdm_ipc::err_log!("hub", "shutdown hook: function '{id}' not found");
                continue;
            };
            if !opusdm_ipc::function::validate(function).is_empty() {
                opusdm_ipc::err_log!("hub", "shutdown hook: function '{}' is invalid", function.name);
                continue;
            }
            let run_id = self.next_run_id;
            self.next_run_id += 1;
            let (tx, rx) = channel();
            let empty = SelectionCtx { paths: Vec::new(), cwd: home_dir() };
            crate::functions::spawn_executor(run_id, function.steps.clone(), empty, None, tx);
            receivers.push(rx);
        }
        for rx in receivers {
            loop {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    opusdm_ipc::err_log!("hub", "shutdown hooks did not finish within {SHUTDOWN_HOOK_TIMEOUT:?}");
                    return;
                };
                match rx.recv_timeout(left) {
                    Ok((_, crate::functions::FnEvent::AllDone)) => break,
                    Ok((_, crate::functions::FnEvent::Failed { step_idx, error })) => {
                        opusdm_ipc::err_log!("hub", "shutdown hook failed at step {step_idx}: {error}");
                        break;
                    }
                    Ok(_) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        opusdm_ipc::err_log!("hub", "shutdown hooks did not finish within {SHUTDOWN_HOOK_TIMEOUT:?}");
                        return;
                    }
                }
            }
        }
    }
```

6. Replace the three `drop_function_state(cid); drop_conn(...)` pairs with `self.drop_lister(cid);` (`Goodbye` arm in `lister_msg`, `ChildExited` arm in `gtk_cmd`) and `hub.drop_lister(cid);` (`OwnerMsg::ListerEof` arm). The `ChildExited` arm keeps its `hit` lookup and calls `self.drop_lister(cid)` inside `if let Some(cid) = hit`.
7. `Register` arm: after `emit_roles(&self.roles, &self.conns, &self.evt_tx);` (inside the `if let Some(c)` block, so borrow `c` has ended) call `self.emit_script_event(ScriptEvent::ListerOpened);` after the `if let` block instead, to avoid the borrow: place it as the last statement of the arm, after the closing brace of `if let Some(c) = self.conns.get_mut(&cid) { ... }`, guarded by `if self.conns.get(&cid).is_some_and(|c| c.registered)`.
8. `gtk_cmd`: add the arm `HubCmd::EmitScriptEvent(event) => self.emit_script_event(event),` and, at the top of the `HubCmd::Shutdown` arm (before the `for c in ... Shutdown` loop), `self.run_shutdown_hooks();`.
9. Guard bookkeeping: in `function_event`, right after `if terminal { self.function_requesters.remove(&run_id); }` add `if terminal { self.event_guard.finish(run_id); }` (merge into the same `if`); in `drop_function_state`, inside `for run_id in doomed { ... }` add `self.event_guard.finish(run_id);`.

`main.rs`: after `let handle = server::spawn_server_with(...)` and before `children::install_sigchld(...)`, add:

```rust
    let _ = handle.cmd_tx.send(server::HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup));
```

- [ ] **Step 5: Desktop events (`crates/hub/src/toolbar.rs`)**

Add the import `use opusdm_ipc::bindings::ScriptEvent;` next to the other `opusdm_ipc` imports. In `wire_primary_icon_field` change the background-activate closure to:

```rust
        field.connect_background_activate(move || {
            let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: None });
            let _ = cmd_tx.send(HubCmd::EmitScriptEvent(ScriptEvent::DesktopDoubleClick));
        });
```

Add this function after `popup_at`:

```rust
/// Emit `DesktopMiddleDoubleClick` and `DesktopRightDoubleClick` for double
/// clicks on the desktop overlay. The gestures run in the capture phase and
/// never claim the sequence, so the icon field and the context menu keep
/// working. The right button already opens the context menu on its first
/// press, so a right double click also shows the menu (accepted).
fn attach_desktop_double_click_events(overlay: &gtk::Overlay, cmd_tx: &std::sync::mpsc::Sender<HubCmd>) {
    for (button, event) in [
        (gtk::gdk::BUTTON_MIDDLE, ScriptEvent::DesktopMiddleDoubleClick),
        (gtk::gdk::BUTTON_SECONDARY, ScriptEvent::DesktopRightDoubleClick),
    ] {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(button);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let cmd_tx = cmd_tx.clone();
        gesture.connect_pressed(move |_, n_press, _, _| {
            if n_press == 2 {
                let _ = cmd_tx.send(HubCmd::EmitScriptEvent(event));
            }
        });
        overlay.add_controller(gesture);
    }
}
```

and call `attach_desktop_double_click_events(overlay, &widgets.cmd_tx);` as the first statement of `attach_desktop_menu`.

- [ ] **Step 6: Palette and editor**

`keys.rs`: add `ManageScripts,` to `CommandId` (doc: "Open the script events editor."). `palette.rs`: add the entry `CommandItem { id: CommandId::ManageScripts, title: Cow::Borrowed("Edit Script Events..."), category: Cow::Borrowed("Functions"), shortcut: None },` and the arm `CommandId::ManageScripts => { crate::scripts_editor::open_scripts_editor(ui); }`. Add `mod scripts_editor;` to `main.rs` (after `mod preview;`).

Create `crates/lister/src/scripts_editor.rs`:

```rust
//! The script events editor (F-05): one row per binding, an event
//! drop-down and a function drop-down, with live validation.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::bindings::{self, BindingError, ScriptBinding, ScriptEvent};
use opusdm_ipc::function::{self, Function, FunctionId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// Everything the editor's callbacks share.
struct State {
    scripts: RefCell<Vec<ScriptBinding>>,
    functions: Vec<Function>,
    events: Vec<ScriptEvent>,
    rows_box: gtk::Box,
    save_btn: gtk::Button,
}

/// Open the "Edit Script Events" window, working on a copy of the Lister's
/// cached script bindings. Save sends only that section back to the hub.
pub fn open_scripts_editor(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit Script Events").size(600, 380).scrolled();
    let root = shell.content();

    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.append(&rows_box);
    let add_btn = gtk::Button::with_label("Add Binding");
    add_btn.set_halign(gtk::Align::Start);
    root.append(&add_btn);

    let save_btn = shell.button("Save");
    shell.button_spacer();
    shell.close_button("Close");

    let functions = function::load_functions();
    let state = Rc::new(State {
        scripts: RefCell::new(ui.hub.bindings.borrow().scripts.clone()),
        functions,
        events: ScriptEvent::emitted(),
        rows_box,
        save_btn: save_btn.clone(),
    });

    {
        let state = state.clone();
        add_btn.connect_clicked(move |_| {
            let Some(first) = state.functions.first() else { return };
            state.scripts.borrow_mut().push(ScriptBinding { event: state.events[0], function: first.id.clone() });
            rebuild(&state);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        save_btn.connect_clicked(move |_| {
            let mut updated = ui.hub.bindings.borrow().clone();
            updated.scripts = state.scripts.borrow().clone();
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::SetBindings(updated));
            crate::actions::flash(&ui, "script events saved");
        });
    }

    rebuild(&state);
    shell.present();
}

/// The set of saved function ids.
fn known_ids(functions: &[Function]) -> HashSet<FunctionId> {
    functions.iter().map(|f| f.id.clone()).collect()
}

/// Repaint every row and re-run the validation that gates Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.rows_box.first_child() {
        state.rows_box.remove(&child);
    }
    let scripts = state.scripts.borrow().clone();
    let errors = bindings::validate_scripts(&scripts, &known_ids(&state.functions));
    for (index, binding) in scripts.iter().enumerate() {
        state.rows_box.append(&build_row(state, index, binding, &errors));
    }
    state.save_btn.set_sensitive(errors.is_empty());
}

/// One row: event drop-down, function drop-down, remove button, errors below.
fn build_row(state: &Rc<State>, index: usize, binding: &ScriptBinding, errors: &[BindingError]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let labels: Vec<&str> = state.events.iter().map(|e| e.label()).collect();
    let event_dropdown = gtk::DropDown::from_strings(&labels);
    event_dropdown.set_selected(
        state.events.iter().position(|e| *e == binding.event).map(|p| p as u32).unwrap_or(gtk::INVALID_LIST_POSITION),
    );
    {
        let state = state.clone();
        event_dropdown.connect_selected_notify(move |d| {
            let Some(event) = state.events.get(d.selected() as usize).copied() else { return };
            if let Some(b) = state.scripts.borrow_mut().get_mut(index) {
                if b.event == event {
                    return;
                }
                b.event = event;
            }
            rebuild(&state);
        });
    }
    line.append(&event_dropdown);

    let function_dropdown = crate::binding_ui::function_dropdown(&state.functions, &binding.function);
    function_dropdown.set_hexpand(true);
    {
        let state = state.clone();
        function_dropdown.connect_selected_notify(move |d| {
            let Some(id) = crate::binding_ui::selected_function(d, &state.functions) else { return };
            if let Some(b) = state.scripts.borrow_mut().get_mut(index) {
                if b.function == id {
                    return;
                }
                b.function = id;
            }
            rebuild(&state);
        });
    }
    line.append(&function_dropdown);

    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
    remove.add_css_class("flat");
    {
        let state = state.clone();
        remove.connect_clicked(move |_| {
            let mut scripts = state.scripts.borrow_mut();
            if index < scripts.len() {
                scripts.remove(index);
            }
            drop(scripts);
            rebuild(&state);
        });
    }
    line.append(&remove);
    row.append(&line);

    for error in errors.iter().filter(|e| e.script_index() == Some(index)) {
        let note = gtk::Label::new(Some(&error.to_string()));
        note.set_xalign(0.0);
        note.set_margin_start(6);
        note.add_css_class("error");
        row.append(&note);
    }
    row
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p opusdm-hub && cargo test -p opusdm-lister`
Expected: PASS (six guard tests, five hub event tests, `script_events_command_exists_in_the_palette`, and every earlier test). `cargo clippy -p opusdm-hub -p opusdm-lister --all-targets` shows no new warning.

- [ ] **Step 8: Manual acceptance (Xephyr)**

In `./xephyr-sandbox.sh`: bind a non-recursive function (External `notify-send opened`) to "Lister opened" in "Edit Script Events...": opening a lister shows the notification. Bind an External step that starts a lister (`opusdm-lister /tmp`) to "Lister opened": opening one lister starts at most one extra lister per 2 s window instead of an unbounded loop (the log shows `script event ListerOpened dropped`). Bind functions to "Startup" and "Shutdown", restart the hub, check both ran. Double click the desktop background with the middle and right buttons, bound to a `notify-send` function: both fire (the right one also shows the context menu, as documented).

- [ ] **Step 9: Commit**

```bash
printf -- '- Feature (F-05, step 8): script events - the hub emits Startup, Shutdown (waits up to 10 s for the hooks), ListerOpened, ListerClosed and the three desktop double clicks, runs the bound functions headless, throttles loops with an in-flight guard plus a 2 s cooldown, and "Edit Script Events..." edits the bindings.\n' >> CHANGES.md
git add crates/hub/src/script_events.rs crates/hub/src/lib.rs crates/hub/src/server.rs crates/hub/src/main.rs crates/hub/src/toolbar.rs crates/lister/src/scripts_editor.rs crates/lister/src/keys.rs crates/lister/src/palette.rs crates/lister/src/main.rs CHANGES.md
git commit -m "feat: script events with loop guards and an editor"
```

---

### Task 9: Documentation and roadmap

**Files:**
- Modify: `docs/usage.md`, `TODO.md`, `CHANGES.md`

**Interfaces:**
- Consumes: the behaviour built in Tasks 1 to 8.
- Produces: user documentation and a ticked roadmap.

- [ ] **Step 1: Document the features in `docs/usage.md`**

Insert three subsections after `### 3.12 Desktop Configuration` (before `## 4. Keyboard Shortcuts Reference`), and add a pointer row to section 4's shortcut tables if a "Everything else" table exists:

```markdown
### 3.13 User Hotkeys

Any saved function (see Manage Functions) can be bound to a key. Open the
Command Palette and choose **Edit Hotkeys...**. Each row has a key (click it,
then press the combination; Escape cancels), the function to run and a
**Global** check box. Rules:

- A user hotkey wins over a built-in binding; the editor notes when it does.
- Two hotkeys cannot share a key, whatever their Global flag: the row shows an
  error and Save stays disabled.
- While a text field has focus, a user hotkey only fires if it has Ctrl, Alt
  or Super, or is a function key, so a plain letter never steals a character.
- A **Global** hotkey works while any window has focus. It needs at least one
  of Ctrl, Alt or Super. It uses the X11 key grab, so it needs an X11 session:
  on Wayland, global hotkeys are not supported (local ones work everywhere).
  A key another program already owns cannot be grabbed and is logged.
- A global hotkey runs the function on the current SOURCE lister's selection;
  with no lister open the selection is empty.

**Find Key...** (Command Palette) asks for a key combination and tells you
what it does: a function, a built-in command, or nothing.

Bindings are stored in `~/.config/opusdm/hotkeys.json`.

### 3.14 User Menu

The **Menu** drop-down in the lister button row shows your own menu. Choose
**Edit User Menu...** at the bottom of it (or the Command Palette entry of the
same name) to add entries (each runs a saved function), submenus and
separators, rename and reorder them. Changes show up in every open lister
without a restart, and an entry whose function was deleted disappears from
the menu (the editor marks it). Stored in `~/.config/opusdm/usermenu.json`.

### 3.15 Script Events

**Edit Script Events...** (Command Palette) runs saved functions on system
events: `Startup`, `Shutdown` (the hub waits up to 10 seconds for these to
finish), `Lister opened`, `Lister closed`, and double clicks on the desktop
background with the primary, middle and right buttons. Functions started by an
event run without a lister asking for them: they use the current SOURCE
lister's selection, or an empty selection in your home directory.

To stop a hook that opens a lister from looping, an event is ignored while its
previous run is still going and never fires twice within 2 seconds. Stored in
`~/.config/opusdm/scripts.json`.
```

- [ ] **Step 2: Tick the roadmap in `TODO.md`**

Make these exact replacements:

- `- [ ] F-03 Hotkey Editor (local + system-global hotkeys, KeyFinder)` becomes `- [x] F-03 Hotkey Editor (local + system-global hotkeys, KeyFinder)`
- `- [ ] F-04 User menus in listers` becomes `- [x] F-04 User menus in listers`
- `- [ ] F-05 Script events (event hooks, viewer overrides)` becomes `- [x] F-05 Script events (system events only; viewer overrides wait for F-16, Hidden/Revealed for F-38)`
- `  - [ ] F-01 follow-up: per-function hotkey (needs F-03)` becomes `  - [x] F-01 follow-up: per-function hotkey (done through hotkeys.json, F-03)`
- `2. [ ] Hotkeys + user menus + scripts (F-03, F-04, F-05)` becomes `2. [x] Hotkeys + user menus + scripts (F-03, F-04, F-05)`

- [ ] **Step 3: Verify the whole workspace**

Run: `cargo test -p opusdm-ipc -p opusdm-hub -p opusdm-lister && cargo build --workspace`
Expected: PASS and a clean build.

- [ ] **Step 4: Commit**

```bash
printf -- '- Docs: documented hotkeys, the user menu and script events in `docs/usage.md` (sections 3.13 to 3.15) and ticked F-03, F-04, F-05 and item 2 of the suggested order in `TODO.md`.\n' >> CHANGES.md
git add docs/usage.md TODO.md CHANGES.md
git commit -m "docs: document hotkeys, user menu and script events, tick the roadmap"
```

---

## Self-Review

**Spec coverage**

- Data model, key normalization, validation, dangling references: Task 1. Duplicate key = editor error: Task 1 `validate_hotkeys`, Task 5 Save gating. Hub defence for hand-edited files: Task 1 `sanitize`, Task 2 hub test.
- IPC (`RunFunctionById`, `SetBindings`, `BindingsUpdate`, `Welcome.bindings`): Tasks 2 and 3.
- Hub execution model (origins, sentinel, headless, log routing): Task 3.
- Local hotkeys (map, priority over built-ins, editing-focus rule): Task 4. Editor and Find Key: Task 5.
- Global hotkeys (lazy connection, four lock variants, unresolvable/`BadAccess` logging, inert on failure, Ctrl/Alt/Super rule): Task 6 (rule enforced by `validate_hotkeys` and `sanitize`).
- User menu (button, lazy popup, edit section, prune, editor, errors): Task 7.
- Script events (emission table, guards, shutdown wait, desktop gestures, editor, `Hidden`/`Revealed` inert): Task 8.
- Testing (unit, hub socket tests, pure X11 parts, gio menu conversion, manual Xephyr acceptance): every task.
- Documentation and roadmap: Task 9.

Open questions of the spec, resolved: background double click uses `connect_background_activate` (Task 8, verified manually in Step 8); `drop_conn` returns `bool` and the new `Hub::drop_lister` emits `ListerClosed` (Task 8); the command channel entry is `HubCmd::RunFunctionHeadless` reached through `OwnerMsg::FromGtk` (Task 6); the editors stay open on Save, matching `function_editor.rs`.

One deliberate clarification of the spec: `SetBindings` is sanitized (tolerant, logged) rather than rejected, and editors validate only their own section, so a dangling id in one section can never block saving another.

**Placeholder scan:** no "TBD", no "implement later", no "similar to Task N"; every code step shows the code.

**Type consistency:** `Hub::run_headless -> Option<u64>` and `run_function_by_id -> Option<u64>` (Task 3) are used as such in `emit_script_event` (Task 8). `start_function_run(requester, source, steps) -> u64` matches its three call sites (`RunFunction`, `run_function_by_id`, and the tests through the socket). `EventGuard::{try_begin, add_run, finish}` match Task 8's hub usage and tests. `binding_ui::{function_dropdown, selected_function}` (Task 5) match their use in Tasks 7 and 8. `ServerOptions` grows `global_keys` in Task 6; the struct-update syntax `..ServerOptions::default()` used in Task 8's helper keeps compiling. `Client::connect`'s 5-tuple (Task 2) is the only shape used afterwards.
