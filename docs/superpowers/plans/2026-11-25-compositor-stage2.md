# OpusDM compositor, stage 2 (OpusDM integration) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** inside `run-native.sh`, the OpusDM panel, pager, tasklist, hub hotkeys and Listers work against `opusdm-compositor` the way they do on X11: 4 desktops, minimize / restore, global hotkeys (on sandbox combos), xdg-activation focus, and a compositor-drawn window menu (Alt+Space).

**Architecture:** the compositor gains a window table with stable ids, desktops and a minimized state (pure model in `src/wm/`), three Wayland protocols served on its own (`wlr-foreign-toplevel-management` v3, `ext-workspace` v1, `xdg-activation` through Smithay), a private IPC (Unix socket, Wayfire framing) with a `watch` event stream, hotkey matching before the focused client, and a window menu drawn with pangocairo. Clients get a native branch chosen only by `opusdm_wayland::native::under_native()`, so X11, labwc and Wayfire take exactly the paths they take today.

**Tech Stack:** Rust (workspace, edition 2021, rust-version 1.92), Smithay 0.7 (features as stage 1, plus only what the build names), `wayland-server` 0.31 / `wayland-protocols` 0.32 (`server`, `staging`) / `wayland-protocols-wlr` 0.3 (`server`), `serde_json`, `pangocairo` 0.21 (same `pango` / `cairo-rs` 0.21.5 already in `Cargo.lock`), `opusdm-ipc`. Tests: headless integration tests (stage 1 harness), `wayland-client` 0.31, smithay-client-toolkit 0.20.

**Spec:** `docs/superpowers/specs/2026-11-25-opusdm-compositor-design.md` (stage 2, plus by the owner's decision: minimize, the window menu with Alt+Space, xdg-activation, the popup-grab fix, the sandbox hotkey table). Stage 1 plan and its ledger: `docs/superpowers/plans/2026-11-25-compositor-stage1.md`, `.superpowers/sdd/2026-11-25-compositor-stage1/progress.md`. Client research: `.superpowers/stage2-research.md`.

## Global Constraints

- Branch `opus-wayland`. Never push, never `--no-verify`, no git worktrees. Commit only the files the task touched; one risky change per commit (project CLAUDE.md rule 6).
- Every rule of the stage 1 plan's Global Constraints still holds (Smithay default features off; no `unwrap` / `expect` / `panic!` / panicking indexing outside tests; `err_log!` / `debug_log!` with tag `"opusdm-compositor"`; English, no em dash character, doc comment on every function, `UPPER_CASE` constants, early exits as one-liners, files end with a newline; `CHANGES.md` entry per task with what was NOT verified; rule 2 ownership answer in commit bodies that create or change ownership).
- **Anti-goal (spec): no regression of the X11, labwc or Wayfire sessions.** Every client change (Tasks 11-13) is taken only when `opusdm_wayland::native::under_native()` is true. After a task that changes `crates/panel` or `crates/hub`, run the project CLAUDE.md rule 4 checks in `run-x11.sh` and a short `run-wayland.sh` (labwc) run in Xephyr, plus `run-native.sh`; never on the owner's `:0`.
- Private IPC: socket `$XDG_RUNTIME_DIR/opusdm-compositor-<WAYLAND_DISPLAY>.sock`, exported to children as `OPUSDM_COMPOSITOR_SOCKET`. Framing: 4-byte little-endian length + JSON. Request `{"method": ..., "data": {...}}`; reply object, with `"error"` on failure. `watch` turns the connection into an event stream.
- The IPC never blocks the event loop: non-blocking calloop sources, `IPC_MAX_MESSAGE = 1 MiB` per incoming message, `IPC_MAX_PENDING = 4 MiB` queued per connection; a connection over either limit or with a protocol error is closed with an `err_log!` line, the server keeps accepting.
- Desktops: count from `DesktopSettings.virtual_screens` (`opusdm_ipc::settings::load_desktop_settings`, range 1..=16, default 4) at startup; changed by `desktops/set-count`. Desktop indices are 0-based on the wire and in code; names shown to users are `"1"`..`"N"`.
- Hotkey matching order (spec): safety bindings, then built-in bindings, then the hub's hotkeys. A matched key press and its release never reach the client. A hub hotkey that matches with no `watch` connection: `err_log!("hotkey {combo}: no hub connected")`, key swallowed.
- Never in the sandbox or in tests: input to `:0`, a kill by name, a write to the owner's `~/.config/opusdm`.

## Review Focus

- The hub restarts (or crashes) while the compositor runs: hotkeys log "no hub connected" meanwhile, and work again once the new hub has sent `hotkeys/set`. Test in Task 5 (watch connection dropped, then a new one) and in Task 12 (hub reconnect loop).
- A window on another desktop or minimized is activated from the tasklist, pager or an activation token: the compositor switches to its desktop, restores it, raises and focuses it. Tests in Tasks 1, 3 and 7.
- The desktop count shrinks while windows live on the removed desktops: they move to the new last desktop, nothing disappears. Test in Task 1 (model) and Task 4 (protocol).
- A client that floods or never reads the IPC socket: it is dropped with a log line, the compositor and the other connections go on. Test in Task 2.
- The window menu opened on a window that is then destroyed (or killed) while the menu is open: the menu closes, no action runs on a dead window, no panic. Test in Task 8.

---

## File map

```
crates/compositor/src/
  wm/mod.rs              window table helpers on State (activate, minimize, desktop moves, sync_space)
  wm/desktops.rs         Desktops model (pure)
  ipc/mod.rs             IpcServer (calloop sources, connections, watch fan-out)
  ipc/frame.rs           framing (pure)
  ipc/protocol.rs        requests -> replies, event JSON (pure over a WindowView snapshot)
  protocols/mod.rs
  protocols/foreign_toplevel.rs   zwlr_foreign_toplevel_manager_v1 v3 server
  protocols/workspace.rs          ext_workspace_manager_v1 v1 server
  activation.rs          XdgActivationHandler
  hotkeys.rs             HotkeyTable, matching (pure parts), compositor.json bindings
  config.rs              CompositorConfig (compositor.json, pure parse)
  menu/mod.rs            WindowMenu state, keyboard / pointer handling
  menu/model.rs          items, navigation (pure)
  menu/theme.rs          themerc parsing (pure)
  menu/render.rs         pangocairo -> MemoryRenderBuffer (nested only)
crates/compositor/tests/common/ipc.rs   harness IPC client
crates/wayland/src/frame.rs              framing shared by wayfire.rs and native.rs
crates/wayland/src/native.rs             under_native, IPC client, watch, model
crates/panel/src/wl_windows.rs           native branch
crates/panel/src/widgets/tasklist.rs     Move to desktop submenu also under native
crates/hub/src/global_keys_wl.rs         native backend
crates/hub/src/toolbar.rs                virtual_screens under native
run-native.sh, docs/WAYLAND-PORT.md, CHANGES.md
```

---

### Task 1: Window table, desktops and minimize in the compositor

**Files:**
- Create: `src/wm/mod.rs`, `src/wm/desktops.rs`
- Modify: `src/state.rs` (`WindowInfo`, `State`), `src/shell/xdg.rs` (`minimize_request`, map / destroy paths), `src/main.rs`, `tests/headless.rs` (stage 1 test `minimize_is_ignored_and_the_window_keeps_getting_frames` is replaced)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)] pub struct WindowId(pub u32)`; ids start at 1, never reused in a run.
  - `WindowInfo` gains `pub id: WindowId`, `pub window: Window`, `pub desktop: u32`, `pub minimized: bool`.
  - `pub struct Desktops { count: u32, current: u32 }` with `pub fn new(count: u32) -> Desktops` (clamped to `DESKTOP_COUNT_RANGE = (1, 16)`), `count()`, `current()`, `pub fn switch_to(&mut self, index: u32) -> bool` (false and no change when out of range), `pub fn set_count(&mut self, count: u32) -> u32` (returns the clamped count; `current` clamped), `pub fn clamp_index(&self, index: u32) -> u32`.
  - `State::desktops: Desktops`, `State::stack: Vec<WindowId>` (stacking order of every toplevel, last = top).
  - `pub fn State::window_by_id(&self, id: WindowId) -> Option<(&WlSurface, &WindowInfo)>`
  - `pub fn State::is_shown(info: &WindowInfo, desktops: &Desktops) -> bool` (has a buffer, not minimized, on the current desktop)
  - `pub fn State::sync_space(&mut self)`: re-maps exactly the shown windows in `stack` order and unmaps the rest; moves the keyboard to the topmost shown window when the focused one is no longer shown (not while an exclusive layer holds it).
  - `pub fn State::activate_window(&mut self, id: WindowId) -> Result<(), String>`: switches to its desktop, clears minimized, raises (top of `stack`), focuses.
  - `pub fn State::set_minimized(&mut self, id: WindowId, minimized: bool) -> Result<(), String>`
  - `pub fn State::move_to_desktop(&mut self, id: WindowId, desktop: u32) -> Result<(), String>`
  - `pub fn State::switch_desktop(&mut self, index: u32) -> Result<(), String>`
  - `pub fn State::set_desktop_count(&mut self, count: u32) -> u32`: windows on removed desktops move to the new last desktop.
  - `pub fn State::close_window(&mut self, id: WindowId) -> Result<(), String>` (`send_close`).
  - `pub const HIDDEN_FRAME_INTERVAL: Duration = Duration::from_secs(1);` hidden windows with a buffer get frame callbacks at this pace.
  - Every function above returns `Err` with a message for an unknown id (the caller logs it). Task 2 adds the event fan-out on top of them.

New windows map on the current desktop. `minimize_request` calls `set_minimized(id, true)` (stage 1 ignored it). An unminimize comes only from activation (tasklist, IPC, foreign-toplevel, xdg-activation), as on Openbox.

- [ ] **Step 1: Write the failing unit tests** (`src/wm/desktops.rs`)

```rust
#[test]
fn switching_stays_inside_the_count() {
    let mut d = Desktops::new(4);
    assert!(d.switch_to(3));
    assert!(!d.switch_to(4));
    assert_eq!(d.current(), 3);
}

#[test]
fn shrinking_clamps_the_current_desktop_and_the_count() {
    let mut d = Desktops::new(4);
    d.switch_to(3);
    assert_eq!(d.set_count(2), 2);
    assert_eq!(d.current(), 1);
    assert_eq!(d.set_count(0), 1);
    assert_eq!(Desktops::new(40).count(), 16);
}
```

- [ ] **Step 2: Write the failing integration tests** (`tests/headless.rs`; replace the stage 1 minimize test)

```rust
#[test]
fn a_minimized_window_loses_the_keyboard_and_still_gets_frames() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    b.xdg().set_minimized();
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
    assert!(c.wait_frame(&b, Duration::from_secs(3)));
}
```

- [ ] **Step 3: Run them to see them fail** - `cargo test -p opusdm-compositor wm` and `--test headless minimized`.
- [ ] **Step 4: Implement** the table, `Desktops`, the `State` functions above, `sync_space`, the 1 s frame pace for hidden windows in `send_frames`; read `virtual_screens` at startup in `State::new` callers (`backend/mod.rs` `setup_common`) with `load_desktop_settings("opusdm-compositor")`. Keep every stage 1 test green (they map on desktop 0).
- [ ] **Step 5: Run** `cargo test -p opusdm-compositor && cargo clippy -p opusdm-compositor --all-targets -- -D warnings`, headless loop 5 times.
- [ ] **Step 6: Commit** `compositor: window ids, desktops and minimize`, rule 2 answer (who owns hidden windows; what happens to them when their client dies).

---

### Task 2: Private IPC server

**Files:**
- Create: `src/ipc/mod.rs`, `src/ipc/frame.rs`, `src/ipc/protocol.rs`, `tests/common/ipc.rs`
- Modify: `src/state.rs` (`ipc: IpcServer`, a `State::window_views()` snapshot), `src/backend/mod.rs` (socket bound next to the Wayland socket, removed at exit), `src/child.rs` (`OPUSDM_COMPOSITOR_SOCKET` set for children), `src/wm/mod.rs` (each state change calls `self.ipc.broadcast(...)`), `src/args.rs` / `State` (`test_input: bool`, true only for `--headless`)

**Interfaces:**
- Consumes: Task 1 functions.
- Produces:
  - `pub fn ipc::frame::encode(json: &[u8]) -> Vec<u8>`; `pub fn ipc::frame::decode(buf: &mut Vec<u8>, max: usize) -> Result<Option<Vec<u8>>, String>` (one whole message out of the buffer, `Ok(None)` when incomplete, `Err` over `max`).
  - `pub struct WindowView { id: u32, app_id: String, title: String, pid: Option<u32>, geometry: Rect, desktop: u32, minimized: bool, maximized: bool, fullscreen: bool, activated: bool }` (serde `Serialize`, JSON keys as written, `geometry` as `{"x","y","width","height"}`).
  - `pub fn ipc::protocol::window_json(v: &WindowView) -> serde_json::Value`
  - Methods (all replies `{"ok": true, ...}` or `{"error": "..."}`):

| Method | Data | Reply / effect |
|---|---|---|
| `windows/list` | - | `{"ok":true,"windows":[WindowView...]}` in `stack` order |
| `desktops/list` | - | `{"ok":true,"count":N,"current":i}` |
| `window/activate` | `{"id"}` | `State::activate_window` |
| `window/minimize` | `{"id","state":bool}` | `State::set_minimized` |
| `window/close` | `{"id"}` | `State::close_window` |
| `window/kill` | `{"id"}` | SIGKILL to the client pid (from `Client::get_credentials`); refused for a pid `<= 1` or the compositor's own pid |
| `window/move-to-desktop` | `{"id","desktop"}` | `State::move_to_desktop` |
| `desktops/switch` | `{"index"}` | `State::switch_desktop` |
| `desktops/set-count` | `{"count"}` | `State::set_desktop_count`, reply has the clamped count |
| `session/quit` | - | sets `quit` (exit code 0) |
| `watch` | - | reply `{"ok":true}`, then events on this connection |
| `debug/key` | `{"key":"Ctrl+Alt+u"}` | only when `test_input`; otherwise error. Press + release of the combo through `State::keyboard_key` (keysyms looked up in the current keymap) |

  - Events (`watch`): `{"event":"window-added","window":{...}}`, `window-changed`, `{"event":"window-removed","id":N}`, `{"event":"desktops-changed","count":N,"current":i}`, and (Task 5) `{"event":"hotkey","function":"..."}`.
  - `pub fn IpcServer::broadcast(&mut self, event: &serde_json::Value)` and `pub fn IpcServer::has_watchers(&self) -> bool`.
  - Harness (`tests/common/ipc.rs`): `pub fn ipc_request(comp: &Compositor, method: &str, data: serde_json::Value) -> serde_json::Value` (2 s timeout), `pub struct IpcWatch` with `pub fn ipc_watch(comp: &Compositor) -> IpcWatch` and `pub fn next_event(&mut self, timeout: Duration) -> Option<serde_json::Value>`. `Compositor` learns the IPC socket path from a second stdout line `OPUSDM_COMPOSITOR_SOCKET=<path>`.

- [ ] **Step 1: Write the failing tests**: unit tests for `encode` / `decode` (round trip; a message split in two reads; length over `max` is an error) and these integration tests:

```rust
#[test]
fn the_ipc_lists_windows_and_minimizes_one() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let list = ipc_request(&comp, "windows/list", json!({}));
    let id = list["windows"][0]["id"].as_u64().unwrap();
    assert_eq!(list["windows"][0]["desktop"], 0);
    ipc_request(&comp, "window/minimize", json!({"id": id, "state": true}));
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["windows"][0]["minimized"], true);
}

#[test]
fn a_window_moved_to_another_desktop_comes_back_when_activated() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 2}));
    ipc_request(&comp, "window/activate", json!({"id": id}));
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 2);
}

#[test]
fn shrinking_the_desktops_keeps_every_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 3}));
    assert_eq!(ipc_request(&comp, "desktops/set-count", json!({"count": 2}))["count"], 2);
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["desktop"], 1);
}

#[test]
fn a_watcher_sees_windows_come_and_go() {
    let comp = Compositor::start();
    let mut watch = ipc_watch(&comp);
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert_eq!(watch.next_event(Duration::from_secs(2)).unwrap()["event"], "window-added");
    drop(w);
    c.roundtrip();
    // window-changed events may come first; a window-removed must follow
    assert!(watch.wait_for("window-removed", Duration::from_secs(2)));
}

#[test]
fn a_connection_that_sends_garbage_is_dropped_and_the_server_goes_on() {
    let comp = Compositor::start();
    ipc_send_raw(&comp, &(u32::MAX).to_le_bytes()); // a length over IPC_MAX_MESSAGE
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
}

#[test]
fn session_quit_ends_the_compositor_cleanly() {
    let mut comp = Compositor::start();
    ipc_request(&comp, "session/quit", json!({}));
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
}
```

(`IpcWatch::wait_for(event, timeout) -> bool` and `ipc_send_raw` are harness helpers. `Compositor::start` sets `XDG_CONFIG_HOME` and `HOME` to the test's temp dir, so `desktop.json` and `compositor.json` are absent, the count is the default 4 and the owner's config is never read by tests.)

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** the server: a calloop `Generic` on a non-blocking `UnixListener`; each connection a `Generic` source with its read buffer and pending-write queue (registered for writability only while it has pending bytes); `IPC_MAX_MESSAGE`, `IPC_MAX_PENDING` as in Global Constraints. A stale socket file is removed before bind; the file is removed when the loop ends. Every refusal (unknown method, bad data, unknown id, kill refused, `debug/key` without `test_input`) is an `{"error"}` reply plus `err_log!`. `child_env` gains `("OPUSDM_COMPOSITOR_SOCKET", Some(path))` (update its unit test).
- [ ] **Step 4: Run** crate tests, clippy, headless loop 5 times.
- [ ] **Step 5: Commit** `compositor: private IPC with window and desktop control`, rule 2 answer (connections owned by `IpcServer` in `State`; a dead or flooding client is dropped and its source removed; the socket file goes with the loop).

---

### Task 3: wlr-foreign-toplevel-management

**Files:**
- Create: `src/protocols/mod.rs`, `src/protocols/foreign_toplevel.rs`
- Modify: `Cargo.toml` (`wayland-protocols-wlr = { version = "0.3", features = ["server"] }`), `src/state.rs`, `src/shell/xdg.rs` (`title_changed`, `app_id_changed`, map / unmap / destroy / state changes call the protocol), `src/wm/mod.rs`, `tests/common/mod.rs` (a foreign-toplevel listener on `TestClient`), `tests/headless.rs`

**Interfaces:**
- Consumes: Task 1 (`WindowId`, `activate_window`, `set_minimized`, `close_window`), stage 1 maximize / fullscreen requests.
- Produces: global `zwlr_foreign_toplevel_manager_v1` version 3. Per shown-or-hidden toplevel with a buffer and per bound manager: a handle with `title`, `app_id`, `output_enter` (the output, for every bound `wl_output` of that client), `state` (`maximized` 0, `minimized` 1, `activated` 2, `fullscreen` 3), `done`; `closed` when the window goes. Requests: `activate` -> `activate_window`; `set_minimized` / `unset_minimized` -> `set_minimized` (`unset` = activate, as Openbox); `close`; `set_maximized` / `unset_maximized` / `set_fullscreen` / `unset_fullscreen` -> the stage 1 request paths; `set_rectangle` ignored with `debug_log!`; `destroy`. Requests on a handle whose window is gone are ignored with `debug_log!`. `pub fn State::foreign_toplevel_update(&mut self, id: WindowId)` sends the changed fields + `done`.
- Harness: `TestClient::toplevels(&mut self) -> Vec<ForeignToplevel>` with `ForeignToplevel { title, app_id, states: Vec<u32>, outputs: usize, handle }`, and `activate(&self, t: &ForeignToplevel)` / `minimize(...)` request helpers.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn foreign_toplevels_list_title_app_id_and_states() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_title("Lister".into());
    w.xdg().set_app_id("opusdm-lister".into());
    w.wl_surface().commit();
    let t = c.toplevels();
    assert_eq!((t[0].title.as_str(), t[0].app_id.as_str()), ("Lister", "opusdm-lister"));
    assert!(t[0].states.contains(&2)); // activated
    assert_eq!(t[0].outputs, 1);
}

#[test]
fn a_foreign_toplevel_minimize_and_activate_round_trip() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let t = c.toplevels().remove(0);
    c.minimize(&t);
    assert!(c.toplevels()[0].states.contains(&1));
    c.activate(&t);
    let states = &c.toplevels()[0].states;
    assert!(!states.contains(&1) && states.contains(&2));
}

#[test]
fn a_fullscreen_window_is_reported_fullscreen() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_fullscreen(None);
    c.roundtrip();
    assert!(c.toplevels()[0].states.contains(&3));
}

#[test]
fn a_destroyed_window_is_closed_for_foreign_toplevel_clients() {
    let comp = Compositor::start();
    let mut watcher = TestClient::new(&comp);
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert_eq!(watcher.toplevels().len(), 1);
    drop(w);
    c.roundtrip();
    assert_eq!(watcher.toplevels().len(), 0);
}
```

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** the global with `wayland-server` `GlobalDispatch` / `Dispatch` on `State` (handles kept in `State::foreign_toplevels: Vec<(WindowId, ZwlrForeignToplevelHandleV1)>`, removed on `closed` / client death).
- [ ] **Step 4: Run** crate tests, clippy, headless loop 5 times.
- [ ] **Step 5: Commit** `compositor: wlr-foreign-toplevel-management`, rule 2 answer.

---

### Task 4: ext-workspace

**Files:**
- Create: `src/protocols/workspace.rs`
- Modify: `Cargo.toml` (`wayland-protocols = { version = "0.32", features = ["server", "staging"] }`), `src/state.rs`, `src/wm/mod.rs` (desktop changes notify the protocol), `tests/common/mod.rs`, `tests/headless.rs`

**Interfaces:**
- Consumes: Task 1 `Desktops`, `switch_desktop`, `set_desktop_count`.
- Produces: global `ext_workspace_manager_v1` version 1. One `workspace_group` (`capabilities` 0, `output_enter` for the client's `wl_output`), `N` workspaces: `id` `"opusdm-desktop-<index>"`, `name` `"1"`..`"N"`, `coordinates` `[index]` (one u32, little-endian array), `capabilities` `activate`, `state` `active` on the current one. `activate` is recorded and applied on the manager's `commit` (`switch_desktop`); `deactivate`, `assign`, `remove`, `create_workspace` are ignored with `debug_log!`. Count changes send `workspace_enter` / `removed` and `done`. `pub fn State::workspaces_update(&mut self)`.
- Harness: `TestClient::workspaces(&mut self) -> Vec<TestWorkspace>` (`name`, `active`, `coordinates`), `activate_workspace(&mut self, index: usize)` (activate + commit).
- Client side check (no change expected): `opusdm_wayland::pager_order` sorts by reversed coordinates; with one coordinate this is the index order. Add a unit test in `crates/wayland/src/model.rs` that pins it: `pager_order` of workspaces with coordinates `[2]`, `[0]`, `[1]` returns them in `0, 1, 2` order.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn four_workspaces_with_the_first_active() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let ws = c.workspaces();
    assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4"]);
    assert!(ws[0].active && !ws[1].active);
}

#[test]
fn activating_a_workspace_switches_the_desktop() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    c.activate_workspace(2);
    assert!(c.workspaces()[2].active);
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 2);
}

#[test]
fn a_new_desktop_count_reaches_workspace_clients() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    assert_eq!(c.workspaces().len(), 4);
    ipc_request(&comp, "desktops/set-count", json!({"count": 6}));
    assert_eq!(c.workspaces().len(), 6);
}
```

- [ ] **Step 2: Run them to see them fail** (and the `pager_order` unit test passes already: it pins today's behaviour).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** crate tests, `cargo test -p opusdm-wayland`, clippy, headless loop 5 times.
- [ ] **Step 5: Commit** `compositor: ext-workspace desktops`.

---

### Task 5: Hotkeys, safety and built-in bindings

**Files:**
- Create: `src/hotkeys.rs`, `src/config.rs`
- Modify: `src/input.rs` (`keyboard_key` matches before forwarding), `src/ipc/mod.rs` (`hotkeys/set`, `hotkey` events), `src/state.rs`, `src/backend/mod.rs` (config loaded at startup), `tests/headless.rs`

**Interfaces:**
- Consumes: `opusdm_ipc::bindings::{parse_key, KeyCombo, Mods}`; Task 2 IPC.
- Produces:
  - `pub struct CompositorConfig { pub window_menu: String, pub quit: String, pub quit_enabled: bool }`, defaults `"Alt+space"`, `"Ctrl+Alt+BackSpace"`, `true`; `pub fn CompositorConfig::parse(json: &str) -> Result<CompositorConfig, String>` (missing keys take the default); `pub fn CompositorConfig::load() -> CompositorConfig` reads `$OPUSDM_COMPOSITOR_CONFIG` or `config_dir()/compositor.json` read only; a missing file is the defaults with a `debug_log!`, a bad file is the defaults with an `err_log!`.
  - `pub fn combo_matches(combo: &KeyCombo, mods: Mods, keysym_name: &str) -> bool`: modifiers equal exactly (Caps / Num Lock ignored), names compared case-insensitively for single letters, exactly otherwise.
  - `pub enum Binding { Quit, WindowMenu, Hub(String) }` and `pub struct HotkeyTable` with `pub fn HotkeyTable::new(config: &CompositorConfig) -> HotkeyTable`, `pub fn set_hub(&mut self, entries: &[(String, String)]) -> Vec<String>` (returns the errors: unparsable keys, and conflicts with a built-in, logged as `hotkey <combo>: <function> conflicts with the built-in <name>, the built-in wins`), `pub fn lookup(&self, mods: Mods, keysym_name: &str) -> Option<Binding>` in the order Quit (when enabled), WindowMenu, Hub.
  - Keysym used for matching: `KeysymHandle::raw_latin_sym_or_raw_current_sym()` (layout independent), name from `xkb::keysym_get_name`.
  - IPC `hotkeys/set` `{"hotkeys":[{"key","function"}...]}` replaces the table; reply `{"ok":true,"errors":[...]}`. The connection that sent it is "the hub": when it closes, `err_log!("hub disconnected")` (spec); the table stays, so hotkeys keep being matched and log "no hub connected" until a new hub sends its table. A matched Hub binding sends `{"event":"hotkey","function":...}` to every watcher; with none, the "no hub connected" log. `WindowMenu` calls `State::open_window_menu()` (a stub that `debug_log!`s until Task 8). Matched presses record their keycode; the release of a recorded keycode is swallowed too.
- [ ] **Step 1: Write the failing tests**: unit tests for `combo_matches` (`Ctrl+Alt+u` matches ctrl+alt with `u` and `U`, not ctrl+alt+shift; `Ctrl+backslash` matches `backslash`; `Super+F1` needs super) and `CompositorConfig::parse` (empty object -> defaults; `{"quit_enabled": false}`), `set_hub` reporting a conflict with `Alt+space`, and these integration tests:

```rust
#[test]
fn a_hub_hotkey_reaches_the_watcher_and_not_the_client() {
    let comp = Compositor::start();
    let mut watch = ipc_watch(&comp);
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    ipc_request(&comp, "hotkeys/set", json!({"hotkeys": [{"key": "Ctrl+backslash", "function": "terminal"}]}));
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(watch.wait_for_hotkey("terminal", Duration::from_secs(2)));
    assert_eq!(c.keys_received(), 0); // new harness helper: counts wl_keyboard.key events
}

#[test]
fn hotkeys_work_again_after_the_hub_reconnects() {
    let comp = Compositor::start();
    ipc_request(&comp, "hotkeys/set", json!({"hotkeys": [{"key": "Ctrl+backslash", "function": "terminal"}]}));
    drop(ipc_watch(&comp)); // the old hub goes away
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"})); // logged, swallowed
    let mut watch = ipc_watch(&comp);
    ipc_request(&comp, "hotkeys/set", json!({"hotkeys": [{"key": "Ctrl+backslash", "function": "terminal"}]}));
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+backslash"}));
    assert!(watch.wait_for_hotkey("terminal", Duration::from_secs(2)));
}

#[test]
fn the_quit_binding_ends_the_compositor() {
    let mut comp = Compositor::start();
    ipc_request(&comp, "debug/key", json!({"key": "Ctrl+Alt+BackSpace"}));
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
}

#[test]
fn an_unbound_key_still_reaches_the_client() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    ipc_request(&comp, "debug/key", json!({"key": "a"}));
    c.roundtrip();
    assert_eq!(c.keys_received(), 2); // press and release
}
```

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** The VT-switch safety bindings stay for stage 4 (no VTs in the nested backend).
- [ ] **Step 4: Run** crate tests, clippy, headless loop 5 times.
- [ ] **Step 5: Commit** `compositor: global hotkeys, quit and window menu bindings`.

---

### Task 6: An exclusive layer ends active popup grabs

**Files:**
- Modify: `src/shell/layer.rs` (where an exclusive layer takes the keyboard), `tests/headless.rs`

**Interfaces:**
- Consumes: stage 1 `exclusive_layer`, the popup grab of `XdgShellHandler::grab`.
- Produces: when a TOP / OVERLAY layer with exclusive keyboard interactivity maps (or becomes exclusive) while a popup grab is active, the grab is ended first (`PopupManager::dismiss_popup` on the grabbed root's popups, which sends `popup_done`, then keyboard and pointer grabs unset), then the layer takes the keyboard. `debug_log!` the dismissal.

- [ ] **Step 1: Write the failing test** (reuses the stage 1 trick of grabbing with the keyboard-enter serial; the harness helpers `map_popup_with_grab` and `popup_done` exist since the stage 1 final fixes or are added here under these names)

```rust
#[test]
fn an_exclusive_layer_mapped_over_an_open_menu_keeps_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let menu = c.map_popup_with_grab(&w, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert!(c.popup_done(&menu));
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
}
```

- [ ] **Step 2: Run it to see it fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** crate tests, clippy, headless loop 5 times.
- [ ] **Step 5: Commit** `compositor: an exclusive keyboard layer closes open menus`.

---

### Task 7: xdg-activation

**Files:**
- Create: `src/activation.rs`
- Modify: `src/state.rs` (`XdgActivationState`, delegate), `src/shell/xdg.rs` (activation pending until a window maps), `tests/common/mod.rs`, `tests/headless.rs`

**Interfaces:**
- Consumes: Task 1 `activate_window`.
- Produces: `XdgActivationHandler for State`. `token_created`: accepted (returns true), `debug_log!` with the app id. `request_activation(token, data, surface)`: when the token is younger than `ACTIVATION_TOKEN_TIMEOUT = Duration::from_secs(10)` and the surface is a known toplevel, `activate_window` (desktop switch, restore, raise, focus); a surface not mapped yet keeps the request in `WindowInfo::pending_activation` and is activated when it maps; an expired or unknown token is refused with `err_log!` (no focus change). The token is removed after use.
- Harness: `TestClient::activation_token(&mut self) -> String`, `request_activation(&mut self, w: &TestWindow, token: &str)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_valid_activation_token_raises_and_focuses_a_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let _b = c.map_toplevel((400, 300)); // b has the keyboard
    let token = c.activation_token();
    c.request_activation(&a, &token);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
}

#[test]
fn activation_restores_a_window_on_another_desktop() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 1}));
    let token = c.activation_token();
    c.request_activation(&a, &token);
    c.roundtrip();
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 1);
}

#[test]
fn an_unknown_token_changes_nothing() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((400, 300));
    c.request_activation(&a, "not-a-token");
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(b.wl_surface().clone()));
}
```

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** with Smithay's `wayland::xdg_activation` (it needs no extra feature; if the build names one, add only that).
- [ ] **Step 4: Run** crate tests, clippy, headless loop 5 times.
- [ ] **Step 5: Commit** `compositor: xdg-activation`.

---

### Task 8: Window menu (model, theme, input)

**Files:**
- Create: `src/menu/mod.rs`, `src/menu/model.rs`, `src/menu/theme.rs`
- Modify: `src/state.rs` (`menu: Option<WindowMenu>`), `src/input.rs` (the menu takes keyboard and pointer while open), `src/hotkeys.rs` (`WindowMenu` binding opens it), `tests/headless.rs`

**Interfaces:**
- Consumes: Task 1 window functions, Task 2 `window/kill` logic (factor the kill into `State::kill_window(id) -> Result<(), String>`), Task 5 `Binding::WindowMenu`.
- Produces:
  - `pub enum MenuAction { ToggleMaximize, Minimize, MoveToDesktop(u32), Close, Kill }`
  - `pub struct MenuItem { pub label: String, pub action: Option<MenuAction>, pub enabled: bool, pub submenu: Vec<MenuItem> }`; `pub fn menu_items(maximized: bool, desktops: &Desktops, window_desktop: u32) -> Vec<MenuItem>`: `"Restore"` or `"Maximize"`, `"Minimize"`, `"Move to desktop"` with `"1"`..`"N"` (the window's own desktop disabled), a separator (`label` empty, no action), `"Close"`, `"Kill"`.
  - `pub struct MenuNav` (pure): `pub fn key(&mut self, key: MenuKey) -> Option<MenuAction>` with `MenuKey { Up, Down, Left, Right, Enter, Escape }` (Up / Down skip separators and disabled items and wrap; Right / Enter on `"Move to desktop"` opens the submenu; Left closes it; Enter returns the action; Escape closes), `pub fn closed(&self) -> bool`, `pub fn selected(&self) -> (usize, Option<usize>)`.
  - `pub struct MenuTheme { item_bg: (Rgb, Rgb), item_text: Rgb, disabled_text: Rgb, active_bg: (Rgb, Rgb), active_text: Rgb, border_width: u32, padding: u32 }`; `pub fn MenuTheme::parse(themerc: &str) -> MenuTheme` (keys `menu.items.bg.color` / `colorTo`, `menu.items.text.color`, `menu.items.disabled.text.color`, `menu.items.active.bg.color` / `colorTo`, `menu.items.active.text.color`, `border.width`, `padding.width`; missing keys keep defaults equal to OpusDMTheme's values); `pub fn MenuTheme::load() -> MenuTheme` searching `$XDG_DATA_HOME/themes`, `~/.themes`, `/usr/share/themes` for `OpusDMTheme/openbox-3/themerc` (not found: defaults + `err_log!` once).
  - `pub fn State::open_window_menu(&mut self)`: on the keyboard-focused window (none: `debug_log!`, nothing opens), placed at the window's top-left corner, kept inside the output. While open, keys go to `MenuNav` (never to clients) and pointer clicks inside select, outside close it. The menu closes, with no action, when its window goes (destroy, client death, kill).
  - Item geometry for hit-testing is computed in `menu/mod.rs` from fixed metrics (`MENU_ITEM_HEIGHT = 24`, `MENU_WIDTH = 200`, `MENU_SEPARATOR_HEIGHT = 9`), shared with Task 9's renderer.

- [ ] **Step 1: Write the failing tests**: unit tests for `menu_items` (labels for a maximized and a normal window; the current desktop disabled in the submenu), `MenuNav` (Down from the first item skips the separator; Right on "Move to desktop" then Down then Enter returns `MoveToDesktop(1)` when the window is on desktop 0; Escape closes), `MenuTheme::parse` on the repo's `assets/openbox-share/themes/OpusDMTheme/openbox-3/themerc` (`active_bg.0` is `#6d95de`), and these integration tests:

```rust
#[test]
fn the_window_menu_minimizes_the_focused_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    for key in ["Alt+space", "Down", "Return"] {
        ipc_request(&comp, "debug/key", json!({"key": key}));
    }
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["minimized"], true);
    assert_eq!(c.keys_received(), 0);
}

#[test]
fn the_window_menu_closes_when_its_window_goes() {
    let mut comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    ipc_request(&comp, "debug/key", json!({"key": "Alt+space"}));
    drop(w);
    c.roundtrip();
    ipc_request(&comp, "debug/key", json!({"key": "Return"})); // no window: nothing runs
    let mut d = TestClient::new(&comp);
    let _x = d.map_toplevel((400, 300));
    ipc_request(&comp, "debug/key", json!({"key": "a"}));
    d.roundtrip();
    assert_eq!(d.keys_received(), 2); // the menu is gone, keys reach clients again
    assert!(comp.is_running());
}
```

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** (no drawing yet: the headless backend has no renderer).
- [ ] **Step 4: Run** crate tests, clippy, headless loop 5 times.
- [ ] **Step 5: Commit** `compositor: window menu (Alt+Space) with OpusDMTheme colours`.

---

### Task 9: Window menu drawing (nested)

**Files:**
- Create: `src/menu/render.rs`
- Modify: `Cargo.toml` (`pangocairo = "0.21"`, `cairo-rs = "0.21"` at the versions already in `Cargo.lock`), `src/backend/nested.rs` (the menu element above OVERLAY, below the cursor)

**Interfaces:**
- Consumes: Task 8 `WindowMenu`, `MenuTheme`, metrics.
- Produces: `pub fn render_menu(menu: &WindowMenu, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String>`: an ARGB8888 image drawn with cairo (vertical gradient `color` -> `colorTo` for item and active backgrounds, border) and pango (font `Sans 10`, labels left-aligned with `padding`, a `>` arrow on submenu items, disabled text colour); rebuilt only when the menu state changes (cached in `WindowMenu`). A render error is logged once per second (stage 1 `ErrorThrottle`) and the menu stays usable by keyboard.

No headless test can see pixels. Unit test: `render_menu` returns a buffer of `MENU_WIDTH x (items height)` for the default items.

- [ ] **Step 1: Write the failing unit test**, run it to see it fail.
- [ ] **Step 2: Implement**, run crate tests and clippy.
- [ ] **Step 3: Check by hand in Xephyr** (`Xephyr :7`, `OPUSDM_COMPOSITOR_CONFIG` pointing to a scratch `compositor.json` with `"window_menu": "Super+Alt+space"` because the host Openbox takes Alt+Space, `foot` as startup app): open the menu, screenshot (`import -display :7`), move with arrows into "Move to desktop", Escape; open again and Minimize. Record PASS / FAIL / NOT CHECKED in `CHANGES.md`.
- [ ] **Step 4: Commit** `compositor: draw the window menu`.

---

### Task 10: `opusdm_wayland::native` client

**Files:**
- Create: `crates/wayland/src/frame.rs`, `crates/wayland/src/native.rs`
- Modify: `crates/wayland/src/wayfire.rs` (uses `frame.rs`; behaviour unchanged, its tests stay green), `crates/wayland/src/lib.rs`

**Interfaces:**
- Consumes: Task 2 wire format.
- Produces:
  - `pub(crate) fn frame::send(stream: &mut UnixStream, method: &str, data: Value) -> Result<(), String>`, `pub(crate) fn frame::receive(stream: &mut UnixStream, max: usize) -> Result<Value, String>` (moved from `wayfire.rs`).
  - `pub const ENV_SOCKET: &str = "OPUSDM_COMPOSITOR_SOCKET";` `pub fn native::under_native() -> bool` (variable set and the socket exists).
  - `pub struct NativeWindow { pub id: u32, pub app_id: String, pub title: String, pub pid: Option<u32>, pub geometry: Rect, pub desktop: u32, pub minimized: bool, pub maximized: bool, pub fullscreen: bool, pub activated: bool }`, `pub struct NativeState { pub windows: Vec<NativeWindow>, pub count: u32, pub current: u32 }` with `pub fn NativeState::read() -> Result<NativeState, String>` and `pub fn active(&self) -> Option<&NativeWindow>`.
  - `pub fn native::watch(on_change: impl FnMut(&NativeState) + Send + 'static) -> Result<(), String>`: same shape as `wayfire::watch` (reader thread, re-read on events, bursts merged).
  - `pub fn native::watch_hotkeys(table: Vec<(String, String)>, on_hit: impl Fn(String) + Send + 'static) -> Result<(), String>`: connects, sends `hotkeys/set`, `watch`, calls `on_hit(function)` per `hotkey` event; on a lost connection logs `err_log!("opusdm-wayland", "compositor IPC lost: {e}, retrying")` and reconnects every `RECONNECT_DELAY = Duration::from_secs(2)`, sending the table again.
  - Requests: `activate(id)`, `set_minimized(id, bool)`, `close(id)`, `kill(id)`, `move_to_desktop(id, desktop)`, `switch_desktop(index)`, `set_desktop_count(count) -> Result<u32, String>`, all `Result<(), String>` unless written.
- Tests: in `native.rs`, a fake server on a `UnixListener` in a temp dir: `NativeState::read` parses a `windows/list` + `desktops/list` reply; a reply with `"error"` is an `Err`; `watch_hotkeys` sends `hotkeys/set` first and calls `on_hit("terminal")` for a `hotkey` event; after the fake server closes and accepts again, the table is sent again. `wayfire.rs` tests unchanged and green.

- [ ] **Step 1: Write the failing tests**; **Step 2:** run them, fail; **Step 3:** implement (move framing first, run `cargo test -p opusdm-wayland` to see wayfire still green, then `native.rs`); **Step 4:** `cargo test -p opusdm-wayland && cargo clippy -p opusdm-wayland --all-targets -- -D warnings`.
- [ ] **Step 5: Commit** `wayland: native compositor IPC client`.

---

### Task 11: Panel native branch

**Files:**
- Modify: `crates/panel/src/wl_windows.rs`, `crates/panel/src/widgets/tasklist.rs` (line ~219)

**Interfaces:**
- Consumes: Task 10.
- Produces: in `wl_windows::spawn`, before the Wayfire check: `if native::under_native()` -> `spawn_native(tx)` (falls back to the foreign-toplevel path with an `err_log!` if `watch` fails). `native_events(prev: Option<&NativeState>, now: &NativeState) -> Vec<EwmhEvent>` mirrors `wayfire_events`: `Desktops { count, current, names: "1".."N" }`, `Clients` with `ClientWin { xid: id, desktop: desktop as i64, minimized, pid, app_id, title, icon: None, rect: WinRect from geometry }`, `ActiveWindow`. The action functions (`switch_desktop`, `activate_window`, `iconify_window`, `close_window`, `move_window_to_desktop`) take the native path first when `under_native()`. The tasklist "Move to desktop" submenu shows when `is_x11() || native::under_native()`.
- Tests: unit tests for `native_events` beside the existing `events` / `wayfire_events` tests (desktop index, pid and rect carried; a window on desktop 2 is `desktop: 2`, never `-1`; names "1".."4").

- [ ] **Step 1-4:** failing tests, implement, `cargo test -p opusdm-panel`, clippy.
- [ ] **Step 5: Check** (Global Constraints): `run-native.sh` (a Lister on desktop 1 shows only in the first pager box; tasklist minimize and restore; Move to desktop; Force kill of a scratch `foot`), then `run-x11.sh` and `run-wayland.sh` short runs: pager, tasklist and desktops as before. Record in `CHANGES.md`.
- [ ] **Step 6: Commit** `panel: native compositor windows and desktops`.

---

### Task 12: Hub native branch (hotkeys, desktop count)

**Files:**
- Modify: `crates/hub/src/global_keys_wl.rs`, `crates/hub/src/toolbar.rs` (`apply_virtual_screens`)

**Interfaces:**
- Consumes: Task 10 `watch_hotkeys`, `set_desktop_count`.
- Produces: `GlobalKeysWl::start`: `native::under_native()` is checked first and selects a `Native` backend that owns the `watch_hotkeys` thread (hits go to the same `on_hit` the portal uses, i.e. `HubCmd::RunFunctionHeadless`); `set(&[Hotkey])` sends the new global table to that thread (a channel; the thread re-sends `hotkeys/set`). `apply_virtual_screens(count)`: under native calls `native::set_desktop_count(count)` (error logged), the X11 path is unchanged, other Wayland sessions still return early.
- Tests: a unit test that the table sent to the compositor is the `global` entries only, as `(key, function)` pairs, in file order.

- [ ] **Step 1-4:** failing test, implement, `cargo test -p opusdm-hub`, clippy.
- [ ] **Step 5: Check** (Global Constraints) with the sandbox hotkey table of Task 13 if it is ready, otherwise with a scratch `hotkeys.json` in a copied config: a hub global hotkey runs its function in `run-native.sh`; the hub killed and restarted: hotkeys work again; X11 and labwc sandboxes: global hotkeys unchanged. Record in `CHANGES.md`.
- [ ] **Step 6: Commit** `hub: native compositor hotkeys and desktop count`.

---

### Task 13: Sandbox config, hotkey table and the stage 2 exit check

**Files:**
- Modify: `run-native.sh`, `docs/WAYLAND-PORT.md` (Native compositor section: stage 2), `CHANGES.md`

**Interfaces:**
- Produces in `run-native.sh`:
  - A private config: `XDG_CONFIG_HOME` = a temp dir where `opusdm/` is a **copy** of `~/.config/opusdm` (the sandbox never writes the live one) and every other entry of `~/.config` is a symlink. Only the hub's environment (autostart) gets it.
  - The sandbox hotkey table: in the copy, every `global` entry of `hotkeys.json` gets `Super+` inserted before the key name when it has no `Super` yet (`jq`; `jq` missing: exit with a message); entries that already use `Super` are kept and listed on stderr as "may be taken by the host". The table used is printed at start.
  - `OPUSDM_COMPOSITOR_CONFIG` = a temp `compositor.json` with `{"window_menu": "Super+Alt+space"}` (the host Openbox takes Alt+Space).
- [ ] **Step 1:** implement, `bash -n`, a run in Xephyr :7 checking that `~/.config/opusdm` is untouched (sha256 of every file before and after) and the hub reads the copied config.
- [ ] **Step 2: Stage 2 exit check** in `./run-native.sh --debug` (Xephyr :7, own D-Bus), each item PASS / FAIL / NOT CHECKED in `CHANGES.md`:
  - project CLAUDE.md rule 4: Lister (Enter in the path bar, double click on a folder, Backspace / parent, Alt+Left / Alt+Right, F-keys), global hotkeys on the sandbox combos (terminal, Run window, shot, desktops 1 / 2), hub (second Lister, SOURCE / DEST, a Copy job on scratch files, a function run);
  - pager: windows only in their desktop's box, click on a box switches desktop; tasklist: minimize, restore, Move to desktop, Force kill (scratch `foot`);
  - window menu (Super+Alt+Space in the sandbox): every item;
  - a notification click focuses its window (xdg-activation), if a notification with an action can be produced; otherwise NOT CHECKED;
  - the hub killed and restarted: hotkeys log "no hub connected" meanwhile, then work again;
  - the log has no error line that is not explained in `CHANGES.md`. The real combos (Ctrl+backslash, Ctrl+Shift+space, Ctrl+Print, Alt+1 / Alt+2) are NOT checked in the sandbox: they are checked on a spare VT in stage 4 (write that).
- [ ] **Step 3:** `cargo test --workspace` (without `DISPLAY`) and `cargo clippy -p opusdm-compositor -p opusdm-wayland -p opusdm-panel -p opusdm-hub --all-targets -- -D warnings`; pre-existing failures in other crates are reported, not fixed.
- [ ] **Step 4: Commit** `compositor: stage 2 sandbox and exit check`.
- [ ] **Step 5: Stop and report to the owner.** Stage 3 gets its own plan after the owner has used stage 2.
