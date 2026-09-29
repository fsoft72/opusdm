# OpusDM Hotkeys, User Menu and Script Events (F-03, F-04, F-05) - design

Status: draft, pending user review 2026-09-29. Second step of the Function
system (`FEATURES.md` section 1, item 2 of the suggested order in
`TODO.md`). Attaches three new ways of running a saved `Function` (F-01):
a key press (F-03), an entry in a lister menu (F-04) and a system event
(F-05). All three reference functions by `FunctionId` and are implemented
together, in sequence, from one spec and one plan, with a test run and a
commit after each step.

## Goals

- **F-03 Hotkeys**: bind a key combination to a saved function. A binding is
  either *local* (works when a lister window has focus) or *global* (works
  while any window has focus, X11 only). An editor window manages the
  bindings; a "Find Key" requester tells what a key combination does.
- **F-04 User menu**: a user-editable, nested menu shown as a "Menu"
  drop-down in the lister button row. Entries run functions. Editing is live
  in every open lister.
- **F-05 Script events**: run functions on system events (startup, shutdown,
  lister opened/closed, desktop double clicks).
- One ownership model for all three: the hub loads, validates, persists and
  distributes the configuration, and is the only place functions are started
  from when there is no requesting lister.

## Non-goals

- Rebinding built-in keys or binding keys to built-in commands: a hotkey
  targets a `Function` only. Built-in bindings stay in `keys.rs`.
- Viewer override hooks (Read, HexRead, Show, ...) of F-05: deferred until
  F-16 formalizes the viewer modes.
- Emitting `Hidden`/`Revealed`: the enum variants exist, but nothing emits
  them until F-38 (hide system).
- A native "open lister" step in `InternalCmd`. Opening a lister from a
  function needs an `External` step for now.
- Global hotkeys on Wayland: X11 sessions only (documented, see below).
- Extracting the generic tree editor of `desktop-config/src/menu_ui.rs`
  into `crates/ui`. The menu editor mirrors it, as `function_editor.rs` did.

## Decisions taken during brainstorming

| Topic | Decision |
|---|---|
| Decomposition | One spec, one plan, implemented F-03 then F-04 then F-05 |
| Hotkey target | `Function` only |
| Conflict with a built-in key | The user binding wins; the editor shows an informational note |
| Duplicate user hotkey | An error: the editor blocks Save (see Validation) |
| Global hotkey selection context | Current SOURCE lister (and first DEST), via `QuerySelection` |
| F-05 scope | System events only, no viewer overrides |
| Architecture | Hub is the single owner and dispatcher (approach 1) |

## Data model (`crates/ipc/src/bindings.rs`, new module in `opusdm_ipc`)

Persistence follows `functions.json`: `settings::load_json`/`save_json`
against `config_dir()`, `#[serde(default)]` on every struct so adding a field
never invalidates an existing file.

```rust
/// One key bound to a function. `key` is canonical, e.g. "Ctrl+Alt+u".
pub struct Hotkey { pub key: String, pub function: FunctionId, pub global: bool }

pub enum MenuNode {
    Entry { label: String, function: FunctionId },
    Submenu { label: String, children: Vec<MenuNode> },
    Separator,
}

pub enum ScriptEvent {
    Startup, Shutdown, ListerOpened, ListerClosed,
    DesktopDoubleClick, DesktopMiddleDoubleClick, DesktopRightDoubleClick,
    Hidden, Revealed,
}
pub struct ScriptBinding { pub event: ScriptEvent, pub function: FunctionId }

/// Wire format and in-memory state. Persisted as three files.
pub struct Bindings {
    pub hotkeys: Vec<Hotkey>,       // hotkeys.json
    pub menu: Vec<MenuNode>,        // usermenu.json
    pub scripts: Vec<ScriptBinding>,// scripts.json
}
```

### Key normalization

- A key is the pair (lower-cased keyval, Ctrl/Alt/Shift/Super flags).
  NumLock and CapsLock are ignored.
- The canonical string is `Ctrl+Alt+Shift+Super+<name>` (modifiers in that
  fixed order, then the GDK key name), e.g. `"Ctrl+Alt+u"`, `"Shift+F7"`.
- The key name is the GDK name. GDK keyvals equal X11 keysyms, so the
  lister (`gdk::Key::from_name`) and the hub (keysym for `XGrabKey`) resolve
  the same string with no table of their own.
- The capture field of the editor and the lookup in the lister apply the
  same normalization, so a captured key always matches when pressed again.
- A pure parser in `ipc` splits a canonical string into
  `(Mods, key_name)` and formats it back, so it is unit tested without GTK.

### Validation (`bindings::validate(&Bindings, &known_function_ids)`)

Pure function, shared by the editors (gate Save) and the hub (never trust a
hand-edited file), like `function::validate`. Returns `Vec<BindingError>`:

- **Hotkey**: key does not parse; **duplicate key** (same normalized key in
  two hotkeys, whatever the `global` flag); function id unknown; a global
  hotkey without at least one of Ctrl, Alt, Super (a bare global `c` would
  steal the key from the whole desktop).
- **Menu**: empty label; entry with unknown function id.
- **Scripts**: function id unknown; event not emitted (`Hidden`/`Revealed`
  until F-38); the same (event, function) pair twice.

Editors show the errors inline and keep Save disabled while any exists. A
duplicate key is an error, not a warning.

Hub-side defence: `SetBindings` is revalidated. A `hotkeys.json` edited by
hand with duplicate keys is loaded keeping the first entry and ignoring the
others, with an error in the log; the file is not rewritten.

### Dangling references

A function deleted in the Function Editor leaves ids in the binding files.
At run time the hub logs the missing id and does nothing. The menu renderer
uses `bindings::prune(&[MenuNode], &known_ids) -> Vec<MenuNode>`, which drops
entries with a missing function and submenus left empty. Editors show
dangling entries flagged as errors.

## IPC (`crates/ipc/src/proto.rs`)

```rust
// Lister -> Hub
ToHub::RunFunctionById { id: FunctionId } // hub loads functions.json
ToHub::SetBindings(Bindings)              // editors; hub validates, saves, broadcasts

// Hub -> Lister
ToLister::BindingsUpdate(Bindings)
Welcome { .., bindings: Bindings }        // new field
```

`ToHub::RunFunction { steps }` stays: the Function Editor's Test button runs
functions that are not saved yet. `RunFunctionById` replies to the requester
with `ToLister::FunctionFailed` when the id does not exist.

Each of the three editors starts from the lister's cached copy of
`Bindings` (kept live by `BindingsUpdate`) and replaces only its own
section before sending `SetBindings`, so editors never clobber each other.

## Hub execution model (`crates/hub`)

`start_function_run(cid, steps)` today treats the requesting lister as
SOURCE. It is generalized with an origin:

- **Lister origin** (palette, local hotkey, user menu): unchanged. The
  requester is SOURCE, the first DEST lister other than the requester is
  DEST, selections are collected with `QuerySelection`/`SelectionReply`.
- **Hub origin** (global hotkey, script events, desktop double clicks): the
  requester is the sentinel `u64::MAX`, the same convention as
  `GTK_REQUESTER` for desktop-originated jobs in `server.rs`.
  - SOURCE is the registered lister holding the SOURCE role, DEST is the
    first DEST lister other than SOURCE, selections come from the usual
    `QuerySelection` round trip.
  - With no lister at all, SOURCE is an empty selection with `cwd=$HOME`,
    there is no DEST, and execution starts at once with no round trip.
  - `FunctionOutput`/`FunctionStepDone`/`FunctionFailed`/`FunctionDone` for
    the sentinel requester go to the hub log via `err_log!`, not to a
    lister. No desktop notifications: a function that wants to notify uses a
    `notify-send` step.
- New `HubCmd::RunFunctionHeadless(FunctionId)` on the existing command
  channel (`ServerHandle::cmd_tx`). The global-key thread, event emission
  and the desktop code use it.
- `spawn_executor`/`run_steps` are unchanged.

## F-03 Hotkeys

### Local hotkeys (lister)

- On `Welcome` and on every `BindingsUpdate` the lister rebuilds a
  `HashMap<(Mods, gdk::Key), FunctionId>` holding only the hotkeys with
  `global == false`. Global ones are grabbed by the hub and never reach the
  lister.
- The `EventControllerKey` in `build.rs` consults the map **before**
  `keys::resolve`. A hit sends `ToHub::RunFunctionById` and stops the event.
  The user binding therefore wins over a built-in one.
- While a text field has focus (`Focus::Editing`) a user hotkey applies only
  if it has Ctrl, Alt or Super, or is a function key. A bare `j` never
  steals characters.

### Global hotkeys (hub, X11 only)

- New `crates/hub/src/global_keys.rs`. One `Arc<x11rb::RustConnection>`
  (`RustConnection` is `Sync`, the same crate and connect pattern as
  `desktop.rs`). A thread blocks in `wait_for_event` on `KeyPress`; the
  owner loop changes grabs from another thread on the same connection, so no
  control channel is needed.
- On every change of the global set: `ungrab_key(AnyKey, root)`, then a
  `grab_key` per global key for the four NumLock/CapsLock combinations,
  `GrabModeAsync`. The keysym to keycode map comes from
  `get_keyboard_mapping`. A shared `Mutex<HashMap<(keycode, Mods),
  FunctionId>>` is updated at the same time.
- On `KeyPress` (lock bits masked out) the thread sends
  `HubCmd::RunFunctionHeadless(id)`.
- A rejected grab (`BadAccess`, key already taken, e.g. by the window
  manager) is logged with the key name; the other grabs stay active.
- The X11 connection is created lazily on the first non-empty global set, so
  server tests never touch X11. If `x11rb::connect` fails (pure Wayland) the
  hub logs it once and global bindings stay inert; local bindings are
  unaffected. **Wayland is unsupported for global grabs** and this is
  documented in `docs/usage.md`.
- A global grab takes the key from every window, OpusDM ones included. The
  no-duplicate rule prevents any shadowing between a local and a global
  binding.
- FEATURES.md names XRecord/XInput2; `XGrabKey` is used instead because it
  is enough to run a function on a combination and is much simpler.

### Editor and Find Key (lister)

- Palette command "Edit Hotkeys" (`CommandId::ManageHotkeys`): window modeled
  on `function_editor.rs`. One row per binding: key, function drop-down,
  Global check box. The key is captured with a "Press a key..." button
  (a key controller in capture mode, showing the canonical string). Errors
  from `validate` are inline and disable Save. A key equal to a built-in one
  shows an informational note.
- Palette command "Find Key" (`CommandId::FindKey`): a small requester "press
  a key combination" that shows what it does: a user function (with the
  Global flag), a built-in command (title from the palette entry that
  `keys::resolve` yields), or unassigned.

## F-04 User menu

- A `gtk::MenuButton` "Menu" is appended to `button_row` in `build.rs`. Its
  model is a `gio::Menu` shown in a `gtk::PopoverMenu`, so submenus and
  separators are native.
- One `gio::SimpleAction` `usermenu.run` with a string parameter (the
  `FunctionId`) runs an entry: it sends `ToHub::RunFunctionById` with this
  lister as SOURCE.
- The button is always visible. A fixed last section holds "Edit User
  Menu...". With an empty menu that is the only item, so the feature stays
  discoverable.
- On every `BindingsUpdate` the lister rebuilds the `gio::Menu` from the
  pruned tree and calls `set_menu_model`. All open listers follow with no
  restart.
- Editor: palette command "Edit User Menu" (`CommandId::ManageUserMenu`),
  new `crates/lister/src/menu_editor.rs` mirroring `menu_ui.rs`. Each row has
  up, down, remove and rename; "add entry" (function drop-down, label
  pre-filled with the function name), "add submenu" (asks a name) and "add
  separator". Errors that disable Save: empty label, missing function.
  The same function may appear more than once.

## F-05 Script events

- New `HubCmd::EmitScriptEvent(ScriptEvent)`. The server owner loop finds the
  `ScriptBinding`s for the event and starts a headless run for each. They
  are started in the order of the list and **not** awaited one after the
  other (each run has its own executor thread).
- Emission points:

  | Event | Where |
  |---|---|
  | `Startup` | `main.rs`, after `spawn_server`. The first lister is not registered yet, so the selection is empty with `cwd=$HOME` |
  | `ListerOpened` | `ToHub::Register` handling, after registration |
  | `ListerClosed` | where `HubEvent::ListerGone` is raised (`Goodbye`, `ChildExited`, socket EOF); the exact `drop_conn` refactor is decided in the plan |
  | `Shutdown` | `HubCmd::Shutdown`. No `QuerySelection` (listers are closing): empty selection, `cwd=$HOME`. The hub waits for the runs to finish, up to `SHUTDOWN_HOOK_TIMEOUT` (10 s), before exiting |
  | `DesktopDoubleClick` | `toolbar.rs`: `connect_background_activate` (to verify it fires on a double click) |
  | `DesktopMiddleDoubleClick`, `DesktopRightDoubleClick` | new `gtk::GestureClick` with `n_press == 2` for buttons 2 and 3. The right button already opens the context menu on the first press, so a right double click also opens it: accepted and documented |
  | `Hidden`, `Revealed` | defined, not emitted until F-38 |

- **Loop protection**: a hook on `ListerOpened` that opens a lister would
  trigger `ListerOpened` again. Two guards: an emission of an event is
  dropped (with a log line) while runs of the previous emission of that same
  event are still in flight, and the same event is never emitted twice within
  `EVENT_COOLDOWN` (2 s).
- Editor: palette command "Edit Script Events" (`CommandId::ManageScripts`),
  one row per binding with event and function drop-downs. The event list
  offers only emitted events.

## Testing

Each implementation step runs `cargo test` for the touched crates and
`cargo clippy` before its commit.

- `opusdm_ipc::bindings`: serde round trips for all three files and
  `Bindings`; key parser and formatter (modifier order, NumLock/CapsLock
  ignored, shifted keys); `validate` for every error above, including
  duplicate keys across `global` values; `prune` (missing function, empty
  submenu left behind).
- Protocol: round trip tests for the new messages, as in `proto.rs`.
- Hub (`server.rs` style tests with a real socket): `RunFunctionById` with a
  known and an unknown id; `SetBindings` validation, persistence and
  broadcast; headless run with a SOURCE lister and with no lister at all;
  `EmitScriptEvent` starts the bound functions; the in-flight and cooldown
  guards; `ListerOpened`/`ListerClosed` emission on register and goodbye.
- `global_keys`: the pure part (keysym to keycode table, the four lock
  combinations, lock masking) with a simulated keyboard mapping. No test
  touches a real X server.
- Lister: building and looking up the hotkey map (no display needed, like
  `keys.rs` tests); editing-focus rule for user hotkeys; `MenuNode` to
  `gio::Menu` conversion (item, submenu and separator counts; `gio::Menu` needs
  no display).
- Manual acceptance (GTK and X11 are not unit tested elsewhere in this
  codebase), using `xephyr-sandbox.sh`:
  1. Define function "beep" (`External`: `notify-send hotkey`), bind it to
     `Ctrl+Alt+u` as local; press it in a lister: the notification appears.
  2. Bind the same key again: the editor shows the duplicate error and Save
     stays disabled.
  3. Switch the binding to global and focus a non-OpusDM window: the key
     still runs the function.
  4. Create a nested user menu, run its entries, edit it with two listers
     open: both update with no restart.
  5. Bind a non-recursive function to `ListerOpened`: opening a lister runs
     it. Bind an `External` step that starts a lister to `ListerOpened`: it
     is throttled by the guards instead of looping.
  6. Bind functions to `Startup` and `Shutdown`, restart the hub, and check
     both ran.

## Documentation

`docs/usage.md` gets sections for hotkeys (including the X11-only note for
global ones), the user menu and script events. `CHANGES.md` records each
step. `TODO.md` ticks F-03, F-04, F-05, item 2 of the suggested order, and the
"per-function hotkey" follow-up of F-01, which is realized by
`hotkeys.json` and not by a field on `Function`.

## Implementation order

One commit per step, each with its tests:

1. `ipc::bindings` model, key parser, `validate`, `prune`, persistence, and
   the new protocol messages.
2. Hub: ownership of `Bindings`, `SetBindings`, broadcast and `Welcome`,
   `RunFunctionById`, headless execution (`RunFunctionHeadless`, sentinel
   requester).
3. Lister: local hotkeys (map and dispatch), the hotkey editor and Find Key.
4. Hub: global hotkeys with `XGrabKey`.
5. User menu: rendering and editor.
6. Script events: emission, guards and editor.
7. Documentation (`docs/usage.md`, `CHANGES.md`, `TODO.md`).

## Open questions to resolve during implementation

- Whether `connect_background_activate` fires on a double click of the
  primary button, or a new gesture is needed for it too.
- The exact `drop_conn` refactor that lets every path emit `ListerClosed`.
- The name and location of the hub's command channel entry used by the
  global-key thread (`ServerHandle::cmd_tx` is the starting point).
- Whether `menu_editor.rs` should close on Save or stay open: match what
  `function_editor.rs` does.
