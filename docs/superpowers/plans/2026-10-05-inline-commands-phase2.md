# Inline commands, phase 2 (user menu, script events, button banks, file types) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (the user asked for direct, inline execution). Steps use checkbox syntax.

**Goal:** Everywhere a saved Function can be picked (user menu entries, script events, button bank buttons, file type actions) the user can also type a Bash command or an Exec program, as Edit Hotkeys already allows (phase 1, `docs/superpowers/plans/2026-10-05-inline-commands.md`).

**Architecture:** The reference picker moves from `lister/src/binding_ui.rs` to the shared `opusdm-ui` crate (the button bank editor lives in another crate), gains an optional "none" kind and a `set_reference` method, and every editor swaps its function drop-down for it. The hub-side and ipc-side code from phase 1 already runs and validates inline references; the remaining gaps are the consumers that still treat a reference as a saved id only (`bank_windows::function_exists`, `filetypes_ui::menu_entries`, the menu editor row text).

**Tech Stack:** Rust, GTK4 (gtk4-rs 0.10). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-05-inline-commands-design.md` (section Phases, item 2). Phase 1 rulings still apply: bash scripts get the selection as `"$@"`, no placeholder expansion inside them.

## Global Constraints

- No em dash anywhere; code and comments in English; doc comment on every function; constants UPPER_SNAKE_CASE; exit conditions first; files end with an empty line.
- No new dependencies. Commit per task, only the files changed, never push, no `--no-verify`, no AI attribution lines.
- `CHANGES.md` (top, today) and `docs/usage.md` updated in the task that finishes a behavior.
- Existing JSON files keep loading and saving unchanged.

## Review Focus

- A saved id that is missing still shows "(missing function)" / stays greyed; only inline references change behavior (Tasks 2, 3, 4).
- An optional slot (bank button click, file type double click and drop) set to "none" stores `None`, never an empty string, so `Bank::validate` ("empty Function id") still passes (Tasks 3, 4).
- Typing in a command field of any editor keeps the keyboard focus (no rebuild per keystroke) (Tasks 2, 3, 4).
- A file type "Open With" entry that is an inline command appears in the context menu and runs; one whose saved function is gone is still left out (Task 4).
- The bank editor's function pickers are refilled from the model when another button is selected, without writing back and without firing `on_change` (Tasks 1, 3).

## File Structure

| File | Change |
|---|---|
| `crates/ui/src/reference_picker.rs` (new) | `RefPicker` (moved from lister, plus none kind, `set_reference`), `browsed_line`, `function_dropdown`, `selected_function` |
| `crates/ui/src/lib.rs` | `pub mod reference_picker;` |
| `crates/lister/src/binding_ui.rs` | removed (callers use `opusdm_ui::reference_picker`) |
| `crates/ipc/src/funcref.rs` | `title(reference, functions)` for list texts |
| `crates/lister/src/{hotkey_editor,menu_editor,scripts_editor,filetypes_ui}.rs` | use the picker |
| `crates/buttonbank/src/editor.rs` | three optional pickers |
| `crates/hub/src/bank_windows.rs` | `function_exists` accepts runnable inline commands |

---

### Task 1: Move the picker to `opusdm-ui`, add the none kind and `set_reference`

**Interfaces produced** (used by Tasks 2 to 4):
- `opusdm_ui::reference_picker::RefPicker::new(functions: &[Function], current: &str, parent: &gtk::Window, none_label: Option<&str>, on_change: impl Fn(String) + 'static) -> RefPicker`
- `RefPicker::widget(&self) -> &gtk::Box`, `RefPicker::set_reference(&self, reference: &str)` (does not fire `on_change`), `RefPicker::set_sensitive(&self, on: bool)`.
- With `none_label = Some(text)` the kind drop-down starts with `text` and the empty reference means "none"; with `None` the kinds are Function, Bash, Exec as before.
- `pub fn browsed_line`, `pub fn function_dropdown`, `pub fn selected_function` (same signatures as in `binding_ui.rs`).

- [ ] Write the tests first in `crates/ui/src/reference_picker.rs` (move the three phase 1 `Draft` tests and add): a draft from `""` with none allowed has kind `None` and reference `""`; switching `None` -> `Bash` -> `None` keeps the bash text; `Draft::from_reference("")` without none allowed is `Function` with an empty id. Run `cargo test -p opusdm-ui reference_picker`: RED (module missing).
- [ ] Move the code from `crates/lister/src/binding_ui.rs` into the new module, adding `RefKind::None`, `none_label`, the `loading` guard around every programmatic `set_*` (a `Cell<bool>` checked by every handler), `set_reference` and `set_sensitive`; delete `binding_ui.rs` and its `mod` line in `crates/lister/src/main.rs`; switch `hotkey_editor.rs`, `menu_editor.rs` and `scripts_editor.rs` imports to `opusdm_ui::reference_picker::{function_dropdown, selected_function}` and `RefPicker::new(.., None, ..)` in the hotkey editor.
- [ ] `cargo test --workspace`: all green, no warnings. Commit `refactor(ui): move the reference picker to opusdm-ui and add a none kind`.

### Task 2: User menu and script events editors

**Interfaces consumed:** `RefPicker` (Task 1). **Produced:** `opusdm_ipc::funcref::title(reference: &str, functions: &[Function]) -> String`: saved id -> its name or `(missing function)`; inline -> `describe`.

- [ ] Test first for `title` in `crates/ipc/src/funcref.rs` (saved found, saved missing, bash, exec). RED, then implement. 
- [ ] `menu_editor.rs`: the row text uses `title`; `prompt_add_entry` uses a `RefPicker` (no early return when there are no functions; initial reference is the first function's id or `""`), the label follows the picker's title until the user edits it (compare with the last auto-filled value), and Add refuses an empty label or a reference with `funcref::problem` or an empty saved id.
- [ ] `scripts_editor.rs`: the row uses a `RefPicker`; typing repaints errors in place (per-row `notes` boxes and `revalidate`, same as `hotkey_editor.rs`); "Add Binding" works with no saved function (reference `""`, flagged until chosen).
- [ ] `cargo test --workspace`; commit `feat(lister): user menu and script events can run bash commands and programs`.

### Task 3: Button banks

**Consumed:** `RefPicker` with `none_label = Some(NO_FUNCTION)`; `funcref::{parse, problem}`.

- [ ] Test first in `crates/hub/src/bank_windows.rs` for a pure helper `function_usable(id: &str, saved: &[Function]) -> bool` (saved present, saved missing, inline valid, inline broken). RED, implement, use it in `BankWindows::new`'s `function_exists` closure.
- [ ] `crates/buttonbank/src/editor.rs`: `Form.functions` becomes `[RefPicker; 3]`; `load_form` calls `set_reference(id or "")` and `set_sensitive`; the change handlers map `""` to `None` and write `Some(reference)` otherwise; widgets go into the same `row(...)` helper via `picker.widget()`.
- [ ] `cargo test --workspace`; commit `feat(buttonbank): buttons can run bash commands and programs`.

### Task 4: File types

**Consumed:** `RefPicker`, `funcref::{parse, problem, title}`.

- [ ] Test first in `filetypes_ui.rs`: `menu_entries` keeps an inline command (label `bash: ls`), keeps a saved function, drops a missing saved id and a broken inline command. RED, implement (`check` through `funcref`).
- [ ] `actions_section`: double click and drop use optional `RefPicker`s ("(default application)", "(nothing)"), each Open With entry a required `RefPicker`, "Add Menu Entry" always enabled (pushes the first function id or `""`); the double click path in `window.rs` keeps sending `RunFunctionById`. Remove the now unused `function_at`, `names_with_none`, `function_picker` and the `function_at_skips_the_none_entry` test.
- [ ] `cargo test --workspace`; commit `feat(lister): file type actions can run bash commands and programs`.

### Task 5: Docs and final check

- [ ] `docs/usage.md` (hotkey section note, user menu, script events, button banks, file types): the same two kinds are offered; a short line in each. `CHANGES.md` entry. `FEATURES.md` note under F-04/F-05 if those sections list function bindings. Remove the sentence "will be offered ... in a later change" from section 3.13.
- [ ] `cargo build --workspace` without warnings, `cargo test --workspace` green; commit `docs: inline commands in every function picker`.
- [ ] Final whole-branch review by a fresh reviewer; fix Critical/Important with RED-GREEN tests; record deferred minors.
