# Button Banks (F-02) - design

Date: 2026-09-30. Roadmap item 8 of the suggested order, see `TODO.md` and
section F-02 of `FEATURES.md`.

## Goal

Free-floating windows of buttons ("banks"). Each button has a label and/or an
icon and up to three Functions (F-01) bound to the left, right and middle
click. Banks are saved on disk, reopened when the hub restarts, and driven by
commands (open, close, hide, toggle) that work as Function steps, so hotkeys,
the user menu and script events can use them. A complete editor creates and
edits banks.

Success criteria (acceptance, from `FEATURES.md`): create a bank with 3 buttons
bound to Functions, save it, restart the hub, the bank is restored; a middle
click runs the alternate Function.

## Scope

In scope:
- Bank windows owned by the hub, restored at startup.
- Buttons with text and/or icon, three Functions per button.
- Persistence, one JSON file per bank.
- Commands `OpenBank`, `CloseBank`, `ToggleBank`, `HideBank`.
- Bank manager and bank editor (add, insert, delete, copy, paste, reorder with
  up/down buttons, button form, bank appearance).
- Bank windows never appear in the window list (taskbar) nor in the pager.

Out of scope (recorded in `NEXT-PHASE.md`):
- Open under the mouse and the Start Menu variant (a bank shown as a popup menu).
- Editing the Lister toolbar as a bank (`Lister - Edit Toolbar`).
- Drag and drop reordering (up/down buttons instead, like the other editors).
- Per-bank hotkeys (any command can already be bound to a Function).

## Architecture

Follows the Program Groups precedent (`opusdm_ipc::groups`,
`hub/src/group_window.rs`): the data model lives in `opusdm_ipc`, the UI in its
own crate, and the hub owns the windows so they survive Lister restarts.

Crate layout:

1. `crates/ipc/src/buttonbank.rs` - data model, validation, persistence. No GTK.
2. `crates/buttonbank` (new, GTK4) - `window.rs` (the bank window) and
   `editor.rs` (manager and editor). Depends on `opusdm-ipc`, `opusdm-ui` and
   `gtk4`, like `desktop-config`. It knows nothing about the hub: it receives
   callbacks.
3. `crates/hub` - `bank_windows.rs` wires the crate to the hub: keeps the open
   windows, restores them at startup, handles the bank commands, runs button
   Functions.

## Data model (`opusdm_ipc::buttonbank`)

```
ButtonBank { name, orientation, borderless, columns, open, x, y, buttons: Vec<Btn> }
Btn        { label, icon: Option<PathBuf>, left, right, middle: Option<FunctionId> }
Orientation { Horizontal, Vertical }
```

- `name` is the display name and is unique (case-insensitive). The file name is
  its slug (reusing `groups::slugify`), `~/.config/opusdm/buttons/<slug>.json`.
- `columns` is the number of buttons per row for Horizontal (per column for
  Vertical), clamped to 1..=32, default 4.
- `open`, `x`, `y` are the runtime state saved with the bank so the hub can
  restore it. `x`/`y` are optional in the file (`#[serde(default)]`); a missing
  position lets the window manager place the window.
- `Btn` with an empty label and no icon is valid (it renders as an empty
  button) but the editor warns about it.
- Every field has `#[serde(default)]` so files written by an older version keep
  parsing.

Functions:
- `Banks::load_from_dir(dir)` / `load()`: tolerant loading. A file that fails
  to parse is copied to `<name>.json.bak` (never overwriting an existing `.bak`)
  and skipped, so a later save cannot destroy it.
- `Banks::save_bank_to(dir, name)` / `save_bank(name)`, `remove_bank_file`.
- `validate(&ButtonBank)`: non-empty name, columns range, at most
  `MAX_BUTTONS` (256) buttons, every referenced FunctionId is a non-empty
  string (existence is checked at run time, not at save time, since Functions
  can be deleted later).
- `create`, `rename`, `delete`, `duplicate_name` checks; button operations
  `insert_button`, `remove_button`, `move_button` (up/down), used by the editor
  and unit tested here.

## Bank window (`crates/buttonbank/src/window.rs`)

- One top-level `gtk::Window` per open bank, titled with the bank name.
  Undecorated when `borderless` is set.
- A `gtk::Grid` of buttons following `orientation` and `columns`. Each button
  shows the icon (loaded from the file, scaled to a fixed size) above or beside
  the label. A missing icon file falls back to the label alone.
- Click handling with a `GestureClick` per button (button 1, 2 and 3), which
  calls the `run` callback with the FunctionId bound to that mouse button. A
  button with no Function for that mouse button does nothing. A button whose
  Function no longer exists is shown insensitive with a tooltip
  "Function not found" (the check uses the callback `function_exists`).
- The window reports its position through a close callback, and the hub saves
  it just before quitting, so `x`, `y` and `open` survive a restart. Bank
  windows hide and show with the desktop (`Hide` / `Reveal`).
- The window is marked `_NET_WM_STATE_SKIP_TASKBAR` and
  `_NET_WM_STATE_SKIP_PAGER` once mapped, using the same `x11rb` approach as
  `crates/dock/src/window.rs`. Bank windows therefore never appear in the
  panel's window list or pager. Without X11 the marking is skipped and logged.

## Bank editor (`crates/buttonbank/src/editor.rs`)

Two windows, both modeled on `lister/src/function_editor.rs`:

- Manager: list of banks with New, Edit, Rename, Delete, Open/Close. Rejects a
  duplicate name with an inline error. Deleting asks for confirmation.
- Editor for one bank:
  - Preview of the bank (the same grid widget as the window), with the selected
    button highlighted.
  - Toolbar: Add, Insert (before the selection), Delete, Copy, Paste, Up, Down.
    Copy and Paste use an in-editor clipboard holding one `Btn`.
  - Button form: label entry, icon (file chooser plus clear), and three
    drop-downs (Left, Right, Middle) filled from the saved Functions plus an
    empty "none" entry.
  - Bank appearance: name, orientation, borderless, columns.
  - Save is disabled while `validate` fails and shows the reason. Saving
    persists the bank file and tells the hub to refresh the open window.

## Hub integration (`crates/hub/src/bank_windows.rs`)

- The hub loads all banks at startup and opens those with `open: true`, after
  the desktop is up. The `Startup` script event is unaffected.
- Button clicks call `HubCmd::RunFunctionHeadless(id)`, reusing F-01 (SOURCE is
  the current source Lister, as for hotkeys).
- New `InternalCmd` variants `OpenBank`, `CloseBank`, `ToggleBank`, `HideBank`,
  each with one required parameter `bank` (a bank name). They are handled by the
  hub like `Hide`/`Reveal`/`ToggleHide` (an `FnEvent` reaching the GTK thread),
  with no selection needed. `HideBank` hides the window but keeps `open: true`;
  `CloseBank` sets `open: false`. Opening a bank that is already open presents
  it.
- The Function Editor parameter form gets a drop-down of bank names for the
  `bank` parameter, read from the bank files when the step form is built (no
  protocol change: the files are the source of truth).
- Entry points: a "Button Banks..." item in the desktop menu and in the Command
  Palette (a new `CommandId::ManageBanks`, which sends `ToHub::OpenBankManager`)
  open the manager. The manager runs in the hub process (like the
  desktop-config windows) and saves directly.
- After every save or delete the hub refreshes the affected open window and
  broadcasts the names.

## Error handling

- A bank command on an unknown bank fails the Function step with
  `Bank "<name>" not found`, visible in the Function output like any step error.
- Unwritable buttons directory or failed save: the editor shows the OS error
  and keeps the window open with the unsaved edits.
- Unparseable bank file: backed up as `.bak`, skipped, and logged once at
  startup.
- Errors are never swallowed silently: every failure has a message either in
  the UI or in the log.

## Testing

- `opusdm_ipc::buttonbank` unit tests: serde round trip and defaults, old file
  without new fields, `validate` cases, duplicate and slug collisions,
  `.bak` on a corrupt file (no overwrite of an existing `.bak`), button insert,
  remove and move, clamping of `columns`.
- Pure helpers in `crates/buttonbank` (grid position of the n-th button for
  each orientation, Function-per-mouse-button lookup) unit tested without GTK.
- Hub tests (ignored like the other integration tests, run with the existing
  flags): `OpenBank`/`CloseBank`/`ToggleBank` as Function steps, unknown bank
  error, restore of `open: true` banks at startup.
- Manual check of the acceptance test and of the window list exclusion: open a
  bank and confirm the panel's window list gains no entry.

## Documentation and bookkeeping

`CHANGES.md` entries, tick F-02 and item 8 in `TODO.md`, a new section in
`docs/usage.md`, the out of scope items in `NEXT-PHASE.md`.
