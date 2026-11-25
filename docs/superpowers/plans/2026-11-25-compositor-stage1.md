# OpusDM compositor, stage 1 (nested skeleton) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `opusdm-compositor --nested` runs as a window of the current session and hosts the OpusDM desktop, panel, dock and Listers: xdg-shell with popups, layer-shell, keyboard and pointer, cascade placement, move / resize, focus, maximize and fullscreen.

**Architecture:** new crate `crates/compositor` on Smithay 0.7. One `State` owned by the calloop event loop for the whole run; two backends share it: `headless` (no rendering, frame callbacks on a timer, used by the integration tests) and `nested` (winit window, GLES rendering, real input). Pure logic (arguments, child environment, geometry) lives in modules with no Smithay types and is unit tested; protocol behaviour is tested by real Wayland clients against the headless backend.

**Tech Stack:** Rust (workspace, edition 2021, rust-version 1.92), Smithay 0.7 (`backend_winit`, `desktop`, `wayland_frontend`), calloop (through `smithay::reexports`), `signal-hook` (workspace), `tracing-subscriber`, `opusdm-ipc` (log macros). Tests: `smithay-client-toolkit` 0.20, `wayland-client` 0.31, `tempfile`.

**Spec:** `docs/superpowers/specs/2026-11-25-opusdm-compositor-design.md` (stage 1 of "Stages"). Stages 2-5 get their own plans after stage 1 has been exercised in the sandbox (project CLAUDE.md rule 6).

## Global Constraints

- Branch `opus-wayland`. Never push. Never `--no-verify`. Commit only the files the task touched.
- Smithay dependency exactly: `smithay = { version = "0.7", default-features = false, features = ["backend_winit", "desktop", "wayland_frontend"] }`. Default features stay off: they pull libinput, libseat and udev, which stage 1 does not use (`libinput-dev`, `libseat-dev`, `xwayland` are not installed on the dev machine). If the build asks for a feature, add only the one it names and say so in the commit.
- Smithay's API is 0.7.0: when a name in this plan does not match it, follow the 0.7.0 `smallvil` / `anvil` sources (tag `v0.7.0`), keep the behaviour this plan asks for, and note the difference in `CHANGES.md`.
- No crate other than `crates/compositor` and the root `Cargo.toml` (`members`) changes in stage 1. X11, labwc and Wayfire sessions are untouched.
- No `unwrap`, `expect`, `panic!` or indexing that can panic in handlers, in the event loop or in backends. A refused request, a failed render, a dead client: `opusdm_ipc::err_log!("opusdm-compositor", ...)` and go on. Debug traces: `opusdm_ipc::debug_log!("opusdm-compositor", ...)` (on with `OPUSDM_DEBUG=1`).
- Smithay's own `tracing` output goes to stderr through `tracing-subscriber`: level `warn`, `debug` when `OPUSDM_DEBUG=1`.
- Code and comments in English; never the em dash character; a doc comment on every function; constants in `UPPER_CASE`; exit conditions at the top of functions as one-liners; text files end with a newline.
- Every task adds a `CHANGES.md` entry under the current date: what changed, what was verified, and **what was NOT verified**.
- Commits that create or change ownership (State, clients, child processes, timers) answer in the body: *who owns this object for the whole run, and what happens when it is gone?* (project CLAUDE.md rule 2).

## Review Focus

- A client killed or crashing mid-session (a Lister `kill -9`): its windows disappear, the compositor keeps running and the next client maps normally. Test in Task 3.
- `xdg_toplevel.set_minimized` with no tasklist yet (stage 2): the window must not vanish for good; it is ignored with a debug trace and keeps being shown. Test in Task 3.
- A menu opened near the bottom-right corner (panel menus, context menus): the popup is moved inside the output, not cut off. Test in Task 4.
- The Run window (layer OVERLAY, exclusive keyboard): it gets the keyboard as soon as it maps. Test in Task 5.
- Many windows opened in a row: cascade wraps back to the top-left of the usable area instead of placing windows off screen; a window bigger than the area goes to its origin. Test in Task 2.

---

## File map

```
crates/compositor/
  Cargo.toml
  src/main.rs            argument handling, logging, backend dispatch, exit code
  src/args.rs            Args, BackendKind, parse_args (pure)
  src/child.rs           child_env (pure), spawn_startup, reap
  src/geometry.rs        Rect, Edges, cascade_position, resize_rect, usable_area helpers (pure)
  src/state.rs           State, globals, ClientState, the delegate registrations, signal flag
  src/shell/mod.rs       shared shell helpers (place_new_window, WindowInfo)
  src/shell/xdg.rs       XdgShellHandler: toplevels, popups, maximize, fullscreen, move/resize requests
  src/shell/layer.rs     WlrLayerShellHandler, layer map, exclusive zones, layer popups
  src/input.rs           pointer and keyboard handling shared by backends, focus, Alt+drag grabs
  src/grabs.rs           MoveGrab, ResizeGrab (pointer grabs)
  src/backend/mod.rs     run(args) dispatch
  src/backend/headless.rs
  src/backend/nested.rs
  tests/common/mod.rs    Compositor process harness, test client helpers
  tests/headless.rs      integration tests
run-native.sh            sandbox script (sibling of run-wayland.sh)
docs/WAYLAND-PORT.md     short "Native compositor" section
CHANGES.md
```

---

### Task 1: Crate, arguments and child environment

**Files:**
- Create: `crates/compositor/Cargo.toml`, `crates/compositor/src/main.rs`, `crates/compositor/src/args.rs`, `crates/compositor/src/child.rs`
- Modify: `Cargo.toml` (append `"crates/compositor"` to `members`)

**Interfaces:**
- Produces:
  - `pub enum BackendKind { Nested, Headless }`
  - `pub struct Args { pub backend: BackendKind, pub size: (i32, i32), pub startup: Option<String> }`
  - `pub fn parse_args(args: &[String]) -> Result<Args, String>` (args without the program name)
  - `pub const DEFAULT_SIZE: (i32, i32) = (1920, 1080);`
  - `pub fn child_env(socket_name: &str) -> Vec<(&'static str, Option<String>)>` (`Some` = set, `None` = remove)
  - `pub fn spawn_startup(command: &str, socket_name: &str) -> std::io::Result<std::process::Child>` (runs `sh -c command` with `child_env` applied)
  - `pub fn reap(child: &mut Option<std::process::Child>)` (non-blocking `try_wait`; on exit logs `startup command exited: <status>` with `err_log!` and sets `None`)

Package: `name = "opusdm-compositor"`, `[[bin]] name = "opusdm-compositor"`, workspace `version`, `edition`, `rust-version`, `license`. Dependencies now: `opusdm-ipc` (path), `signal-hook.workspace`, `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`, `smithay` as in Global Constraints. Dev-dependencies (used from Task 3): `tempfile.workspace`, `smithay-client-toolkit = "0.20"`, `wayland-client = "0.31"`, `wayland-protocols = { version = "0.32", features = ["client"] }`, `wayland-protocols-wlr = { version = "0.3", features = ["client"] }`.

- [ ] **Step 1: Write the failing tests** (in `args.rs` and `child.rs`, `#[cfg(test)]`)

```rust
#[test]
fn nested_with_size_and_startup() {
    let a = parse_args(&s(&["--nested", "--size", "1280x800", "--startup", "sh /tmp/x"])).unwrap();
    assert_eq!(a.backend, BackendKind::Nested);
    assert_eq!(a.size, (1280, 800));
    assert_eq!(a.startup.as_deref(), Some("sh /tmp/x"));
}

#[test]
fn headless_takes_the_default_size() {
    let a = parse_args(&s(&["--headless"])).unwrap();
    assert_eq!((a.backend, a.size, a.startup), (BackendKind::Headless, DEFAULT_SIZE, None));
}

#[test]
fn a_backend_is_required_and_bad_values_fail() {
    assert!(parse_args(&s(&[])).unwrap_err().contains("--nested or --headless"));
    assert!(parse_args(&s(&["--nested", "--size", "big"])).is_err());
    assert!(parse_args(&s(&["--nested", "--size", "0x600"])).is_err());
    assert!(parse_args(&s(&["--nested", "--startup"])).is_err());
    assert!(parse_args(&s(&["--nested", "--headless"])).is_err());
    assert!(parse_args(&s(&["--nested", "--bogus"])).is_err());
}

#[test]
fn children_see_only_this_compositor() {
    let env = child_env("wayland-3");
    assert!(env.contains(&("WAYLAND_DISPLAY", Some("wayland-3".into()))));
    assert!(env.contains(&("XDG_SESSION_TYPE", Some("wayland".into()))));
    for gone in ["DISPLAY", "GDK_BACKEND", "LABWC_PID", "WAYFIRE_SOCKET"] {
        assert!(env.contains(&(gone, None)), "{gone} must be removed");
    }
}
```

(`s` is a local helper turning `&[&str]` into `Vec<String>`.) `DISPLAY` is removed because stage 1 has no XWayland: an X11 app must fail, not open on the host desktop; `LABWC_PID` / `WAYFIRE_SOCKET` are removed so the hub does not take the labwc or Wayfire paths.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p opusdm-compositor`
Expected: compile errors for the missing items.

- [ ] **Step 3: Implement `parse_args`, `child_env`, `spawn_startup`, `reap`, and `main`**

`main() -> std::process::ExitCode`: parse `std::env::args().skip(1)`; on error print `opusdm-compositor: <error>` plus a one-line usage (`opusdm-compositor --nested|--headless [--size WxH] [--startup COMMAND]`) and return exit code 2; init `tracing-subscriber` (see Global Constraints); call `backend::run(&args)` (until Task 3 a stub returning `Err("no backend yet")`); on `Err(e)` log it with `err_log!` and return 1.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-compositor && cargo clippy -p opusdm-compositor -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit** (`Cargo.toml`, `Cargo.lock`, `crates/compositor/`, `CHANGES.md`), message `compositor: new crate, arguments and child environment`.

---

### Task 2: Placement and resize geometry

**Files:**
- Create: `crates/compositor/src/geometry.rs`

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub struct Edges { pub top: bool, pub bottom: bool, pub left: bool, pub right: bool }`
  - `pub const CASCADE_STEP: i32 = 30;`
  - `pub const MIN_WINDOW_SIZE: (i32, i32) = (64, 32);`
  - `pub fn cascade_position(area: Rect, size: (i32, i32), index: usize) -> (i32, i32)`: positions are `area.origin + k * CASCADE_STEP` on both axes, with `k = index % fit` and `fit = min((area.w - w) / CASCADE_STEP, (area.h - h) / CASCADE_STEP) + 1` (the positions that keep the window inside `area`); a window wider or taller than `area` goes to `area.origin`.
  - `pub fn resize_rect(start: Rect, edges: Edges, dx: i32, dy: i32, min: (i32, i32)) -> Rect`: moving the left / top edge moves `x` / `y` and keeps the opposite edge fixed; the size never goes under `min` (the dragged edge stops, the opposite one does not move).

- [ ] **Step 1: Write the failing tests**

```rust
const AREA: Rect = Rect { x: 0, y: 30, w: 1920, h: 1050 };

#[test]
fn windows_cascade_from_the_area_origin() {
    assert_eq!(cascade_position(AREA, (800, 600), 0), (0, 30));
    assert_eq!(cascade_position(AREA, (800, 600), 2), (60, 90));
}

#[test]
fn the_cascade_wraps_before_leaving_the_area() {
    // 800x600 in 1920x1050: fit = min(1120 / 30, 450 / 30) + 1 = 16 positions.
    assert_eq!(cascade_position(AREA, (800, 600), 15), (450, 480)); // last one, bottom edge at 1080
    assert_eq!(cascade_position(AREA, (800, 600), 16), (0, 30));
    assert_eq!(cascade_position(AREA, (800, 600), 17), (30, 60));
}

#[test]
fn a_window_bigger_than_the_area_goes_to_its_origin() {
    assert_eq!(cascade_position(AREA, (2500, 600), 5), (0, 30));
}

#[test]
fn dragging_the_left_edge_keeps_the_right_edge() {
    let r = resize_rect(Rect { x: 100, y: 100, w: 400, h: 300 }, Edges { left: true, ..Edges::default() }, -50, 0, MIN_WINDOW_SIZE);
    assert_eq!(r, Rect { x: 50, y: 100, w: 450, h: 300 });
}

#[test]
fn a_resize_stops_at_the_minimum_size() {
    let r = resize_rect(Rect { x: 100, y: 100, w: 400, h: 300 }, Edges { left: true, top: true, ..Edges::default() }, 1000, 1000, MIN_WINDOW_SIZE);
    assert_eq!(r, Rect { x: 436, y: 368, w: 64, h: 32 });
}
```

- [ ] **Step 2: Run them to see them fail** - `cargo test -p opusdm-compositor geometry`, compile errors.
- [ ] **Step 3: Implement the two functions** in `geometry.rs` (`mod geometry;` in `main.rs`).
- [ ] **Step 4: Run them** - `cargo test -p opusdm-compositor geometry`, PASS; clippy clean.
- [ ] **Step 5: Commit** (`crates/compositor/src/geometry.rs`, `src/main.rs`, `CHANGES.md`), message `compositor: cascade placement and resize geometry`.

---

### Task 3: State, headless backend, toplevels, test harness

**Files:**
- Create: `src/state.rs`, `src/shell/mod.rs`, `src/shell/xdg.rs`, `src/backend/mod.rs`, `src/backend/headless.rs`, `tests/common/mod.rs`, `tests/headless.rs`
- Modify: `src/main.rs`, `src/child.rs` (wire `reap` into the loop)

**Interfaces:**
- Consumes: `Args`, `BackendKind`, `spawn_startup`, `reap` (Task 1); `Rect`, `cascade_position` (Task 2).
- Produces:
  - `pub struct State` with at least: `display_handle`, `space: Space<Window>`, `windows: HashMap<WlSurface, WindowInfo>` (keyed by the toplevel's `wl_surface`), the Smithay states (`CompositorState`, `XdgShellState`, `ShmState`, `SeatState<State>`, `DataDeviceState`, `OutputManagerState` with xdg-output), `seat: Seat<State>`, `output: Output`, `startup: Option<Child>`, `cascade_index: usize`, `quit: Arc<AtomicBool>`, `start_time: Instant`.
  - `pub struct WindowInfo { pub restore: Option<Rect> }` (geometry before maximize / fullscreen).
  - `pub struct ClientState { pub compositor_state: CompositorClientState }` with `ClientData` whose `disconnected` logs `debug_log!` with the reason.
  - `pub fn State::new(display: &DisplayHandle, output: Output) -> State`
  - `pub fn State::usable_area(&self) -> Rect` (output geometry minus layer exclusive zones; the whole output until Task 5)
  - `pub fn State::send_frames(&self)` (frame callbacks to every mapped window, popup and layer surface)
  - `pub fn backend::run(args: &Args) -> Result<(), String>` dispatching to `headless::run` / `nested::run` (`nested::run` returns `Err("nested backend: not yet")` until Task 7).
  - Harness (`tests/common/mod.rs`): `pub struct Compositor { pub socket: PathBuf, child, _runtime: TempDir }` with `pub fn start() -> Compositor` (spawns `env!("CARGO_BIN_EXE_opusdm-compositor") --headless --size 1920x1080` with `XDG_RUNTIME_DIR` set to a fresh temp dir, reads the stdout line `WAYLAND_DISPLAY=<name>`, waits at most 5 s), `pub fn is_running(&mut self) -> bool`, `Drop` kills and waits. `pub fn connect(&Compositor) -> wayland_client::Connection` through `UnixStream::connect(socket)` + `Connection::from_socket` (never through process environment: tests run in parallel). Client helpers on smithay-client-toolkit: `TestClient::new(&Compositor)`, `map_toplevel(&mut self, size: (i32, i32)) -> TestWindow` (shm buffer, waits for the first configure, commits; a configure with size 0x0 keeps the requested size; every later configure is acked and answered with a buffer of the configured size), `roundtrip(&mut self)`, `wait_frame(&mut self, surface: &impl HasSurface, timeout: Duration) -> bool`, `last_configure(&self, window: &TestWindow) -> (i32, i32)`. `pub trait HasSurface { fn wl_surface(&self) -> &WlSurface; }` is implemented by `TestWindow`, `TestPopup` and `TestLayer`; `TestWindow::xdg(&self) -> &XdgToplevel`. The binary's modules are not visible to integration tests, so `tests/common` has its own `Rect { x, y, w, h: i32 }`; `Layer`, `Anchor` and `KeyboardInteractivity` come from `smithay_client_toolkit::shell::wlr_layer`.

Headless backend: an `Output` named `HEADLESS-1`, mode `args.size` at 60 Hz, scale 1; `ListeningSocketSource::new_auto()` in calloop; prints `WAYLAND_DISPLAY=<name>` on stdout (then flushes) once the socket is bound; a 16 ms calloop `Timer` that calls `send_frames`, `reap(&mut state.startup)`, checks `quit`, and flushes clients. `signal-hook::flag::register` for `SIGTERM` and `SIGINT` sets `quit`; when set, the loop logs `debug_log!("quit on signal")` and returns `Ok(())`. The startup command (if any) is spawned after the socket is bound. Keyboard (`seat.add_keyboard(XkbConfig::default(), KEY_REPEAT_DELAY, KEY_REPEAT_RATE)` with `KEY_REPEAT_DELAY: i32 = 600`, `KEY_REPEAT_RATE: i32 = 25`) and pointer are added in both backends.

Toplevels: `new_toplevel` wraps the surface in a `Window`, sends the initial configure with no size (client decides) and maps it at `cascade_position(usable_area, size, cascade_index)` once its first buffer commits (size known), then raises and focuses it (keyboard focus, `Activated` state). `toplevel_destroyed` and client disconnect remove the window from the `Space` and from `windows`, and give focus to the topmost remaining window. `minimize_request`: ignored with `debug_log!("minimize ignored: no tasklist before stage 2")`. Commit handler: `on_commit_buffer_handler`, the initial configure for toplevels not yet configured, `space.refresh()` as in smallvil.

- [ ] **Step 1: Write the failing integration tests** (`tests/headless.rs`)

```rust
#[test]
fn a_toplevel_gets_configured_and_frame_callbacks() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_frame(&w, Duration::from_secs(2)));
}

#[test]
fn a_client_that_disconnects_abruptly_does_not_take_the_compositor_down() {
    let mut comp = Compositor::start();
    let mut a = TestClient::new(&comp);
    let _w = a.map_toplevel((400, 300));
    drop(a); // socket closed with the objects still alive, like kill -9
    let mut b = TestClient::new(&comp);
    let w = b.map_toplevel((400, 300));
    assert!(b.wait_frame(&w, Duration::from_secs(2)));
    assert!(comp.is_running());
}

#[test]
fn minimize_is_ignored_and_the_window_keeps_getting_frames() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_minimized();
    c.roundtrip();
    assert!(c.wait_frame(&w, Duration::from_secs(2)));
}

#[test]
fn sigterm_ends_the_compositor_cleanly() {
    let mut comp = Compositor::start();
    comp.signal(libc::SIGTERM); // harness helper, `libc` dev-dependency (workspace)
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
}
```

(`signal` and `wait_exit(timeout) -> Option<i32>` are harness helpers to add to `Compositor`; add `libc.workspace = true` to dev-dependencies.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p opusdm-compositor --test headless`
Expected: FAIL (the binary exits with "no backend yet").

- [ ] **Step 3: Implement `State`, the xdg toplevel handling, the headless backend and the harness** as described above.

- [ ] **Step 4: Run all the crate tests**

Run: `cargo test -p opusdm-compositor && cargo clippy -p opusdm-compositor --all-targets -- -D warnings`
Expected: PASS (run the integration tests 5 times in a row: `for i in 1 2 3 4 5; do cargo test -q -p opusdm-compositor --test headless || break; done`, all pass: no flaky timing).

- [ ] **Step 5: Commit** with the rule 2 answer in the body (`State` owned by the event loop in `headless::run` for the whole run; windows owned by `space` + `windows`, removed on destroy or disconnect; the startup child owned by `State.startup`, reaped every tick, its exit logged; when the loop ends the socket and all clients go). Message `compositor: state, headless backend and xdg toplevels`.

---

### Task 4: Popups

**Files:**
- Modify: `src/shell/xdg.rs`, `src/state.rs` (`popups: PopupManager`), `tests/headless.rs`, `tests/common/mod.rs`

**Interfaces:**
- Consumes: `State` (Task 3).
- Produces: `State::popups: PopupManager`; helper `fn unconstrain_popup(&self, popup: &PopupSurface)` that keeps the popup inside the output (Smithay `PositionerState::get_unconstrained_geometry` with the target rectangle = output geometry relative to the popup's root surface position). Popup grabs (`XdgShellHandler::grab`) with `PopupKeyboardGrab` / `PopupPointerGrab` as in anvil, so a click outside closes GTK menus. Harness: `TestClient::map_popup(&mut self, parent: &impl PopupParent, anchor: Rect, size: (i32, i32)) -> TestPopup` (`PopupParent` implemented by `TestWindow` now and by `TestLayer` in Task 5), `popup_configure(&self, popup: &TestPopup) -> Rect` (position relative to the parent), with constraint adjustment `slide_x | slide_y | flip_x | flip_y`.

A toplevel's position is not visible to the client, so the "stays inside the output" test needs a parent at a known position and lives in Task 5 (layer surface in a corner). This task tests that a popup is configured and shown.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_popup_gets_configured_and_shown() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let p = c.map_popup(&w, Rect { x: 10, y: 10, w: 1, h: 1 }, (200, 150));
    assert_eq!(c.popup_configure(&p).w, 200);
    assert!(c.wait_frame(&p, Duration::from_secs(2)));
}
```

- [ ] **Step 2: Run it to see it fail** - `cargo test -p opusdm-compositor --test headless popup`, FAIL.
- [ ] **Step 3: Implement popups** (new_popup tracks and unconstrains; reposition_request reconfigures; popups included in `send_frames`).
- [ ] **Step 4: Run all the crate tests** - PASS, clippy clean.
- [ ] **Step 5: Commit**, message `compositor: xdg popups with grabs and output constraint`.

---

### Task 5: Layer shell, exclusive zones, maximize and fullscreen

**Files:**
- Create: `src/shell/layer.rs`
- Modify: `src/state.rs` (`WlrLayerShellState`), `src/shell/xdg.rs`, `src/shell/mod.rs`, `tests/headless.rs`, `tests/common/mod.rs`

**Interfaces:**
- Consumes: `State`, `PopupManager`, `unconstrain_popup` (Tasks 3-4); `Rect` (Task 2).
- Produces:
  - `State::usable_area()` now returns the output's `layer_map_for_output(..).non_exclusive_zone()` as a `Rect`.
  - Layer surfaces mapped with `layer_map_for_output(&output).map_layer(..)`, arranged on every commit; their initial configure sent on first commit.
  - Keyboard: a layer surface with `KeyboardInteractivity::Exclusive` on the TOP or OVERLAY layer takes keyboard focus when it maps and keeps it while mapped; `OnDemand` gets focus on click (Task 6); when it unmaps, focus goes back to the topmost window.
  - Layer popups (`new_popup` whose parent is a layer surface) unconstrained like xdg popups.
  - `maximize_request` / `unmaximize_request`: maximize stores `WindowInfo::restore`, configures `usable_area()` size and moves the window to its origin; unmaximize restores. `fullscreen_request` / `unfullscreen_request`: whole output geometry, window raised above the TOP layer while fullscreen (render order in Task 7), restore the same way.
  - When the usable area changes (a panel appears or goes), maximized windows are reconfigured to the new area.
  - Harness: `TestClient::map_layer(&mut self, layer: Layer, anchor: Anchor, size: (u32, u32), exclusive_zone: i32, keyboard: KeyboardInteractivity) -> TestLayer` (on the default output, waits for the first configure and commits a buffer of the configured size), `layer_configure(&self, l: &TestLayer) -> (u32, u32)`, `keyboard_focus(&self) -> Option<WlSurface>` (from this client's `wl_keyboard` enter / leave), `PopupParent` for `TestLayer` (`zwlr_layer_surface_v1.get_popup`).

- [ ] **Step 1: Write the failing tests** (the last one is the popup constraint test announced in Task 4)

```rust
#[test]
fn a_top_panel_spans_the_output_width() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let p = c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    assert_eq!(c.layer_configure(&p), (1920, 30));
}

#[test]
fn a_maximized_window_leaves_room_for_an_exclusive_zone() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_maximized();
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (1920, 1050));
    w.xdg().unset_maximized();
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (400, 300));
}

#[test]
fn a_fullscreen_window_covers_the_whole_output() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    c.map_layer(Layer::Top, Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, 30), 30, KeyboardInteractivity::None);
    let w = c.map_toplevel((400, 300));
    w.xdg().set_fullscreen(None);
    c.roundtrip();
    assert_eq!(c.last_configure(&w), (1920, 1080));
}

#[test]
fn an_exclusive_keyboard_layer_gets_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let run = c.map_layer(Layer::Overlay, Anchor::empty(), (600, 80), 0, KeyboardInteractivity::Exclusive);
    c.roundtrip();
    assert_eq!(c.keyboard_focus(), Some(run.wl_surface().clone()));
}

#[test]
fn a_popup_at_the_bottom_right_corner_stays_inside_the_output() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    // 100x100 surface in the bottom-right corner: its origin is (1820, 980).
    let corner = c.map_layer(Layer::Top, Anchor::BOTTOM | Anchor::RIGHT, (100, 100), 0, KeyboardInteractivity::None);
    let p = c.map_popup(&corner, Rect { x: 99, y: 99, w: 1, h: 1 }, (300, 400));
    let g = c.popup_configure(&p);
    assert!(1820 + g.x >= 0 && 1820 + g.x + 300 <= 1920, "x out of the output: {g:?}");
    assert!(980 + g.y >= 0 && 980 + g.y + 400 <= 1080, "y out of the output: {g:?}");
}
```

- [ ] **Step 2: Run them to see them fail** - `cargo test -p opusdm-compositor --test headless`, the new tests FAIL.
- [ ] **Step 3: Implement layer shell, maximize, fullscreen** as described.
- [ ] **Step 4: Run all the crate tests** - PASS (5 runs in a row, as in Task 3); clippy clean.
- [ ] **Step 5: Commit**, message `compositor: layer shell, exclusive zones, maximize and fullscreen`.

---

### Task 6: Pointer, keyboard, focus and grabs

**Files:**
- Create: `src/input.rs`, `src/grabs.rs`
- Modify: `src/shell/xdg.rs` (`move_request`, `resize_request`), `src/state.rs`

**Interfaces:**
- Consumes: `State`, `usable_area` (Tasks 3, 5); `Rect`, `Edges`, `resize_rect`, `MIN_WINDOW_SIZE` (Task 2).
- Produces (backend-independent, called by the nested backend in Task 7):
  - `pub fn State::pointer_motion(&mut self, position: Point<f64, Logical>, time_msec: u32)`: pointer focus = the surface under the position, searched in render order: OVERLAY and TOP layers, then windows (topmost first; a fullscreen window above TOP), then BOTTOM and BACKGROUND layers.
  - `pub fn State::pointer_button(&mut self, button: u32, pressed: bool, time_msec: u32)`: on press, focus and raise the window under the pointer (or give the keyboard to an `OnDemand` layer surface under it; the BACKGROUND desktop counts); with Alt held, left button starts a `MoveGrab`, right button a `ResizeGrab` on the nearest edges (the quadrant of the window the pointer is in), and the click is not sent to the client.
  - `pub fn State::pointer_axis(&mut self, frame: AxisFrame)` and `pub fn State::keyboard_key(&mut self, keycode: Keycode, pressed: bool, time_msec: u32)` (keys go to the focused surface; no compositor bindings in stage 1).
  - `MoveGrab { start_data, window, initial_loc }` and `ResizeGrab { start_data, window, edges: Edges, initial: Rect }` implementing Smithay `PointerGrab<State>`; the resize computes the new size with `resize_rect` and sends it in a configure with the `Resizing` state; on button release the grab ends. Client-initiated `move_request` / `resize_request` (GTK title bar drag, border drag) start the same grabs when the serial matches the pointer's grab serial.
  - A maximized or fullscreen window that is moved by a grab is first restored to its `WindowInfo::restore` size.

No integration test can inject pointer input into the headless backend (no virtual pointer in stage 1); the behaviour is checked by hand in Task 8. The pure parts are already covered by `resize_rect` tests; add one unit test for the edge choice:

- [ ] **Step 1: Write the failing test** (in `grabs.rs`)

```rust
#[test]
fn the_resize_edges_follow_the_quadrant_of_the_pointer() {
    let r = Rect { x: 0, y: 0, w: 400, h: 300 };
    assert_eq!(edges_for_point(r, (10, 10)), Edges { top: true, left: true, ..Edges::default() });
    assert_eq!(edges_for_point(r, (390, 290)), Edges { bottom: true, right: true, ..Edges::default() });
}
```

(`pub fn edges_for_point(window: Rect, point: (i32, i32)) -> Edges`, point relative to the window origin.)

- [ ] **Step 2: Run it to see it fail** - compile error.
- [ ] **Step 3: Implement input, focus and grabs** as described.
- [ ] **Step 4: Run all the crate tests** - PASS, clippy clean.
- [ ] **Step 5: Commit**, message `compositor: pointer, keyboard, focus and move/resize grabs`.

---

### Task 7: Nested backend (winit, rendering)

**Files:**
- Create: `src/backend/nested.rs`
- Modify: `src/backend/mod.rs`, `src/state.rs` (cursor status, damage tracker)

**Interfaces:**
- Consumes: everything above; `Args.size`.
- Produces: `pub fn nested::run(args: &Args) -> Result<(), String>`.

Behaviour:
- `winit::init::<GlesRenderer>()`, window title `OpusDM compositor (nested)`, inner size `args.size`; one `Output` named `NESTED-1` whose mode follows the window size (a host resize changes the mode, re-arranges the layer map and reconfigures maximized and fullscreen windows).
- Same socket, startup command, signals, reaping and logging as the headless backend; factor the shared setup into `backend/mod.rs` (`fn setup_common(..)`) instead of copying it.
- `linux-dmabuf` global from the renderer's formats, so GTK4 can use its GL renderer.
- Render order, back to front: BACKGROUND, BOTTOM, windows (bottom to top), TOP, fullscreen window (if any), OVERLAY, popups with their parents, cursor. Damage tracked with `OutputDamageTracker`.
- Cursor: a client cursor surface is drawn; `CursorImageStatus::Named(icon)` sets the winit window cursor to that icon; `Hidden` hides it.
- winit events map to the Task 6 functions (`pointer_motion` with the position scaled to the output, `pointer_button`, `pointer_axis`, `keyboard_key`); `WinitEvent::CloseRequested` ends the loop like `SIGTERM`.
- A failed frame (`bind`, `render_output`, `submit`) is logged with `err_log!` once per kind of error per second at most, and the loop goes on.

- [ ] **Step 1: Build and run it by hand**

Run: `cargo build -p opusdm-compositor && target/debug/opusdm-compositor --nested --size 1280x800 --startup "gtk4-demo || weston-terminal || foot"`
Expected: a host window opens; the startup app shows inside it, can be typed in, moved by its title bar and with Alt+drag, resized with Alt+right-drag, maximized and restored; its menus open where expected; closing the host window ends the compositor with exit code 0 and no error lines.

- [ ] **Step 2: Run all the crate tests** - `cargo test -p opusdm-compositor` PASS, clippy clean.
- [ ] **Step 3: Commit**, rule 2 answer in the body (renderer, damage tracker and winit backend owned by the loop in `nested::run`; on a host window close or a signal the loop ends and clients are disconnected). Message `compositor: nested backend with rendering and input`. `CHANGES.md` lists which apps were tried.

---

### Task 8: Sandbox script and the stage 1 exit check

**Files:**
- Create: `run-native.sh`
- Modify: `docs/WAYLAND-PORT.md` (new short section "Native compositor (opusdm-compositor)": what stage 1 covers, how to run `run-native.sh`, what is missing until stage 2: tasklist, pager, global hotkeys, minimize), `CHANGES.md`

**Interfaces:**
- Consumes: `opusdm-compositor --nested --size WxH --startup CMD`.

`run-native.sh [--debug] [resolution]`, modelled on `run-wayland.sh`: exit early with a message when `cargo` is missing; build the workspace (`cargo build --release --workspace`); make a temp config dir with an `autostart` script that makes `WAYLAND_DISPLAY` an absolute path (`$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`), switches `XDG_RUNTIME_DIR` to a fresh temp dir for the hub, unsets `GDK_BACKEND`, prepends `target/release` to `PATH`, exports `OPUSDM_DEBUG=1` with `--debug` (log to `/ramdisk/opusdm.log` as `run-wayland.sh` does) and starts `opusdm-hub`; then `exec target/release/opusdm-compositor --nested --size "$RESOLUTION" --startup "sh $CONFIG_DIR/autostart"`. `trap` removes the temp dirs.

- [ ] **Step 1: Write `run-native.sh`** (executable, `set -euo pipefail`, header comment like `run-wayland.sh`).
- [ ] **Step 2: Run the stage 1 exit check by hand** in `./run-native.sh --debug`, and record each result in `CHANGES.md`:
  - desktop (BACKGROUND layer) with its icons, panel (TOP, exclusive zone respected by a maximized window), dock (BOTTOM) visible;
  - right click on the desktop and on a panel item: menus open, inside the output, a click outside closes them;
  - double click on a desktop folder opens a Lister;
  - Lister: Enter in the path bar, double click on a folder, Backspace / parent button, Alt+Left / Alt+Right, F-keys;
  - hub: open a second Lister, SOURCE / DEST roles, a Copy job, a function run;
  - Run window: opens (its key comes from the host hotkeys, so start it from the panel or the desktop menu), gets the keyboard at once;
  - move by title bar, Alt+drag, Alt+right-drag resize, maximize / restore, fullscreen (a video in the player);
  - `kill -9` of a Lister: the compositor survives, the other windows keep working;
  - the log has no error line that is not explained in `CHANGES.md`.
  Global hotkeys are NOT part of stage 1 (stage 2); write that in `CHANGES.md` explicitly.
- [ ] **Step 3: Run the whole workspace checks**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS (the other crates are untouched; a failure there means the workspace change broke something and must be fixed before committing).

- [ ] **Step 4: Commit** (`run-native.sh`, `docs/WAYLAND-PORT.md`, `CHANGES.md`), message `compositor: run-native.sh sandbox and stage 1 check`.
- [ ] **Step 5: Stop and report to the owner**: what passed, what did not, what was not checked. Stage 2 gets its own plan only after the owner has seen stage 1 running.
