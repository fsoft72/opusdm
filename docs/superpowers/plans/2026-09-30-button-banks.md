# Button Banks (F-02) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Free-floating, hub-owned windows of buttons (banks), each button running up to three saved Functions (left / right / middle click), persisted one JSON file per bank, restored at hub startup, driven by `OpenBank` / `CloseBank` / `ToggleBank` / `HideBank` Function steps, with a bank manager and editor.

**Architecture:** Data model, validation and persistence in `opusdm_ipc::buttonbank` (no GTK, unit tested). New GTK4 crate `crates/buttonbank` with the bank window and the manager/editor windows; it knows nothing about the hub and receives callbacks. The hub (`crates/hub/src/bank_windows.rs`) owns the open windows, restores them, handles the bank commands (as `FnEvent`/`HubEvent`, like `Hide`) and runs button Functions with `HubCmd::RunFunctionHeadless`.

**Tech Stack:** Rust 2021, GTK4 (`gtk4` 0.10), `x11rb` + `gdk4-x11` (skip-taskbar and window position), `serde`/`serde_json`, `tempfile` for tests.

**Spec:** `docs/superpowers/specs/2026-09-30-button-banks-design.md`

## Global Constraints

- Never use the long dash character in text, code or comments; use `-`.
- Code and comments in English. Every function and method has a doc comment. Public items get rustdoc.
- Constants in `UPPER_SNAKE_CASE`. Exit conditions at the top of a function, one-liners when possible (`if !ok { return; }` style used by the surrounding code).
- Text files end with an empty line.
- Follow existing conventions: `#[serde(default)]` on persisted structs, `opusdm_ipc::err_log!` for non-debug logging, no `unwrap` on user data.
- `pnpm` is the package manager if a JS tool is ever needed (none is here). The formatter is `dprint`, run by the `.githooks/pre-commit` hook: NEVER bypass it (`--no-verify` is forbidden); if it reformats, the commit still includes the formatted files.
- Commit messages carry no AI attribution lines. Commit only the files changed by the task. Do not push.
- Bank files: `~/.config/opusdm/buttons/<slug>.json` (`opusdm_ipc::settings::config_dir().join("buttons")`).
- `columns` is clamped to `1..=32`, default `4`. At most `256` buttons per bank.
- Bank windows must never appear in the panel window list nor in the pager (`_NET_WM_STATE_SKIP_TASKBAR` and `_NET_WM_STATE_SKIP_PAGER`).

## Deviations from the spec (decided while planning, spec updated in Task 7)

- The Function Editor reads the bank names for the `bank` parameter directly from `~/.config/opusdm/buttons/` when a step form is built. No `Welcome`/`BanksUpdate` protocol change: the files are the source of truth and every process runs as the same user.
- The Lister palette entry "Button Banks..." sends a new `ToHub::OpenBankManager`, because the manager runs in the hub process.
- The manager has New, Edit, Delete, Open, Close. A bank is renamed in the editor (its file id does not change).
- Window position is saved when a bank window closes and just before the hub quits (X11 has no cheap "moved" signal in GTK4).
- `Hide` (desktop hide) also hides the bank windows and `Reveal` shows them again, like group windows.

## Review Focus

- A bank file that does not parse must never be overwritten and must leave a `.bak` (tests in Task 1).
- A saved position that is negative or absurdly large (monitor unplugged since) must not put the window off screen: it is dropped (test in Task 1).
- A button bound to a Function that was deleted afterwards is shown insensitive with a tooltip, not a crash (test of the pure helper in Task 4).
- A bank with zero buttons must still open as a small empty window and be editable (grid helper test in Task 4).
- Two banks whose names slugify to the same id ("My Bank" and "my-bank") must get distinct files (test in Task 1).
- Toggle / Hide on a bank that is hidden, closed or open must follow one fixed state table (test in Task 1).

---

### Task 1: Data model in `opusdm_ipc::buttonbank`

**Files:**
- Create: `crates/ipc/src/buttonbank.rs`
- Modify: `crates/ipc/src/lib.rs` (add `pub mod buttonbank;` and re-exports)

**Interfaces:**
- Produces (used by every later task):
  - `pub const MAX_BUTTONS: usize = 256; pub const MIN_COLUMNS: u32 = 1; pub const MAX_COLUMNS: u32 = 32; pub const DEFAULT_COLUMNS: u32 = 4;`
  - `pub fn banks_dir() -> PathBuf`
  - `pub enum Orientation { Horizontal, Vertical }` (`Default` = Horizontal, serde, `Copy`)
  - `pub enum MouseButton { Left, Right, Middle }`
  - `pub enum BankAction { Open, Close, Toggle, Hide }` and `pub enum BankView { Closed, Hidden, Shown }` with `BankAction::apply(self, BankView) -> BankView`
  - `pub struct Btn { label: String, icon: Option<PathBuf>, left: Option<FunctionId>, right: Option<FunctionId>, middle: Option<FunctionId> }` with `Btn::function_for(&self, MouseButton) -> Option<&str>`
  - `pub struct ButtonBank { id: String (serde skip), name, orientation, borderless: bool, columns: u32, open: bool, x: Option<i32>, y: Option<i32>, buttons: Vec<Btn> }` with `grid_position(&self, index) -> (usize /*row*/, usize /*col*/)`, `validate(&self) -> Result<(), String>`, `insert_button(&mut self, index, Btn) -> Result<usize, String>`, `remove_button(&mut self, index) -> Option<Btn>`, `move_button(&mut self, index, delta: isize) -> Option<usize>`
  - `pub struct Banks { pub banks: Vec<ButtonBank> }` with `get(&self, id) -> Option<&ButtonBank>`, `get_by_name(&self, name) -> Option<&ButtonBank>`, `names(&self) -> Vec<String>`, `create_in_dir(&mut self, dir: &Path, name: &str) -> Result<String, String>`, `replace(&mut self, id: &str, bank: ButtonBank) -> Result<(), String>`, `delete(&mut self, id: &str) -> bool`, `set_runtime(&mut self, id, open: bool, pos: Option<(i32, i32)>)`, `load_from_dir(dir) -> Banks`, `load() -> Banks`, `save_bank_to(&self, dir, id) -> io::Result<()>`, `save_bank(&self, id)`, `remove_bank_file_from(dir, id)`, `remove_bank_file(id)`

- [ ] **Step 1: Write the failing tests**

Create `crates/ipc/src/buttonbank.rs` containing only the module doc, the `use` lines and this test module (the items above do not exist yet, so it fails to compile):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn btn(label: &str) -> Btn {
        Btn { label: label.to_string(), ..Btn::default() }
    }

    fn bank_with(n: usize) -> ButtonBank {
        let mut b = ButtonBank { name: "Tools".to_string(), ..ButtonBank::default() };
        for i in 0..n {
            b.buttons.push(btn(&format!("b{i}")));
        }
        b
    }

    #[test]
    fn defaults_are_four_columns_horizontal() {
        let b = ButtonBank::default();
        assert_eq!(b.columns, DEFAULT_COLUMNS);
        assert_eq!(b.orientation, Orientation::Horizontal);
        assert!(!b.open && !b.borderless);
    }

    #[test]
    fn a_file_without_the_new_fields_still_parses() {
        let b: ButtonBank = serde_json::from_str(r#"{"name":"Old"}"#).unwrap();
        assert_eq!(b.name, "Old");
        assert_eq!(b.columns, DEFAULT_COLUMNS);
        assert!(b.buttons.is_empty());
    }

    #[test]
    fn serde_round_trip_keeps_everything_but_the_id() {
        let mut b = bank_with(2);
        b.id = "tools".to_string();
        b.orientation = Orientation::Vertical;
        b.buttons[0].left = Some("f1".to_string());
        b.buttons[0].icon = Some(PathBuf::from("/a/b.png"));
        b.x = Some(10);
        b.y = Some(20);
        let back: ButtonBank = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back.id, "");
        assert_eq!(ButtonBank { id: "tools".to_string(), ..back }, b);
    }

    #[test]
    fn function_for_maps_each_mouse_button() {
        let b = Btn { left: Some("l".into()), middle: Some("m".into()), ..Btn::default() };
        assert_eq!(b.function_for(MouseButton::Left), Some("l"));
        assert_eq!(b.function_for(MouseButton::Middle), Some("m"));
        assert_eq!(b.function_for(MouseButton::Right), None);
    }

    #[test]
    fn grid_position_fills_rows_when_horizontal_and_columns_when_vertical() {
        let mut b = bank_with(6);
        b.columns = 3;
        assert_eq!(b.grid_position(0), (0, 0));
        assert_eq!(b.grid_position(2), (0, 2));
        assert_eq!(b.grid_position(3), (1, 0));
        b.orientation = Orientation::Vertical;
        assert_eq!(b.grid_position(0), (0, 0));
        assert_eq!(b.grid_position(2), (2, 0));
        assert_eq!(b.grid_position(3), (0, 1));
    }

    #[test]
    fn validate_rejects_empty_name_bad_columns_and_too_many_buttons() {
        assert!(bank_with(1).validate().is_ok());
        let mut b = bank_with(1);
        b.name = "  ".to_string();
        assert!(b.validate().unwrap_err().contains("name"));
        let mut b = bank_with(1);
        b.columns = 0;
        assert!(b.validate().unwrap_err().contains("columns"));
        b.columns = MAX_COLUMNS + 1;
        assert!(b.validate().unwrap_err().contains("columns"));
        assert!(bank_with(MAX_BUTTONS + 1).validate().unwrap_err().contains("buttons"));
    }

    #[test]
    fn validate_rejects_an_empty_function_id() {
        let mut b = bank_with(1);
        b.buttons[0].left = Some(" ".to_string());
        assert!(b.validate().unwrap_err().contains("Function"));
    }

    #[test]
    fn insert_remove_and_move_buttons() {
        let mut b = bank_with(3);
        assert_eq!(b.insert_button(1, btn("new")), Ok(1));
        assert_eq!(b.buttons[1].label, "new");
        assert_eq!(b.insert_button(99, btn("end")), Ok(4));
        assert_eq!(b.buttons.last().unwrap().label, "end");
        assert_eq!(b.remove_button(1).unwrap().label, "new");
        assert!(b.remove_button(99).is_none());
        // move up / down, refused at the edges
        assert_eq!(b.move_button(1, -1), Some(0));
        assert_eq!(b.buttons[0].label, "b1");
        assert_eq!(b.move_button(0, -1), None);
        let last = b.buttons.len() - 1;
        assert_eq!(b.move_button(last, 1), None);
        assert_eq!(b.move_button(0, 1), Some(1));
    }

    #[test]
    fn insert_is_refused_at_the_button_limit() {
        let mut b = bank_with(MAX_BUTTONS);
        assert!(b.insert_button(0, btn("x")).is_err());
        assert_eq!(b.buttons.len(), MAX_BUTTONS);
    }

    #[test]
    fn cleaned_clamps_columns_and_drops_absurd_positions() {
        let b = ButtonBank { columns: 500, x: Some(-5), y: Some(10), ..bank_with(1) }.cleaned();
        assert_eq!(b.columns, MAX_COLUMNS);
        assert_eq!((b.x, b.y), (None, None));
        let b = ButtonBank { columns: 0, x: Some(40_000), y: Some(0), ..bank_with(1) }.cleaned();
        assert_eq!(b.columns, MIN_COLUMNS);
        assert_eq!((b.x, b.y), (None, None));
        let b = ButtonBank { x: Some(100), y: Some(200), ..bank_with(1) }.cleaned();
        assert_eq!((b.x, b.y), (Some(100), Some(200)));
    }

    #[test]
    fn action_state_table() {
        use BankAction::*;
        use BankView::*;
        for (action, from, to) in [
            (Open, Closed, Shown),
            (Open, Hidden, Shown),
            (Open, Shown, Shown),
            (Close, Closed, Closed),
            (Close, Hidden, Closed),
            (Close, Shown, Closed),
            (Hide, Closed, Closed),
            (Hide, Hidden, Hidden),
            (Hide, Shown, Hidden),
            (Toggle, Closed, Shown),
            (Toggle, Hidden, Shown),
            (Toggle, Shown, Closed),
        ] {
            assert_eq!(action.apply(from), to, "{action:?} on {from:?}");
        }
    }

    #[test]
    fn create_gives_distinct_ids_to_names_with_the_same_slug() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let a = banks.create_in_dir(td.path(), "My Bank").unwrap();
        let b = banks.create_in_dir(td.path(), "my-bank").unwrap();
        assert_eq!(a, "my-bank");
        assert_eq!(b, "my-bank-2");
    }

    #[test]
    fn create_refuses_empty_and_duplicate_names_ignoring_case() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        assert!(banks.create_in_dir(td.path(), "  ").is_err());
        banks.create_in_dir(td.path(), "Tools").unwrap();
        assert!(banks.create_in_dir(td.path(), "tOOLS").unwrap_err().contains("already exists"));
    }

    #[test]
    fn create_never_reuses_an_id_whose_file_exists_on_disk() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("tools.json"), "not json").unwrap();
        let mut banks = Banks::default();
        assert_eq!(banks.create_in_dir(td.path(), "Tools").unwrap(), "tools-2");
    }

    #[test]
    fn replace_keeps_the_runtime_state_and_checks_the_name() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let id = banks.create_in_dir(td.path(), "Tools").unwrap();
        let other = banks.create_in_dir(td.path(), "Other").unwrap();
        banks.set_runtime(&id, true, Some((5, 6)));
        let mut edited = bank_with(2);
        edited.name = "Renamed".to_string();
        banks.replace(&id, edited).unwrap();
        let got = banks.get(&id).unwrap();
        assert_eq!(got.name, "Renamed");
        assert_eq!(got.id, id);
        assert!(got.open);
        assert_eq!((got.x, got.y), (Some(5), Some(6)));
        let mut clash = bank_with(1);
        clash.name = "renamed".to_string();
        assert!(banks.replace(&other, clash).is_err());
        assert!(banks.replace("missing", bank_with(1)).is_err());
    }

    #[test]
    fn save_and_load_round_trip_in_name_order() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let b = banks.create_in_dir(td.path(), "Bravo").unwrap();
        let a = banks.create_in_dir(td.path(), "Alpha").unwrap();
        banks.save_bank_to(td.path(), &b).unwrap();
        banks.save_bank_to(td.path(), &a).unwrap();
        let loaded = Banks::load_from_dir(td.path());
        assert_eq!(loaded.names(), vec!["Alpha".to_string(), "Bravo".to_string()]);
        assert_eq!(loaded.get("alpha").unwrap().name, "Alpha");
        assert_eq!(loaded.get_by_name("BRAVO").unwrap().id, "bravo");
    }

    #[test]
    fn a_file_that_does_not_parse_is_backed_up_and_left_alone() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("broken.json");
        std::fs::write(&path, "{ not json").unwrap();
        let loaded = Banks::load_from_dir(td.path());
        assert!(loaded.banks.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        assert_eq!(std::fs::read_to_string(td.path().join("broken.json.bak")).unwrap(), "{ not json");
        // an existing backup is never overwritten
        std::fs::write(&path, "{ other").unwrap();
        Banks::load_from_dir(td.path());
        assert_eq!(std::fs::read_to_string(td.path().join("broken.json.bak")).unwrap(), "{ not json");
    }

    #[test]
    fn an_empty_name_in_a_file_falls_back_to_the_file_name() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("noname.json"), r#"{"name":""}"#).unwrap();
        assert_eq!(Banks::load_from_dir(td.path()).names(), vec!["noname".to_string()]);
    }

    #[test]
    fn delete_and_remove_file() {
        let td = tempfile::tempdir().unwrap();
        let mut banks = Banks::default();
        let id = banks.create_in_dir(td.path(), "Tools").unwrap();
        banks.save_bank_to(td.path(), &id).unwrap();
        assert!(banks.delete(&id));
        assert!(!banks.delete(&id));
        Banks::remove_bank_file_from(td.path(), &id).unwrap();
        assert!(!td.path().join("tools.json").exists());
        Banks::remove_bank_file_from(td.path(), &id).unwrap();
    }

    #[test]
    fn saving_an_unknown_id_is_an_error() {
        let td = tempfile::tempdir().unwrap();
        assert!(Banks::default().save_bank_to(td.path(), "nope").is_err());
    }
}
```

- [ ] **Step 2: Wire the module and run the tests to see them fail**

In `crates/ipc/src/lib.rs` add `pub mod buttonbank;` (alphabetical, after `bindings`) and, after the `pub use groups::...` line, `pub use buttonbank::{BankAction, BankView, Banks, Btn, ButtonBank, MouseButton, Orientation};`.

Run: `cargo test -p opusdm-ipc buttonbank`
Expected: FAIL to compile (`cannot find type ButtonBank` ...).

- [ ] **Step 3: Write the implementation**

Put this above the test module in `crates/ipc/src/buttonbank.rs`:

```rust
//! Button banks: named, free-floating windows of buttons. Each button has a
//! label and/or an icon and up to three Functions (left, right, middle
//! click). One JSON file per bank under `~/.config/opusdm/buttons/`. See
//! `docs/superpowers/specs/2026-09-30-button-banks-design.md`.

use crate::function::FunctionId;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Most buttons one bank may hold.
pub const MAX_BUTTONS: usize = 256;
/// Smallest and largest number of buttons per row (or per column).
pub const MIN_COLUMNS: u32 = 1;
pub const MAX_COLUMNS: u32 = 32;
/// Buttons per row (or per column) of a new bank.
pub const DEFAULT_COLUMNS: u32 = 4;
/// Saved window positions outside `0..=MAX_POSITION` are dropped on load.
const MAX_POSITION: i32 = 32_000;

/// Base directory of the bank files (`~/.config/opusdm/buttons`).
pub fn banks_dir() -> PathBuf {
    crate::settings::config_dir().join("buttons")
}

/// How the buttons are laid out: rows filled left to right, or columns
/// filled top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

/// The mouse button that was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// What a bank command asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankAction {
    Open,
    Close,
    Toggle,
    Hide,
}

/// The visible state of one bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankView {
    /// No window.
    Closed,
    /// A window that is open but hidden.
    Hidden,
    /// A visible window.
    Shown,
}

impl BankAction {
    /// The state a bank in `current` ends up in. `Hide` keeps a closed bank
    /// closed; `Toggle` closes a shown bank and shows anything else.
    pub fn apply(self, current: BankView) -> BankView {
        match (self, current) {
            (BankAction::Open, _) => BankView::Shown,
            (BankAction::Close, _) => BankView::Closed,
            (BankAction::Hide, BankView::Closed) => BankView::Closed,
            (BankAction::Hide, _) => BankView::Hidden,
            (BankAction::Toggle, BankView::Shown) => BankView::Closed,
            (BankAction::Toggle, _) => BankView::Shown,
        }
    }
}

/// One button: what it shows and the Function each mouse button runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Btn {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<FunctionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<FunctionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub middle: Option<FunctionId>,
}

impl Btn {
    /// The Function bound to `button`, if any.
    pub fn function_for(&self, button: MouseButton) -> Option<&str> {
        match button {
            MouseButton::Left => self.left.as_deref(),
            MouseButton::Right => self.right.as_deref(),
            MouseButton::Middle => self.middle.as_deref(),
        }
    }
}

/// One bank: its file id, appearance, saved window state and buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonBank {
    /// File stem of the bank's JSON file. Not stored inside the file.
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub orientation: Orientation,
    pub borderless: bool,
    /// Buttons per row (Horizontal) or per column (Vertical).
    pub columns: u32,
    /// Whether the window was open when the hub last ran.
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    pub buttons: Vec<Btn>,
}

impl Default for ButtonBank {
    fn default() -> Self {
        ButtonBank {
            id: String::new(),
            name: String::new(),
            orientation: Orientation::Horizontal,
            borderless: false,
            columns: DEFAULT_COLUMNS,
            open: false,
            x: None,
            y: None,
            buttons: Vec::new(),
        }
    }
}

impl ButtonBank {
    /// Clamp `columns`, cap the button count and drop a saved position that
    /// is negative, beyond `MAX_POSITION` or only half there.
    pub fn cleaned(mut self) -> ButtonBank {
        self.columns = self.columns.clamp(MIN_COLUMNS, MAX_COLUMNS);
        self.buttons.truncate(MAX_BUTTONS);
        let ok = matches!((self.x, self.y), (Some(x), Some(y)) if (0..=MAX_POSITION).contains(&x) && (0..=MAX_POSITION).contains(&y));
        if !ok {
            self.x = None;
            self.y = None;
        }
        self
    }

    /// The `(row, column)` of button `index`: rows fill first when
    /// Horizontal, columns fill first when Vertical.
    pub fn grid_position(&self, index: usize) -> (usize, usize) {
        let columns = self.columns.clamp(MIN_COLUMNS, MAX_COLUMNS) as usize;
        match self.orientation {
            Orientation::Horizontal => (index / columns, index % columns),
            Orientation::Vertical => (index % columns, index / columns),
        }
    }

    /// Why this bank cannot be saved, if it cannot. A referenced Function
    /// only has to be a non-empty id: it may be deleted later, which the
    /// window shows instead.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("The bank name is empty".to_string());
        }
        if !(MIN_COLUMNS..=MAX_COLUMNS).contains(&self.columns) {
            return Err(format!("columns must be between {MIN_COLUMNS} and {MAX_COLUMNS}"));
        }
        if self.buttons.len() > MAX_BUTTONS {
            return Err(format!("A bank holds at most {MAX_BUTTONS} buttons"));
        }
        for (i, b) in self.buttons.iter().enumerate() {
            for id in [&b.left, &b.right, &b.middle].into_iter().flatten() {
                if id.trim().is_empty() {
                    return Err(format!("Button {}: empty Function id", i + 1));
                }
            }
        }
        Ok(())
    }

    /// Insert `button` before `index` (clamped to the end). Returns the
    /// index it ended up at.
    pub fn insert_button(&mut self, index: usize, button: Btn) -> Result<usize, String> {
        if self.buttons.len() >= MAX_BUTTONS {
            return Err(format!("A bank holds at most {MAX_BUTTONS} buttons"));
        }
        let index = index.min(self.buttons.len());
        self.buttons.insert(index, button);
        Ok(index)
    }

    /// Remove and return button `index`, or `None` when it does not exist.
    pub fn remove_button(&mut self, index: usize) -> Option<Btn> {
        if index >= self.buttons.len() {
            return None;
        }
        Some(self.buttons.remove(index))
    }

    /// Move button `index` by `delta` places. Returns its new index, or
    /// `None` when it would leave the list.
    pub fn move_button(&mut self, index: usize, delta: isize) -> Option<usize> {
        let target = index.checked_add_signed(delta)?;
        if index >= self.buttons.len() || target >= self.buttons.len() {
            return None;
        }
        self.buttons.swap(index, target);
        Some(target)
    }
}

/// Every bank, in file-name order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Banks {
    pub banks: Vec<ButtonBank>,
}

impl Banks {
    /// The bank with file id `id`.
    pub fn get(&self, id: &str) -> Option<&ButtonBank> {
        self.banks.iter().find(|b| b.id == id)
    }

    /// The bank called `name`, ignoring case.
    pub fn get_by_name(&self, name: &str) -> Option<&ButtonBank> {
        let name = name.trim().to_lowercase();
        self.banks.iter().find(|b| b.name.to_lowercase() == name)
    }

    /// Every bank name, in file-name order.
    pub fn names(&self) -> Vec<String> {
        self.banks.iter().map(|b| b.name.clone()).collect()
    }

    /// Whether a bank other than `except` already has `name`, ignoring case.
    fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let name = name.trim().to_lowercase();
        self.banks.iter().any(|b| Some(b.id.as_str()) != except && b.name.to_lowercase() == name)
    }

    /// Create an empty bank called `name` (trimmed). Its id is a slug that
    /// also avoids every `<id>.json` already in `dir`, files that failed to
    /// load included, so a new bank can never overwrite a file it did not
    /// read. Returns the id. Empty and duplicate names are refused.
    pub fn create_in_dir(&mut self, dir: &Path, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("The bank name is empty".to_string());
        }
        if self.name_taken(name, None) {
            return Err(format!("A bank named \"{name}\" already exists"));
        }
        let base = crate::groups::slugify(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() || dir.join(format!("{id}.json")).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.banks.push(ButtonBank { id: id.clone(), name: name.to_string(), ..ButtonBank::default() });
        self.banks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(id)
    }

    /// Replace bank `id` with `bank` after validating it. The id and the
    /// runtime state (`open`, `x`, `y`) of the stored bank are kept, so an
    /// editor holding an older copy cannot undo what the hub did meanwhile.
    pub fn replace(&mut self, id: &str, bank: ButtonBank) -> Result<(), String> {
        bank.validate()?;
        if self.name_taken(&bank.name, Some(id)) {
            return Err(format!("A bank named \"{}\" already exists", bank.name.trim()));
        }
        let slot = self.banks.iter_mut().find(|b| b.id == id).ok_or_else(|| format!("No bank \"{id}\""))?;
        let bank = ButtonBank { id: slot.id.clone(), open: slot.open, x: slot.x, y: slot.y, name: bank.name.trim().to_string(), ..bank };
        *slot = bank.cleaned();
        Ok(())
    }

    /// Remove bank `id` from the list. Returns whether it existed. The file
    /// is removed with [`Banks::remove_bank_file`].
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.banks.len();
        self.banks.retain(|b| b.id != id);
        self.banks.len() != before
    }

    /// Record whether bank `id` is open and where its window is.
    pub fn set_runtime(&mut self, id: &str, open: bool, pos: Option<(i32, i32)>) {
        let Some(bank) = self.banks.iter_mut().find(|b| b.id == id) else { return };
        bank.open = open;
        if let Some((x, y)) = pos {
            bank.x = Some(x);
            bank.y = Some(y);
        }
    }

    /// Load every `*.json` file of `dir` (sorted by name). A missing
    /// directory gives no banks. A file that cannot be read or parsed is
    /// skipped with a log line; a parse failure also copies it to
    /// `<file>.json.bak` (an existing backup is kept), so it survives even if
    /// something else later writes a bank with the same id.
    pub fn load_from_dir(dir: &Path) -> Banks {
        let Ok(read) = std::fs::read_dir(dir) else { return Banks::default() };
        let mut files: Vec<PathBuf> =
            read.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
        files.sort();
        let mut banks = Vec::new();
        for path in files {
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    crate::err_log!("buttonbank", "cannot read {}: {e}", path.display());
                    continue;
                }
            };
            match serde_json::from_str::<ButtonBank>(&text) {
                Ok(mut bank) => {
                    bank.id = id.clone();
                    if bank.name.trim().is_empty() {
                        bank.name = id;
                    }
                    banks.push(bank.cleaned());
                }
                Err(e) => {
                    crate::err_log!("buttonbank", "malformed bank file {}: {e}", path.display());
                    let backup = path.with_extension("json.bak");
                    if !backup.exists() {
                        if let Err(e) = std::fs::copy(&path, &backup) {
                            crate::err_log!("buttonbank", "cannot back up {}: {e}", path.display());
                        }
                    }
                }
            }
        }
        Banks { banks }
    }

    /// [`Banks::load_from_dir`] the standard directory.
    pub fn load() -> Banks {
        Banks::load_from_dir(&banks_dir())
    }

    /// Write bank `id` to `<dir>/<id>.json`, creating `dir`. An unknown id is
    /// an error.
    pub fn save_bank_to(&self, dir: &Path, id: &str) -> io::Result<()> {
        let bank = self.get(id).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no bank \"{id}\"")))?;
        crate::settings::save_json(&dir.join(format!("{id}.json")), bank)
    }

    /// [`Banks::save_bank_to`] the standard directory.
    pub fn save_bank(&self, id: &str) -> io::Result<()> {
        self.save_bank_to(&banks_dir(), id)
    }

    /// Delete `<dir>/<id>.json`. A file that is already gone is not an error.
    pub fn remove_bank_file_from(dir: &Path, id: &str) -> io::Result<()> {
        match std::fs::remove_file(dir.join(format!("{id}.json"))) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// [`Banks::remove_bank_file_from`] the standard directory.
    pub fn remove_bank_file(id: &str) -> io::Result<()> {
        Banks::remove_bank_file_from(&banks_dir(), id)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p opusdm-ipc buttonbank`
Expected: PASS (all tests in `buttonbank::tests`). If `create_gives_distinct_ids...` fails because `slugify("my-bank")` differs from `"my-bank"`, read `groups::slugify` (it maps `-` to a dash run, so it returns `my-bank`).

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/buttonbank.rs crates/ipc/src/lib.rs
git commit -m "feat(ipc): button bank data model, validation and persistence (F-02)"
```

---

### Task 2: Bank commands in the Function system (ipc + Function Editor)

**Files:**
- Modify: `crates/ipc/src/function.rs` (new `InternalCmd` variants, `schema::ParamKind::Bank`, `params_for`, `bank_action`)
- Modify: `crates/lister/src/function_editor.rs` (`INTERNAL_CMDS`, bank drop-down)

**Interfaces:**
- Consumes: `BankAction` (Task 1).
- Produces: `InternalCmd::{OpenBank, CloseBank, ToggleBank, HideBank}`, `InternalCmd::bank_action(self) -> Option<BankAction>`, `schema::ParamKind::Bank`, param key `"bank"` on those commands.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` of `crates/ipc/src/function.rs`:

```rust
    #[test]
    fn bank_commands_need_a_bank_name_and_map_to_actions() {
        use crate::buttonbank::BankAction;
        for (cmd, label, action) in [
            (InternalCmd::OpenBank, "Open Bank", BankAction::Open),
            (InternalCmd::CloseBank, "Close Bank", BankAction::Close),
            (InternalCmd::ToggleBank, "Toggle Bank", BankAction::Toggle),
            (InternalCmd::HideBank, "Hide Bank", BankAction::Hide),
        ] {
            assert_eq!(cmd.label(), label);
            assert_eq!(cmd.bank_action(), Some(action));
            let params = schema::params_for(cmd);
            assert_eq!(params.len(), 1);
            assert_eq!(params[0].key, "bank");
            assert!(params[0].required);
            assert_eq!(params[0].kind, schema::ParamKind::Bank);
            let step = FnStep::Internal { cmd, args: InternalArgs::default() };
            let json = serde_json::to_string(&step).unwrap();
            assert_eq!(serde_json::from_str::<FnStep>(&json).unwrap(), step);
            let f = Function { id: "x".into(), name: "n".into(), steps: vec![step] };
            assert_eq!(validate(&f).len(), 1, "an empty bank name must not validate");
        }
        assert_eq!(InternalCmd::Copy.bank_action(), None);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p opusdm-ipc bank_commands`
Expected: FAIL to compile (`no variant OpenBank`).

- [ ] **Step 3: Implement in `crates/ipc/src/function.rs`**

Add to the `InternalCmd` enum (after `ToggleHide`):

```rust
    /// Open (or show) a button bank by name.
    OpenBank,
    /// Close a button bank by name.
    CloseBank,
    /// Open a bank that is closed or hidden, close one that is showing.
    ToggleBank,
    /// Hide a button bank's window without closing it.
    HideBank,
```

In `InternalCmd::label`, add:

```rust
            InternalCmd::OpenBank => "Open Bank",
            InternalCmd::CloseBank => "Close Bank",
            InternalCmd::ToggleBank => "Toggle Bank",
            InternalCmd::HideBank => "Hide Bank",
```

In `impl InternalCmd`, add:

```rust
    /// The bank command this is, or `None` for every other command.
    pub fn bank_action(self) -> Option<crate::buttonbank::BankAction> {
        use crate::buttonbank::BankAction;
        match self {
            InternalCmd::OpenBank => Some(BankAction::Open),
            InternalCmd::CloseBank => Some(BankAction::Close),
            InternalCmd::ToggleBank => Some(BankAction::Toggle),
            InternalCmd::HideBank => Some(BankAction::Hide),
            _ => None,
        }
    }
```

In `schema::ParamKind` add a variant:

```rust
        /// The name of a button bank, picked from the saved banks.
        Bank,
```

In `schema::params_for`, add the arm:

```rust
            InternalCmd::OpenBank | InternalCmd::CloseBank | InternalCmd::ToggleBank | InternalCmd::HideBank => &[ParamSpec {
                key: "bank",
                label: "Button bank",
                required: true,
                kind: ParamKind::Bank,
            }],
```

Run `cargo build --workspace 2>&1 | head -60`: the compiler lists every non-exhaustive `match` on `InternalCmd` and `ParamKind`. `crates/hub/src/functions.rs` `run_internal` has an `unreachable!` arm list: add
```rust
        InternalCmd::OpenBank | InternalCmd::CloseBank | InternalCmd::ToggleBank | InternalCmd::HideBank => {
            unreachable!("bank commands are handled by run_bank")
        }
```
Fix any other reported match the same boring way (a `ParamKind::Bank` arm next to `Text` where a match exists).

- [ ] **Step 4: Function Editor: command list and bank drop-down**

In `crates/lister/src/function_editor.rs` append to `INTERNAL_CMDS`:

```rust
    InternalCmd::OpenBank,
    InternalCmd::CloseBank,
    InternalCmd::ToggleBank,
    InternalCmd::HideBank,
```

In `build_param_field`, right after the `Bool` early return, add:

```rust
    if spec.kind == schema::ParamKind::Bank {
        return build_bank_field(state, step_idx, spec);
    }
```

Add the function next to `build_flag_field`:

```rust
/// One button-bank parameter: a drop-down of the saved banks, read from disk
/// now. When the step has no bank yet the first one is stored, so what the
/// drop-down shows is what gets saved. With no banks at all the field says so
/// and the required parameter stays missing.
fn build_bank_field(state: &Rc<EditorState>, step_idx: usize, spec: &schema::ParamSpec) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(spec.label));
    label.set_xalign(0.0);
    row.append(&label);

    let names = opusdm_ipc::buttonbank::Banks::load().names();
    if names.is_empty() {
        row.append(&gtk::Label::new(Some("(no button banks yet, create one first)")));
        return row;
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&refs);
    let key = spec.key;
    let current = match state.function.borrow().steps.get(step_idx) {
        Some(FnStep::Internal { args, .. }) => args.get(key).map(str::to_string),
        _ => None,
    };
    match current.and_then(|c| names.iter().position(|n| *n == c)) {
        Some(i) => dropdown.set_selected(i as u32),
        None => {
            if let Some(FnStep::Internal { args, .. }) = state.function.borrow_mut().steps.get_mut(step_idx) {
                args.0.insert(key.to_string(), names[0].clone());
            }
        }
    }
    let state_clone = state.clone();
    dropdown.connect_selected_notify(move |d| {
        let Some(name) = names.get(d.selected() as usize) else { return };
        if let Some(FnStep::Internal { args, .. }) = state_clone.function.borrow_mut().steps.get_mut(step_idx) {
            args.0.insert(key.to_string(), name.clone());
        }
        update_button_states(&state_clone);
    });
    row.append(&dropdown);
    row
}
```

- [ ] **Step 5: Verify and commit**

Run: `cargo test -p opusdm-ipc && cargo build --workspace`
Expected: tests PASS, build OK (a `todo`/warning-free build).

```bash
git add crates/ipc/src/function.rs crates/lister/src/function_editor.rs crates/hub/src/functions.rs
git commit -m "feat(functions): OpenBank, CloseBank, ToggleBank and HideBank commands (F-02)"
```

(Also `git add` any other file the compiler forced you to touch.)

---

### Task 3: Hub runs bank commands (executor step, event, server dispatch)

**Files:**
- Modify: `crates/hub/src/functions.rs` (`FnEvent::Bank`, `run_bank`, dispatch in `run_steps`, tests)
- Modify: `crates/hub/src/server.rs` (`HubEvent::Bank`, `HubEvent::OpenBankManager`, `ToHub::OpenBankManager` handling, event mapping)
- Modify: `crates/ipc/src/proto.rs` (`ToHub::OpenBankManager`, round trip test)
- Modify: `crates/hub/src/toolbar.rs` (temporary logging arms so it compiles; replaced in Task 6)

**Interfaces:**
- Consumes: `InternalCmd::bank_action`, `BankAction` (Tasks 1-2), `Banks::load`, `Banks::get_by_name`.
- Produces: `FnEvent::Bank { action: BankAction, bank: String }`, `HubEvent::Bank { action: BankAction, bank: String }`, `HubEvent::OpenBankManager`, `ToHub::OpenBankManager`.

- [ ] **Step 1: Write the failing tests**

In `crates/hub/src/functions.rs` `mod tests` add (uses the existing `channel`, `drain`, `ctx` helpers):

```rust
    fn bank_args(name: &str) -> InternalArgs {
        InternalArgs(std::collections::BTreeMap::from([("bank".to_string(), name.to_string())]))
    }

    #[test]
    fn run_bank_emits_the_action_for_a_known_bank() {
        use opusdm_ipc::buttonbank::BankAction;
        for (cmd, action) in [
            (InternalCmd::OpenBank, BankAction::Open),
            (InternalCmd::CloseBank, BankAction::Close),
            (InternalCmd::ToggleBank, BankAction::Toggle),
            (InternalCmd::HideBank, BankAction::Hide),
        ] {
            let (tx, rx) = channel();
            let result = run_bank(1, 0, action, &bank_args(" Tools "), |n| n == "Tools", &tx);
            drop(tx);
            assert_eq!(result, Ok(()), "{cmd:?}");
            let events = drain(rx);
            assert!(events.iter().any(|e| matches!(e, FnEvent::Bank { action: a, bank } if *a == action && bank == "Tools")));
            assert!(matches!(events.last(), Some(FnEvent::StepDone { step_idx: 0, exit_code: Some(0) })));
        }
    }

    #[test]
    fn run_bank_fails_for_an_unknown_or_missing_bank_and_emits_no_event() {
        use opusdm_ipc::buttonbank::BankAction;
        for args in [bank_args("Nope"), InternalArgs::default(), bank_args("  ")] {
            let (tx, rx) = channel();
            let result = run_bank(1, 0, BankAction::Open, &args, |_| false, &tx);
            drop(tx);
            let error = result.unwrap_err();
            assert!(error.contains("Bank"), "{error}");
            assert!(!drain(rx).iter().any(|e| matches!(e, FnEvent::Bank { .. })));
        }
    }

    #[test]
    fn run_steps_reports_an_unknown_bank_as_a_failed_step() {
        let (tx, rx) = channel();
        let src = ctx(&[], "/tmp");
        let steps = vec![FnStep::Internal { cmd: InternalCmd::OpenBank, args: bank_args("definitely-not-a-real-bank-name") }];
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        assert!(matches!(drain(rx).last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("not found")));
    }
```

In `crates/ipc/src/proto.rs`, next to `roundtrip_tohub(ToHub::QueryCaches);` add `roundtrip_tohub(ToHub::OpenBankManager);`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p opusdm-hub run_bank && cargo test -p opusdm-ipc roundtrip`
Expected: FAIL to compile (`run_bank`, `FnEvent::Bank`, `ToHub::OpenBankManager` missing).

- [ ] **Step 3: Implement**

`crates/ipc/src/proto.rs`, in `ToHub` after `JumpTo`:

```rust
    /// Ask the hub to open the button bank manager window. No reply.
    OpenBankManager,
```

`crates/hub/src/functions.rs`: add to `FnEvent` (after `ToggleHide`):

```rust
    /// A bank step: the hub must apply `action` to the bank called `bank`.
    Bank { action: opusdm_ipc::buttonbank::BankAction, bank: String },
```

In `run_steps`, add before the generic `FnStep::Internal { cmd, args }` arm:

```rust
            FnStep::Internal { cmd, args } if cmd.bank_action().is_some() => {
                let action = cmd.bank_action().expect("guarded by is_some");
                run_bank(run_id, step_idx, action, args, bank_exists, events_tx)
            }
```

Add the helpers next to `run_hide`:

```rust
/// Whether a button bank called `name` exists on disk.
fn bank_exists(name: &str) -> bool {
    opusdm_ipc::buttonbank::Banks::load().get_by_name(name).is_some()
}

/// Run a bank step: check the named bank exists (`exists`), then hand the
/// action to the hub. No selection is needed. An empty or unknown name fails
/// the step and emits nothing.
fn run_bank(
    run_id: u64,
    step_idx: usize,
    action: opusdm_ipc::buttonbank::BankAction,
    args: &InternalArgs,
    exists: impl Fn(&str) -> bool,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let name = args.get("bank").map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err("Bank: no bank name given".to_string());
    }
    if !exists(name) {
        return Err(format!("Bank \"{name}\" not found"));
    }
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("{action:?} bank {name}") }));
    let _ = events_tx.send((run_id, FnEvent::Bank { action, bank: name.to_string() }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}
```

`crates/hub/src/server.rs`:
- Add to `HubEvent`:
```rust
    /// A Function step asked to apply `action` to the bank called `bank`.
    Bank { action: opusdm_ipc::buttonbank::BankAction, bank: String },
    /// Open the button bank manager window.
    OpenBankManager,
```
- In the `FnEvent` handling (next to the `Hide`/`Reveal`/`ToggleHide` arms, before `_ => {}`), add:
```rust
            crate::functions::FnEvent::Bank { action, bank } => {
                let _ = self.evt_tx.send(HubEvent::Bank { action: *action, bank: bank.clone() });
                return;
            }
```
  and add `| crate::functions::FnEvent::Bank { .. }` to the `=> return` list that already holds `Hide | Reveal | ToggleHide` (the second match, around the `let msg = match ev` block).
- In the `ToHub` dispatch, next to `ToHub::FreeAllCaches`, add:
```rust
            ToHub::OpenBankManager => {
                let _ = self.evt_tx.send(HubEvent::OpenBankManager);
            }
```

`crates/hub/src/toolbar.rs` `apply_event`: add two temporary arms (Task 6 replaces them):

```rust
        HubEvent::Bank { action, bank } => opusdm_ipc::err_log!("hub", "bank command {action:?} {bank}: not wired yet"),
        HubEvent::OpenBankManager => opusdm_ipc::err_log!("hub", "bank manager: not wired yet"),
```

- [ ] **Step 4: Run tests and build**

Run: `cargo test -p opusdm-hub functions && cargo test -p opusdm-ipc && cargo build --workspace`
Expected: PASS. If another `match` on `FnEvent` or `HubEvent` is reported non-exhaustive, add the arm that ignores or forwards it the way `Hide` is treated.

- [ ] **Step 5: Commit**

```bash
git add crates/hub/src/functions.rs crates/hub/src/server.rs crates/hub/src/toolbar.rs crates/ipc/src/proto.rs
git commit -m "feat(hub): run bank commands from Function steps and route them to the GTK side (F-02)"
```

---

### Task 4: The `buttonbank` crate: X11 helpers and the bank window

**Files:**
- Create: `crates/buttonbank/Cargo.toml`, `crates/buttonbank/src/lib.rs`, `crates/buttonbank/src/x11.rs`, `crates/buttonbank/src/grid.rs`, `crates/buttonbank/src/window.rs`
- Modify: `Cargo.toml` (workspace member)

**Interfaces:**
- Consumes: `ButtonBank`, `Btn`, `MouseButton` (Task 1).
- Produces (used by Tasks 5 and 6):
  - `grid::build_grid(bank: &ButtonBank, on_click: Rc<dyn Fn(usize, MouseButton)>, function_exists: &dyn Fn(&str) -> bool) -> gtk::Grid`
  - `grid::all_bound_functions_missing(btn: &Btn, exists: &dyn Fn(&str) -> bool) -> bool`
  - `window::BankWindow::new(app: &gtk::Application, bank: &ButtonBank, hooks: &WindowHooks) -> BankWindow`, `update(&self, bank, hooks)`, `set_visible(&self, bool)`, `present(&self)`, `position(&self) -> Option<(i32, i32)>`, `close(&self)`, `connect_close(&self, f: impl Fn(Option<(i32, i32)>) + 'static)`
  - `window::WindowHooks { pub run: Rc<dyn Fn(&str)>, pub function_exists: Rc<dyn Fn(&str) -> bool> }`
  - `x11::set_skip_taskbar(&gtk::Window)`, `x11::move_to(&gtk::Window, i32, i32)`, `x11::position(&gtk::Window) -> Option<(i32, i32)>`

- [ ] **Step 1: Crate skeleton**

`Cargo.toml` (workspace): add `"crates/buttonbank"` to `members`.

`crates/buttonbank/Cargo.toml`:

```toml
[package]
name = "opusdm-buttonbank"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[lib]
name = "opusdm_buttonbank"
path = "src/lib.rs"

[dependencies]
opusdm-ipc = { path = "../ipc" }
opusdm-ui = { path = "../ui" }
gtk4.workspace = true
gdk4-x11 = "0.10"
x11rb.workspace = true
```

`crates/buttonbank/src/lib.rs`:

```rust
//! Button bank windows and their editor. The crate knows nothing about the
//! hub: callers pass callbacks. See
//! `docs/superpowers/specs/2026-09-30-button-banks-design.md`.

pub mod grid;
pub mod window;
pub mod x11;
```

- [ ] **Step 2: Write the failing test (pure helper)**

Create `crates/buttonbank/src/grid.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::Btn;

    fn btn(left: Option<&str>, right: Option<&str>) -> Btn {
        Btn { left: left.map(String::from), right: right.map(String::from), ..Btn::default() }
    }

    #[test]
    fn a_button_with_no_functions_is_not_disabled() {
        assert!(!all_bound_functions_missing(&btn(None, None), &|_| false));
    }

    #[test]
    fn a_button_is_disabled_only_when_every_bound_function_is_gone() {
        let exists = |id: &str| id == "alive";
        assert!(all_bound_functions_missing(&btn(Some("dead"), None), &exists));
        assert!(all_bound_functions_missing(&btn(Some("dead"), Some("gone")), &exists));
        assert!(!all_bound_functions_missing(&btn(Some("dead"), Some("alive")), &exists));
        assert!(!all_bound_functions_missing(&btn(Some("alive"), None), &exists));
    }
}
```

Run: `cargo test -p opusdm-buttonbank`
Expected: FAIL to compile (`all_bound_functions_missing` missing).

- [ ] **Step 3: Implement `grid.rs`**

Put above the tests:

```rust
//! The grid of buttons shared by the bank window and the editor preview.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{Btn, ButtonBank, MouseButton};
use std::path::Path;
use std::rc::Rc;

/// Pixel size of a button icon.
const ICON_PIXELS: i32 = 32;
/// Smallest size of a button, so an empty one is still clickable.
const MIN_BUTTON_WIDTH: i32 = 48;
const MIN_BUTTON_HEIGHT: i32 = 32;
/// Space between buttons and around the grid.
const GRID_SPACING: u32 = 4;
const GRID_MARGIN: i32 = 4;
/// Tooltip of a button whose Functions were all deleted.
pub const MISSING_TOOLTIP: &str = "Function not found (it may have been deleted)";

/// Whether `btn` has at least one bound Function and none of them exists
/// (`exists` says whether a Function id is still saved). Such a button is
/// shown insensitive. A button with nothing bound is left alone.
pub fn all_bound_functions_missing(btn: &Btn, exists: &dyn Fn(&str) -> bool) -> bool {
    let bound: Vec<&str> = [&btn.left, &btn.right, &btn.middle].into_iter().flatten().map(String::as_str).collect();
    !bound.is_empty() && bound.iter().all(|id| !exists(id))
}

/// One button: the icon (when the file is there) above the label.
fn build_button(btn: &Btn) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
    if let Some(icon) = btn.icon.as_deref().filter(|p| Path::new(p).is_file()) {
        let image = gtk::Image::from_file(icon);
        image.set_pixel_size(ICON_PIXELS);
        content.append(&image);
    }
    if !btn.label.is_empty() {
        content.append(&gtk::Label::new(Some(&btn.label)));
    }
    let button = gtk::Button::new();
    button.set_child(Some(&content));
    button.set_size_request(MIN_BUTTON_WIDTH, MIN_BUTTON_HEIGHT);
    button
}

/// Build the grid of `bank`'s buttons. `on_click(index, mouse_button)` is
/// called for a left, right or middle click on button `index`.
/// `function_exists` decides which buttons are shown insensitive.
pub fn build_grid(
    bank: &ButtonBank,
    on_click: Rc<dyn Fn(usize, MouseButton)>,
    function_exists: &dyn Fn(&str) -> bool,
) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(GRID_SPACING);
    grid.set_column_spacing(GRID_SPACING);
    grid.set_margin_start(GRID_MARGIN);
    grid.set_margin_end(GRID_MARGIN);
    grid.set_margin_top(GRID_MARGIN);
    grid.set_margin_bottom(GRID_MARGIN);
    for (index, btn) in bank.buttons.iter().enumerate() {
        let (row, column) = bank.grid_position(index);
        let button = build_button(btn);
        if all_bound_functions_missing(btn, function_exists) {
            button.set_sensitive(false);
            button.set_tooltip_text(Some(MISSING_TOOLTIP));
        }
        let gesture = gtk::GestureClick::new();
        gesture.set_button(0);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let on_click = on_click.clone();
        gesture.connect_released(move |g, _, _, _| {
            let mouse = match g.current_button() {
                1 => MouseButton::Left,
                2 => MouseButton::Middle,
                3 => MouseButton::Right,
                _ => return,
            };
            on_click(index, mouse);
        });
        button.add_controller(gesture);
        grid.attach(&button, column as i32, row as i32, 1, 1);
    }
    grid
}
```

Run: `cargo test -p opusdm-buttonbank`
Expected: PASS (2 tests). A GTK link error means the GTK4 dev packages are missing, same as for the rest of the workspace.

- [ ] **Step 4: Implement `x11.rs`**

```rust
//! X11 details GTK4 does not expose: keeping a window out of the taskbar
//! and pager, and reading or setting its position. Every function is a no-op
//! (with a log line for real errors) when the session is not X11.

use gdk4_x11::X11Surface;
use gtk4 as gtk;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, ClientMessageEvent, ConnectionExt, EventMask};

/// `_NET_WM_STATE` action: add the property.
const NET_WM_STATE_ADD: u32 = 1;

/// The X11 window id behind a GTK window, if it runs under X11 and is mapped.
fn xid(window: &gtk::Window) -> Option<u32> {
    let surface = window.surface()?;
    let x11 = surface.downcast::<X11Surface>().ok()?;
    Some(x11.xid() as u32)
}

/// Ask the window manager to keep `window` out of the taskbar (window list)
/// and the pager. One `ClientMessage` per state, like the dock does.
pub fn set_skip_taskbar(window: &gtk::Window) {
    let Some(xid) = xid(window) else { return };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
        let skip_taskbar = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR")?.reply()?.atom;
        let skip_pager = conn.intern_atom(false, b"_NET_WM_STATE_SKIP_PAGER")?.reply()?.atom;
        for state in [skip_taskbar, skip_pager] {
            let ev = ClientMessageEvent::new(32, xid, wm_state, [NET_WM_STATE_ADD, state, 0, 0, 0]);
            conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)?;
        }
        conn.flush()?;
        Ok(())
    };
    if let Err(e) = run() {
        opusdm_ipc::err_log!("buttonbank", "cannot hide a bank window from the taskbar: {e}");
    }
}

/// Move `window` so its top-left corner is at `(x, y)` on the screen.
pub fn move_to(window: &gtk::Window, x: i32, y: i32) {
    let Some(xid) = xid(window) else { return };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let (conn, _) = x11rb::connect(None)?;
        conn.configure_window(xid, &xproto::ConfigureWindowAux::new().x(x).y(y))?;
        conn.flush()?;
        Ok(())
    };
    if let Err(e) = run() {
        opusdm_ipc::err_log!("buttonbank", "cannot move a bank window: {e}");
    }
}

/// The position of `window` on the screen, or `None` when it cannot be read
/// (not X11, not mapped).
pub fn position(window: &gtk::Window) -> Option<(i32, i32)> {
    let xid = xid(window)?;
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn.translate_coordinates(xid, root, 0, 0).ok()?.reply().ok()?;
    Some((i32::from(reply.dst_x), i32::from(reply.dst_y)))
}
```

- [ ] **Step 5: Implement `window.rs`**

```rust
//! One bank window: a top-level window with the grid of buttons.

use crate::{grid, x11};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::ButtonBank;
use std::cell::RefCell;
use std::rc::Rc;

/// What a bank window asks of its owner.
#[derive(Clone)]
pub struct WindowHooks {
    /// Run the Function with this id.
    pub run: Rc<dyn Fn(&str)>,
    /// Whether a Function id is still saved.
    pub function_exists: Rc<dyn Fn(&str) -> bool>,
}

/// A bank window. Cloning shares the same window.
#[derive(Clone)]
pub struct BankWindow {
    window: gtk::Window,
    /// The bank the grid was last built from, so a click maps to its button.
    bank: Rc<RefCell<ButtonBank>>,
}

/// Fill `window` with the grid for `bank` and apply its appearance.
fn fill(window: &gtk::Window, shared: &Rc<RefCell<ButtonBank>>, bank: &ButtonBank, hooks: &WindowHooks) {
    *shared.borrow_mut() = bank.clone();
    window.set_title(Some(&bank.name));
    window.set_decorated(!bank.borderless);
    let on_click = {
        let shared = shared.clone();
        let run = hooks.run.clone();
        Rc::new(move |index: usize, mouse| {
            let id = shared.borrow().buttons.get(index).and_then(|b| b.function_for(mouse).map(str::to_string));
            if let Some(id) = id {
                run(&id);
            }
        })
    };
    let exists = hooks.function_exists.clone();
    window.set_child(Some(&grid::build_grid(bank, on_click, &|id| exists(id))));
}

impl BankWindow {
    /// Create (but do not show) the window of `bank`. Once mapped it is kept
    /// out of the taskbar and pager and moved to the bank's saved position.
    pub fn new(app: &gtk::Application, bank: &ButtonBank, hooks: &WindowHooks) -> BankWindow {
        let window = gtk::Window::builder().application(app).resizable(false).build();
        let shared = Rc::new(RefCell::new(ButtonBank::default()));
        fill(&window, &shared, bank, hooks);
        let saved = bank.x.zip(bank.y);
        window.connect_map(move |w| {
            x11::set_skip_taskbar(w);
            if let Some((x, y)) = saved {
                x11::move_to(w, x, y);
            }
        });
        BankWindow { window, bank: shared }
    }

    /// Rebuild the grid, title and decoration from `bank`.
    pub fn update(&self, bank: &ButtonBank, hooks: &WindowHooks) {
        fill(&self.window, &self.bank, bank, hooks);
    }

    /// Show or hide the window without destroying it.
    pub fn set_visible(&self, visible: bool) {
        self.window.set_visible(visible);
    }

    /// Show the window and bring it to the front.
    pub fn present(&self) {
        self.window.present();
    }

    /// The position of the window on the screen, if it can be read.
    pub fn position(&self) -> Option<(i32, i32)> {
        x11::position(&self.window)
    }

    /// Close the window (runs the `connect_close` callback).
    pub fn close(&self) {
        self.window.close();
    }

    /// Call `f` with the window position when the window is closed, whether
    /// by the user or by [`BankWindow::close`].
    pub fn connect_close(&self, f: impl Fn(Option<(i32, i32)>) + 'static) {
        self.window.connect_close_request(move |w| {
            f(x11::position(w));
            glib::Propagation::Proceed
        });
    }
}
```

- [ ] **Step 6: Verify and commit**

Run: `cargo build -p opusdm-buttonbank && cargo test -p opusdm-buttonbank`
Expected: build OK, 2 tests PASS.

```bash
git add Cargo.toml Cargo.lock crates/buttonbank
git commit -m "feat(buttonbank): bank window, button grid and X11 taskbar/position helpers (F-02)"
```

---

### Task 5: Bank manager and editor

**Files:**
- Create: `crates/buttonbank/src/editor.rs`
- Modify: `crates/buttonbank/src/lib.rs` (`pub mod editor;`)

**Interfaces:**
- Consumes: `Banks`, `ButtonBank`, `Btn`, `Orientation`, `MouseButton` (Task 1), `Function`/`load_functions` (ipc), `grid::build_grid` (Task 4), `opusdm_ui::{DialogShell, requester, requester_entry}`.
- Produces:
  - `pub enum BankChange { Saved(String), Deleted(String) }`
  - `#[derive(Clone)] pub struct Hooks { pub changed: Rc<dyn Fn(BankChange)>, pub open: Rc<dyn Fn(&str)>, pub close: Rc<dyn Fn(&str)> }` (ids, not names)
  - `pub fn open_manager(parent: &impl IsA<gtk::Window>, banks: Rc<RefCell<Banks>>, hooks: Hooks)`
  - `pub fn open_editor(parent: &impl IsA<gtk::Window>, banks: Rc<RefCell<Banks>>, id: &str, hooks: Hooks, after_save: Rc<dyn Fn()>)`

The editor logic that is not GTK (button operations, validation) already lives in `opusdm_ipc::buttonbank` and is unit tested there; this task is UI wiring, verified by the manual check in Task 7.

- [ ] **Step 1: Write the manager and editor**

Create `crates/buttonbank/src/editor.rs`:

```rust
//! The bank manager (list of banks: New, Edit, Delete, Open, Close) and the
//! editor of one bank (preview, button toolbar, button form, appearance).
//! The button and validation rules live in `opusdm_ipc::buttonbank`.

use crate::grid;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::function::{load_functions, Function};
use opusdm_ipc::{Banks, Btn, ButtonBank, MouseButton, Orientation};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What changed in the store, for the owner of the windows.
pub enum BankChange {
    /// Bank `id` was created or saved.
    Saved(String),
    /// Bank `id` was deleted.
    Deleted(String),
}

/// What the manager and editor ask of their owner.
#[derive(Clone)]
pub struct Hooks {
    pub changed: Rc<dyn Fn(BankChange)>,
    /// Open (show) the window of bank `id`.
    pub open: Rc<dyn Fn(&str)>,
    /// Close the window of bank `id`.
    pub close: Rc<dyn Fn(&str)>,
}

/// Text of the "no Function" entry of the Function drop-downs.
const NO_FUNCTION: &str = "(none)";

/// Show a modal error message over `parent`.
fn show_error(parent: &impl IsA<gtk::Window>, message: &str) {
    opusdm_ui::requester(parent, "Error", message, None, &[("OK", ())], |_| {});
}

/// Persist bank `id`, returning the OS error as text.
fn save(banks: &Banks, id: &str) -> Result<(), String> {
    banks.save_bank(id).map_err(|e| format!("Cannot save the bank: {e}"))
}

/// Open the manager window.
pub fn open_manager(parent: &impl IsA<gtk::Window>, banks: Rc<RefCell<Banks>>, hooks: Hooks) {
    let shell = opusdm_ui::DialogShell::new(parent, "Button Banks").size(380, 340);
    let list_box = gtk::ListBox::new();
    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).build();
    shell.content().append(&scroller);

    let refresh: Rc<dyn Fn()> = {
        let list_box = list_box.clone();
        let banks = banks.clone();
        Rc::new(move || {
            while let Some(row) = list_box.first_child() {
                list_box.remove(&row);
            }
            for bank in &banks.borrow().banks {
                let label = gtk::Label::new(Some(&bank.name));
                label.set_xalign(0.0);
                label.set_margin_start(6);
                label.set_margin_end(6);
                label.set_margin_top(4);
                label.set_margin_bottom(4);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&label));
                list_box.append(&row);
            }
        })
    };
    refresh();

    let new_btn = shell.button("New...");
    let edit_btn = shell.button("Edit...");
    let delete_btn = shell.button("Delete");
    let open_btn = shell.button("Open");
    let close_btn = shell.button("Close Bank");
    shell.button_spacer();
    shell.close_button("Close");
    let window = shell.window().clone();

    let selected_id = {
        let list_box = list_box.clone();
        let banks = banks.clone();
        move || -> Option<String> {
            let index = usize::try_from(list_box.selected_row()?.index()).ok()?;
            banks.borrow().banks.get(index).map(|b| b.id.clone())
        }
    };

    {
        let (banks, hooks, refresh, window) = (banks.clone(), hooks.clone(), refresh.clone(), window.clone());
        new_btn.connect_clicked(move |_| {
            let (banks, hooks, refresh, window_for_cb) = (banks.clone(), hooks.clone(), refresh.clone(), window.clone());
            opusdm_ui::requester_entry(&window, "New bank", "", &[("Create", true), ("Cancel", false)], move |ok, name| {
                if !ok {
                    return;
                }
                let created = banks.borrow_mut().create_in_dir(&opusdm_ipc::buttonbank::banks_dir(), &name);
                match created.and_then(|id| save(&banks.borrow(), &id).map(|()| id)) {
                    Ok(id) => {
                        refresh();
                        (hooks.changed)(BankChange::Saved(id.clone()));
                        open_editor(&window_for_cb, banks.clone(), &id, hooks.clone(), refresh.clone());
                    }
                    Err(message) => show_error(&window_for_cb, &message),
                }
            });
        });
    }
    {
        let (banks, hooks, refresh, window, selected_id) =
            (banks.clone(), hooks.clone(), refresh.clone(), window.clone(), selected_id.clone());
        edit_btn.connect_clicked(move |_| {
            let Some(id) = selected_id() else { return };
            open_editor(&window, banks.clone(), &id, hooks.clone(), refresh.clone());
        });
    }
    {
        let (banks, hooks, refresh, window, selected_id) =
            (banks.clone(), hooks.clone(), refresh.clone(), window.clone(), selected_id.clone());
        delete_btn.connect_clicked(move |_| {
            let Some(id) = selected_id() else { return };
            let name = banks.borrow().get(&id).map(|b| b.name.clone()).unwrap_or_default();
            let (banks, hooks, refresh, window_for_cb) = (banks.clone(), hooks.clone(), refresh.clone(), window.clone());
            opusdm_ui::requester(
                &window,
                "Request",
                &format!("Delete the bank \"{name}\"?"),
                None,
                &[("Delete", true), ("Cancel", false)],
                move |delete| {
                    if !delete {
                        return;
                    }
                    banks.borrow_mut().delete(&id);
                    if let Err(e) = Banks::remove_bank_file(&id) {
                        show_error(&window_for_cb, &format!("Cannot delete the bank file: {e}"));
                    }
                    refresh();
                    (hooks.changed)(BankChange::Deleted(id.clone()));
                },
            );
        });
    }
    {
        let (hooks, selected_id) = (hooks.clone(), selected_id.clone());
        open_btn.connect_clicked(move |_| {
            if let Some(id) = selected_id() {
                (hooks.open)(&id);
            }
        });
    }
    {
        let (hooks, selected_id) = (hooks.clone(), selected_id.clone());
        close_btn.connect_clicked(move |_| {
            if let Some(id) = selected_id() {
                (hooks.close)(&id);
            }
        });
    }
    shell.present();
}

/// The widgets of the button form.
struct Form {
    label: gtk::Entry,
    icon: gtk::Entry,
    /// Left, right and middle Function drop-downs.
    functions: [gtk::DropDown; 3],
    name: gtk::Entry,
    orientation: gtk::DropDown,
    borderless: gtk::CheckButton,
    columns: gtk::SpinButton,
}

/// Everything the editor's callbacks share.
struct State {
    window: gtk::Window,
    banks: Rc<RefCell<Banks>>,
    id: String,
    /// The working copy being edited.
    bank: RefCell<ButtonBank>,
    selected: Cell<Option<usize>>,
    clipboard: RefCell<Option<Btn>>,
    functions: Vec<Function>,
    preview: gtk::Box,
    form: Form,
    error: gtk::Label,
    save_btn: gtk::Button,
    /// True while the form is being filled from the model, so the change
    /// handlers do not write the values straight back.
    loading: Cell<bool>,
}

/// Open the editor of bank `id`. `after_save` runs after every successful
/// save (the manager refreshes its list with it).
pub fn open_editor(parent: &impl IsA<gtk::Window>, banks: Rc<RefCell<Banks>>, id: &str, hooks: Hooks, after_save: Rc<dyn Fn()>) {
    let Some(bank) = banks.borrow().get(id).cloned() else { return };
    let functions = load_functions();
    let shell = opusdm_ui::DialogShell::new(parent, &format!("Edit bank: {}", bank.name)).size(640, 460);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let names: Vec<&str> = std::iter::once(NO_FUNCTION).chain(functions.iter().map(|f| f.name.as_str())).collect();
    let function_dropdown = || gtk::DropDown::from_strings(&names);
    let form = Form {
        label: gtk::Entry::new(),
        icon: gtk::Entry::new(),
        functions: [function_dropdown(), function_dropdown(), function_dropdown()],
        name: gtk::Entry::new(),
        orientation: gtk::DropDown::from_strings(&["Horizontal", "Vertical"]),
        borderless: gtk::CheckButton::with_label("Borderless"),
        columns: gtk::SpinButton::with_range(f64::from(opusdm_ipc::buttonbank::MIN_COLUMNS), f64::from(opusdm_ipc::buttonbank::MAX_COLUMNS), 1.0),
    };
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.add_css_class("error");

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let tool = |label: &str| {
        let b = gtk::Button::with_label(label);
        toolbar.append(&b);
        b
    };
    let (add_btn, insert_btn, delete_btn, copy_btn, paste_btn, up_btn, down_btn) =
        (tool("Add"), tool("Insert"), tool("Delete"), tool("Copy"), tool("Paste"), tool("Up"), tool("Down"));

    let form_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let row = |caption: &str, widget: &gtk::Widget| {
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let l = gtk::Label::new(Some(caption));
        l.set_width_chars(10);
        l.set_xalign(0.0);
        r.append(&l);
        widget.set_hexpand(true);
        r.append(widget);
        form_box.append(&r);
    };
    row("Bank name", form.name.upcast_ref());
    row("Orientation", form.orientation.upcast_ref());
    row("Columns", form.columns.upcast_ref());
    form_box.append(&form.borderless);
    form_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    row("Label", form.label.upcast_ref());
    row("Icon file", form.icon.upcast_ref());
    row("Left click", form.functions[0].upcast_ref());
    row("Right click", form.functions[1].upcast_ref());
    row("Middle click", form.functions[2].upcast_ref());

    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.append(&toolbar);
    left.append(&gtk::ScrolledWindow::builder().child(&preview).vexpand(true).hexpand(true).build());
    let split = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    split.append(&left);
    split.append(&form_box);
    shell.content().append(&split);
    shell.content().append(&error);

    let save_btn = shell.button("Save");
    shell.close_button("Close");

    let state = Rc::new(State {
        window: shell.window().clone(),
        banks,
        id: id.to_string(),
        bank: RefCell::new(bank),
        selected: Cell::new(None),
        clipboard: RefCell::new(None),
        functions,
        preview,
        form,
        error,
        save_btn: save_btn.clone(),
        loading: Cell::new(false),
    });

    wire_toolbar(&state, [&add_btn, &insert_btn, &delete_btn, &copy_btn, &paste_btn, &up_btn, &down_btn]);
    wire_form(&state);
    {
        let (state, hooks, after_save) = (state.clone(), hooks, after_save);
        save_btn.connect_clicked(move |_| {
            let result = state
                .banks
                .borrow_mut()
                .replace(&state.id, state.bank.borrow().clone())
                .and_then(|()| save(&state.banks.borrow(), &state.id));
            match result {
                Ok(()) => {
                    (hooks.changed)(BankChange::Saved(state.id.clone()));
                    after_save();
                }
                Err(message) => show_error(&state.window, &message),
            }
        });
    }
    refresh_all(&state);
    shell.present();
}

/// Rebuild the preview, fill the form from the selected button and
/// re-validate.
fn refresh_all(state: &Rc<State>) {
    while let Some(child) = state.preview.first_child() {
        state.preview.remove(&child);
    }
    let on_click = {
        let state = state.clone();
        Rc::new(move |index: usize, _mouse: MouseButton| {
            state.selected.set(Some(index));
            load_form(&state);
        })
    };
    let ids: Vec<String> = state.functions.iter().map(|f| f.id.clone()).collect();
    let grid = grid::build_grid(&state.bank.borrow(), on_click, &|id| ids.iter().any(|i| i == id));
    state.preview.append(&grid);
    load_form(state);
}

/// Copy the bank settings and the selected button into the form widgets.
fn load_form(state: &Rc<State>) {
    state.loading.set(true);
    let bank = state.bank.borrow();
    let form = &state.form;
    form.name.set_text(&bank.name);
    form.orientation.set_selected(u32::from(bank.orientation == Orientation::Vertical));
    form.columns.set_value(f64::from(bank.columns));
    form.borderless.set_active(bank.borderless);
    let btn = state.selected.get().and_then(|i| bank.buttons.get(i));
    form.label.set_text(btn.map(|b| b.label.as_str()).unwrap_or(""));
    form.icon.set_text(&btn.and_then(|b| b.icon.as_ref()).map(|p| p.display().to_string()).unwrap_or_default());
    let bound = [btn.and_then(|b| b.left.as_ref()), btn.and_then(|b| b.right.as_ref()), btn.and_then(|b| b.middle.as_ref())];
    for (dropdown, id) in form.functions.iter().zip(bound) {
        let position = id.and_then(|id| state.functions.iter().position(|f| &f.id == id)).map_or(0, |p| p + 1);
        dropdown.set_selected(position as u32);
        dropdown.set_sensitive(btn.is_some());
    }
    form.label.set_sensitive(btn.is_some());
    form.icon.set_sensitive(btn.is_some());
    drop(bank);
    state.loading.set(false);
    validate(state);
}

/// Show why the bank cannot be saved, and enable Save only when it can.
fn validate(state: &Rc<State>) {
    let bank = state.bank.borrow();
    let clash = state
        .banks
        .borrow()
        .banks
        .iter()
        .any(|b| b.id != state.id && b.name.to_lowercase() == bank.name.trim().to_lowercase());
    let problem = match bank.validate() {
        Err(message) => Some(message),
        Ok(()) if clash => Some(format!("A bank named \"{}\" already exists", bank.name.trim())),
        Ok(()) if bank.buttons.iter().any(|b| b.label.is_empty() && b.icon.is_none()) => {
            state.error.set_text("Warning: a button has neither a label nor an icon");
            state.save_btn.set_sensitive(true);
            return;
        }
        Ok(()) => None,
    };
    state.error.set_text(problem.as_deref().unwrap_or(""));
    state.save_btn.set_sensitive(problem.is_none());
}

/// Write the form widgets back into the working copy.
fn wire_form(state: &Rc<State>) {
    let form = &state.form;
    let apply = |state: &Rc<State>, edit: &dyn Fn(&mut ButtonBank, Option<usize>)| {
        if state.loading.get() {
            return;
        }
        edit(&mut state.bank.borrow_mut(), state.selected.get());
        refresh_preview_only(state);
        validate(state);
    };
    {
        let state = state.clone();
        form.name.connect_changed(move |e| {
            let text = e.text().to_string();
            apply(&state, &|b, _| b.name = text.clone());
        });
    }
    {
        let state = state.clone();
        form.orientation.connect_selected_notify(move |d| {
            let vertical = d.selected() == 1;
            apply(&state, &|b, _| b.orientation = if vertical { Orientation::Vertical } else { Orientation::Horizontal });
        });
    }
    {
        let state = state.clone();
        form.columns.connect_value_changed(move |s| {
            let value = s.value() as u32;
            apply(&state, &|b, _| b.columns = value);
        });
    }
    {
        let state = state.clone();
        form.borderless.connect_toggled(move |c| {
            let on = c.is_active();
            apply(&state, &|b, _| b.borderless = on);
        });
    }
    {
        let state = state.clone();
        form.label.connect_changed(move |e| {
            let text = e.text().to_string();
            apply(&state, &|b, i| {
                if let Some(btn) = i.and_then(|i| b.buttons.get_mut(i)) {
                    btn.label = text.clone();
                }
            });
        });
    }
    {
        let state = state.clone();
        form.icon.connect_changed(move |e| {
            let text = e.text().trim().to_string();
            apply(&state, &|b, i| {
                if let Some(btn) = i.and_then(|i| b.buttons.get_mut(i)) {
                    btn.icon = if text.is_empty() { None } else { Some(text.clone().into()) };
                }
            });
        });
    }
    for (slot, dropdown) in form.functions.iter().enumerate() {
        let state = state.clone();
        dropdown.connect_selected_notify(move |d| {
            let choice = d.selected() as usize;
            let id = choice.checked_sub(1).and_then(|p| state.functions.get(p)).map(|f| f.id.clone());
            apply(&state, &|b, i| {
                let Some(btn) = i.and_then(|i| b.buttons.get_mut(i)) else { return };
                match slot {
                    0 => btn.left = id.clone(),
                    1 => btn.right = id.clone(),
                    _ => btn.middle = id.clone(),
                }
            });
        });
    }
}

/// Rebuild only the preview grid (the form keeps what the user is typing).
fn refresh_preview_only(state: &Rc<State>) {
    while let Some(child) = state.preview.first_child() {
        state.preview.remove(&child);
    }
    let on_click = {
        let state = state.clone();
        Rc::new(move |index: usize, _mouse: MouseButton| {
            state.selected.set(Some(index));
            load_form(&state);
        })
    };
    let ids: Vec<String> = state.functions.iter().map(|f| f.id.clone()).collect();
    state.preview.append(&grid::build_grid(&state.bank.borrow(), on_click, &|id| ids.iter().any(|i| i == id)));
}

/// Wire Add, Insert, Delete, Copy, Paste, Up and Down.
fn wire_toolbar(state: &Rc<State>, buttons: [&gtk::Button; 7]) {
    let [add, insert, delete, copy, paste, up, down] = buttons;
    let edit = |btn: &gtk::Button, action: Rc<dyn Fn(&Rc<State>)>| {
        let state = state.clone();
        btn.connect_clicked(move |_| {
            action(&state);
            refresh_all(&state);
        });
    };
    let report = |state: &Rc<State>, result: Result<usize, String>| match result {
        Ok(index) => state.selected.set(Some(index)),
        Err(message) => show_error(&state.window, &message),
    };
    edit(add, Rc::new(move |s| {
        let end = s.bank.borrow().buttons.len();
        let result = s.bank.borrow_mut().insert_button(end, Btn { label: "Button".to_string(), ..Btn::default() });
        report(s, result);
    }));
    edit(insert, Rc::new(move |s| {
        let at = s.selected.get().unwrap_or(0);
        let result = s.bank.borrow_mut().insert_button(at, Btn { label: "Button".to_string(), ..Btn::default() });
        report(s, result);
    }));
    edit(delete, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        s.bank.borrow_mut().remove_button(i);
        let len = s.bank.borrow().buttons.len();
        s.selected.set(if len == 0 { None } else { Some(i.min(len - 1)) });
    }));
    edit(copy, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        *s.clipboard.borrow_mut() = s.bank.borrow().buttons.get(i).cloned();
    }));
    edit(paste, Rc::new(move |s| {
        let Some(button) = s.clipboard.borrow().clone() else { return };
        let at = s.selected.get().map_or(s.bank.borrow().buttons.len(), |i| i + 1);
        let result = s.bank.borrow_mut().insert_button(at, button);
        report(s, result);
    }));
    edit(up, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        if let Some(new) = s.bank.borrow_mut().move_button(i, -1) {
            s.selected.set(Some(new));
        }
    }));
    edit(down, Rc::new(|s| {
        let Some(i) = s.selected.get() else { return };
        if let Some(new) = s.bank.borrow_mut().move_button(i, 1) {
            s.selected.set(Some(new));
        }
    }));
}
```

Add `pub mod editor;` to `crates/buttonbank/src/lib.rs`.

- [ ] **Step 2: Build and fix**

Run: `cargo build -p opusdm-buttonbank`
Expected: OK. GTK code written without a compiler at hand often needs small fixes (borrow of `toolbar` in the `tool` closure, `requester` generic type for the `()` button, a moved `state`). Fix them without changing behavior; do not restructure. If `opusdm_ui::requester` needs `T: Clone` and `()` is fine, keep it.

- [ ] **Step 3: Commit**

```bash
git add crates/buttonbank
git commit -m "feat(buttonbank): bank manager and editor (F-02)"
```

---

### Task 6: Hub wiring: windows, restore, commands, menu, palette

**Files:**
- Create: `crates/hub/src/bank_windows.rs`
- Modify: `crates/hub/Cargo.toml` (dependency), `crates/hub/src/lib.rs` (or `main.rs`, wherever `group_window` is declared: `pub mod bank_windows;`), `crates/hub/src/toolbar.rs`
- Modify: `crates/lister/src/keys.rs`, `crates/lister/src/palette.rs`

**Interfaces:**
- Consumes: `BankWindow`, `WindowHooks` (Task 4), `open_manager`, `Hooks`, `BankChange` (Task 5), `BankAction`, `BankView`, `Banks` (Task 1), `HubEvent::{Bank, OpenBankManager}`, `ToHub::OpenBankManager` (Task 3), `HubCmd::RunFunctionHeadless(FunctionId)`.
- Produces: `bank_windows::BankWindows` with `new(banks: Rc<RefCell<Banks>>, cmd_tx: Sender<HubCmd>) -> BankWindows`, `restore(&self, app)`, `apply(&self, app, action, name)`, `open_id(&self, app, id)`, `close_id(&self, id)`, `refresh(&self, id)`, `set_desktop_hidden(&self, hidden)`, `begin_shutdown(&self)`; `CommandId::ManageBanks`.

- [ ] **Step 1: `crates/hub/Cargo.toml`**

Add under `[dependencies]`: `opusdm-buttonbank = { path = "../buttonbank" }`.

- [ ] **Step 2: Write `crates/hub/src/bank_windows.rs`**

```rust
//! The open button bank windows. The hub owns them (like the group windows)
//! so they survive Lister restarts; the store is `Banks` on disk. A click
//! runs the button's Function headless. See
//! `docs/superpowers/specs/2026-09-30-button-banks-design.md`.

use crate::server::HubCmd;
use gtk4 as gtk;
use opusdm_buttonbank::window::{BankWindow, WindowHooks};
use opusdm_ipc::buttonbank::{BankAction, BankView};
use opusdm_ipc::function::load_functions;
use opusdm_ipc::Banks;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;

/// One open bank window and whether a `HideBank` hid it.
struct Open {
    window: BankWindow,
    own_hidden: Cell<bool>,
}

/// Every open bank window, keyed by bank id.
pub struct BankWindows {
    banks: Rc<RefCell<Banks>>,
    open: Rc<RefCell<HashMap<String, Open>>>,
    hooks: WindowHooks,
    /// Whether the whole desktop is hidden (`Hide`): every bank window hides.
    desktop_hidden: Cell<bool>,
    /// Set once the hub is quitting, so closing the windows does not clear
    /// their saved `open` state.
    shutting_down: Rc<Cell<bool>>,
}

impl BankWindows {
    /// New, with no window open. A button click sends
    /// `HubCmd::RunFunctionHeadless` on `cmd_tx`.
    pub fn new(banks: Rc<RefCell<Banks>>, cmd_tx: Sender<HubCmd>) -> BankWindows {
        let run: Rc<dyn Fn(&str)> = Rc::new(move |id| {
            let _ = cmd_tx.send(HubCmd::RunFunctionHeadless(id.to_string()));
        });
        let function_exists: Rc<dyn Fn(&str) -> bool> = Rc::new(|id| load_functions().iter().any(|f| f.id == id));
        BankWindows {
            banks,
            open: Rc::new(RefCell::new(HashMap::new())),
            hooks: WindowHooks { run, function_exists },
            desktop_hidden: Cell::new(false),
            shutting_down: Rc::new(Cell::new(false)),
        }
    }

    /// Open every bank that was open when the hub last ran.
    pub fn restore(&self, app: &gtk::Application) {
        let ids: Vec<String> = self.banks.borrow().banks.iter().filter(|b| b.open).map(|b| b.id.clone()).collect();
        for id in ids {
            self.open_id(app, &id);
        }
    }

    /// The current view of bank `id`.
    fn view(&self, id: &str) -> BankView {
        match self.open.borrow().get(id) {
            None => BankView::Closed,
            Some(o) if o.own_hidden.get() => BankView::Hidden,
            Some(_) => BankView::Shown,
        }
    }

    /// Apply a bank command to the bank called `name`. An unknown name is
    /// logged (the Function step already checked it, so this means the bank
    /// was deleted in between).
    pub fn apply(&self, app: &gtk::Application, action: BankAction, name: &str) {
        let Some(id) = self.banks.borrow().get_by_name(name).map(|b| b.id.clone()) else {
            opusdm_ipc::err_log!("hub", "bank command {action:?}: no bank called \"{name}\"");
            return;
        };
        match action.apply(self.view(&id)) {
            BankView::Shown => self.open_id(app, &id),
            BankView::Closed => self.close_id(&id),
            BankView::Hidden => self.hide_id(&id),
        }
    }

    /// Open bank `id`, or show it again when it is open but hidden.
    pub fn open_id(&self, app: &gtk::Application, id: &str) {
        if let Some(open) = self.open.borrow().get(id) {
            open.own_hidden.set(false);
            open.window.set_visible(!self.desktop_hidden.get());
            open.window.present();
            self.remember(id, true);
            return;
        }
        let Some(bank) = self.banks.borrow().get(id).cloned() else { return };
        let window = BankWindow::new(app, &bank, &self.hooks);
        {
            let (open, banks, shutting_down, id) = (self.open.clone(), self.banks.clone(), self.shutting_down.clone(), id.to_string());
            window.connect_close(move |pos| {
                open.borrow_mut().remove(&id);
                if shutting_down.get() {
                    return;
                }
                banks.borrow_mut().set_runtime(&id, false, pos);
                if let Err(e) = banks.borrow().save_bank(&id) {
                    opusdm_ipc::err_log!("hub", "cannot save bank {id}: {e}");
                }
            });
        }
        if !self.desktop_hidden.get() {
            window.present();
        }
        self.open.borrow_mut().insert(id.to_string(), Open { window, own_hidden: Cell::new(false) });
        self.remember(id, true);
    }

    /// Close bank `id`, saving its position and `open: false`.
    pub fn close_id(&self, id: &str) {
        let window = self.open.borrow().get(id).map(|o| o.window.clone());
        match window {
            Some(window) => window.close(),
            None => self.remember(id, false),
        }
    }

    /// Hide the window of bank `id` without closing it.
    fn hide_id(&self, id: &str) {
        if let Some(open) = self.open.borrow().get(id) {
            open.own_hidden.set(true);
            open.window.set_visible(false);
        }
    }

    /// Bring the window of bank `id` in line with the store (after a save),
    /// or close it when the bank is gone.
    pub fn refresh(&self, id: &str) {
        let bank = self.banks.borrow().get(id).cloned();
        let window = self.open.borrow().get(id).map(|o| o.window.clone());
        match (window, bank) {
            (Some(window), Some(bank)) => window.update(&bank, &self.hooks),
            (Some(window), None) => window.close(),
            _ => {}
        }
    }

    /// Hide (`true`) or show (`false`) every bank window with the desktop.
    /// A window a `HideBank` hid stays hidden.
    pub fn set_desktop_hidden(&self, hidden: bool) {
        self.desktop_hidden.set(hidden);
        for open in self.open.borrow().values() {
            open.window.set_visible(!hidden && !open.own_hidden.get());
        }
    }

    /// Save the position of every open window and keep `open: true`, then
    /// stop the close handlers from clearing it. Call before the hub quits.
    pub fn begin_shutdown(&self) {
        self.shutting_down.set(true);
        let positions: Vec<(String, Option<(i32, i32)>)> =
            self.open.borrow().iter().map(|(id, o)| (id.clone(), o.window.position())).collect();
        for (id, pos) in positions {
            self.banks.borrow_mut().set_runtime(&id, true, pos);
            if let Err(e) = self.banks.borrow().save_bank(&id) {
                opusdm_ipc::err_log!("hub", "cannot save bank {id}: {e}");
            }
        }
    }

    /// Record `open` for bank `id` and save it.
    fn remember(&self, id: &str, open: bool) {
        self.banks.borrow_mut().set_runtime(id, open, None);
        if let Err(e) = self.banks.borrow().save_bank(id) {
            opusdm_ipc::err_log!("hub", "cannot save bank {id}: {e}");
        }
    }
}
```

Declare the module where `group_window` is declared (`grep -n "group_window" crates/hub/src/lib.rs crates/hub/src/main.rs`): `pub mod bank_windows;`.

- [ ] **Step 3: Wire `toolbar.rs`**

1. Add to `struct Widgets`:
```rust
    /// The button banks (`buttons/<id>.json`), owned by the GTK side.
    banks: Rc<RefCell<opusdm_ipc::Banks>>,
    /// The open bank windows.
    bank_windows: crate::bank_windows::BankWindows,
```
2. In `build_ui`, before `let widgets = Rc::new(Widgets {`:
```rust
    let banks = Rc::new(RefCell::new(opusdm_ipc::Banks::load()));
    let bank_windows = crate::bank_windows::BankWindows::new(banks.clone(), cmd_tx.clone());
```
   and add `banks, bank_windows,` to the `Widgets { ... }` initializer.
3. After `full_rebuild_desktop_windows(app, &widgets, rects);` add: `widgets.bank_windows.restore(app);`
4. In `apply_hidden`, after `widgets.desktop.group_windows.set_visible(!hidden);` add: `widgets.bank_windows.set_desktop_hidden(hidden);`
5. In `request_quit`'s accepted branch, right after `widgets.quitting.set(true);` add: `widgets.bank_windows.begin_shutdown();`
6. Add helpers near `open_group`:
```rust
/// The callbacks the bank manager and editor use to reach the hub.
fn bank_hooks(widgets: &Rc<Widgets>) -> opusdm_buttonbank::editor::Hooks {
    use opusdm_buttonbank::editor::{BankChange, Hooks};
    let for_changed = widgets.clone();
    let for_open = widgets.clone();
    let for_close = widgets.clone();
    Hooks {
        changed: Rc::new(move |change| match change {
            BankChange::Saved(id) | BankChange::Deleted(id) => for_changed.bank_windows.refresh(&id),
        }),
        open: Rc::new(move |id| {
            if let Some(app) = for_open.primary_window().application() {
                for_open.bank_windows.open_id(&app, id);
            }
        }),
        close: Rc::new(move |id| for_close.bank_windows.close_id(id)),
    }
}

/// Open the button bank manager.
fn open_bank_manager(widgets: &Rc<Widgets>) {
    opusdm_buttonbank::editor::open_manager(&widgets.primary_window(), widgets.banks.clone(), bank_hooks(widgets));
}
```
7. Replace the two temporary arms of `apply_event` from Task 3:
```rust
        HubEvent::Bank { action, bank } => {
            if let Some(app) = widgets.primary_window().application() {
                widgets.bank_windows.apply(&app, action, &bank);
            }
        }
        HubEvent::OpenBankManager => open_bank_manager(widgets),
```
8. In `build_desktop_menu`, right after the "New Group..." block, add:
```rust
    // Button Banks
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Button Banks...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            open_bank_manager(&widgets);
        });
        menu.append(&btn);
    }
```
   and raise `DESKTOP_MENU_H` by one row (e.g. `270` to `300`) so the clamp still fits the taller menu.

- [ ] **Step 4: Lister palette entry**

`crates/lister/src/keys.rs`: add `ManageBanks,` after `ManageFunctions,` in `CommandId` (line ~100). If the file has an exhaustive list of known ids in a test (`known.contains(&CommandId::ManageFunctions)` near line 352), add `assert!(known.contains(&CommandId::ManageBanks));` beside it.

`crates/lister/src/palette.rs`: add the entry next to the `ManageFunctions` one:
```rust
        CommandItem { id: CommandId::ManageBanks, title: Cow::Borrowed("Button Banks..."), category: Cow::Borrowed("Functions"), shortcut: None },
```
and the handler next to `CommandId::ManageFunctions =>`:
```rust
        CommandId::ManageBanks => {
            let _ = ui.hub.client.send(&ToHub::OpenBankManager);
        }
```

- [ ] **Step 5: Build, test, commit**

Run: `cargo build --workspace && cargo test --workspace 2>&1 | tail -30`
Expected: build OK; tests pass (the hub integration tests are `#[ignore]`d as before). Fix any exhaustive-`match` error the compiler reports for `CommandId`.

Run the app to smoke test: `./run-x11.sh` (see the script header for how it starts Xephyr and the hub).
- Desktop right click shows "Button Banks...", it opens the manager.
- Palette (Lister) "Button Banks..." opens the same manager.

```bash
git add crates/hub crates/lister Cargo.lock
git commit -m "feat(hub): own and restore button bank windows, bank commands, manager entry points (F-02)"
```

---

### Task 7: Docs, spec update and acceptance check

**Files:**
- Modify: `CHANGES.md`, `TODO.md`, `docs/usage.md`, `NEXT-PHASE.md`, `docs/superpowers/specs/2026-09-30-button-banks-design.md`

- [ ] **Step 1: Update the spec to match the plan**

In the spec, replace the sentence about `Welcome` carrying the list of names and `BanksUpdate` with: the Function Editor reads the bank names from `~/.config/opusdm/buttons/` when a step form is built. Replace "a new `CommandId::ManageBanks` open the manager. The manager runs in the hub process (like the desktop-config windows), so it needs no extra IPC to save." with: the palette entry sends `ToHub::OpenBankManager`, the manager runs in the hub process and saves directly. Add under "Bank window": position is saved when the window closes and just before the hub quits; the windows hide with the desktop (`Hide`/`Reveal`).

- [ ] **Step 2: Docs**

- `CHANGES.md`: a `## 2026-09-30` entry (append to the existing section for that date if present) describing: `opusdm_ipc::buttonbank`, the new `opusdm-buttonbank` crate (window, manager, editor), `OpenBank`/`CloseBank`/`ToggleBank`/`HideBank` commands, `ToHub::OpenBankManager`, restore at startup, skip taskbar, hide with the desktop.
- `TODO.md`: tick `- [x] F-02 Button Banks + Button Bank Editor (open under the mouse, Start Menu variant and the Lister toolbar are not implemented)` and item `8. [x] Button banks (F-02)`.
- `docs/usage.md`: a new numbered subsection after the last one (find it with `grep -n "^### 3\." docs/usage.md | tail -3`): what a bank is, creating one (desktop menu or palette "Button Banks..."), the button form, the three mouse buttons, the four commands, where the files live, that bank windows are not in the window list.
- `NEXT-PHASE.md`: list the out of scope items: open under the mouse, Start Menu variant, Lister toolbar as a bank, drag and drop reordering, saving the window position on every move.

- [ ] **Step 3: Acceptance check (manual, report the result)**

Run `./run-x11.sh`, then:
1. Create three Functions (Function Editor): e.g. `Notify A`, `Notify B`, `Notify C` with an external step `notify-send A` etc.
2. Desktop menu, "Button Banks...", New "Tools", add 3 buttons, bind Left / Right / Middle on the first to A / B / C, Save, Open.
3. Click the first button with left, right and middle: three different notifications (the middle click runs the alternate Function).
4. Confirm the panel window list shows no entry for the bank window.
5. Move the window, quit the hub (Quit in the desktop menu), start it again: the bank is open at the same place.
6. Make a Function with a `ToggleBank` step bound to a hotkey: it closes and reopens the bank.
7. Delete Function `Notify B`: the button keeps working for A and C; delete all bound Functions of a button: it turns insensitive with a tooltip.
8. `Hide All` from the desktop menu hides the bank, `Reveal` brings it back.

If a check fails, do not tick TODO: fix it (systematic-debugging) and repeat.

- [ ] **Step 4: Commit**

```bash
git add CHANGES.md TODO.md docs/usage.md NEXT-PHASE.md docs/superpowers/specs/2026-09-30-button-banks-design.md docs/superpowers/plans/2026-09-30-button-banks.md
git commit -m "docs: Button Banks (F-02) changes, roadmap step 8 done, usage, plan and spec updates"
```
