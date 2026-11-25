# OpusDM compositor, stage 3 (full window control) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** inside `run-native.sh`, `opusdm-compositor` offers every window manager behaviour of today's Openbox session (all `openbox-rc.xml` bindings, an Alt+Tab window switcher, edge snapping, active / inactive opacity, server side title bars for the clients that ask), plus `wlr-screencopy` for `opusdm-shot`, clipboard, primary selection and drag and drop between apps, and a strict xdg-activation policy.

**Architecture:** pure modules for every decision (bindings, switcher order, snap zones, decoration geometry, opacity rule, activation policy), wired into `State`; cairo / pango drawing for the switcher and the title bars like the stage 2 window menu; hand-written `zwlr_screencopy_manager_v1` v3 server with the capture done by the backend; Smithay's `data_device` / `primary_selection` / `xdg::decoration` helpers. Client changes (`crates/wayland`, `crates/hub`, `crates/lister`) are taken only under `opusdm_wayland::native::under_native()`; one shared change in `crates/ipc` (log macros) is an isolated commit verified on X11.

**Tech Stack:** Rust (edition 2021, rust-version 1.92), Smithay 0.7 (`default-features = false`, `backend_winit`, `desktop`, `wayland_frontend`), `wayland-protocols-wlr` 0.3 through `smithay::reexports`, cairo-rs / pangocairo 0.21, `opusdm-ipc`. Tests: the stage 1 / 2 headless harness (`crates/compositor/tests/common`), `wayland-client` 0.31, smithay-client-toolkit 0.20.

**Spec:** `docs/superpowers/specs/2026-11-25-opusdm-compositor-design.md` (stage 3, "Built-in window manager bindings", "Window management behaviour", "Configuration", "Title bar colours"). Stage 2 plan and ledger: `docs/superpowers/plans/2026-11-25-compositor-stage2.md`, `.superpowers/sdd/2026-11-25-compositor-stage2/progress.md`. Key code facts (2026-11-25): bindings in `openbox-rc.xml` / `labwc-rc.xml` (repo root); opacity rule `crates/hub/src/compositor.rs` + base `picom.conf`; `DesktopSettings` in `crates/ipc/src/desktop.rs`; theme `assets/openbox-share/themes/OpusDMTheme/openbox-3/themerc`; screencopy client `crates/wayland/src/screencopy.rs` (binds v1..=3, shm only, `capture_output`, `copy`); hotkey validation `crates/ipc/src/bindings.rs` `validate_hotkeys` (callers `crates/lister/src/hotkey_editor.rs`); Smithay 0.7.0 has `selection::{data_device, primary_selection}` and `shell::xdg::decoration`, no screencopy helper.

## Global Constraints

- Branch `opus-wayland`. Never push, never `--no-verify`, never a git worktree. Commit only the files the task touched; one risky change per commit (project CLAUDE.md rule 6).
- Every stage 1 / stage 2 Global Constraint holds: Smithay default features off (add only a feature the build names, and say so); no `unwrap` / `expect` / `panic!` / panicking indexing outside tests; `err_log!` / `debug_log!` with tag `"opusdm-compositor"`; English, no em dash character, doc comment on every function, `UPPER_CASE` constants, one-line early exits, files end with a newline; `CHANGES.md` entry per task with what was NOT verified; rule 2 ownership answer in commit bodies that create or change ownership.
- **Anti-goal: no regression of the X11, labwc or Wayfire sessions.** Client changes are taken only under `under_native()` (evaluated once per process, as in stage 2). The single exception is Task 1 (`crates/ipc` log macros), whose behaviour change is limited to "a closed stderr no longer panics".
- Tests never read the owner's config (`XDG_CONFIG_HOME` / `HOME` temp, `OPUSDM_COMPOSITOR_CONFIG` removed). Manual checks follow `.superpowers/sdd/2026-11-25-compositor-stage2/sandbox-protocol.md`: private Xvfb host, never `:0`, sandbox D-Bus only (no process ever gets the live `DBUS_SESSION_BUS_ADDRESS`), sha256 of `~/.config/opusdm` before / after, kill only own pids.
- Built-in binding defaults (spec, `openbox-rc.xml`): `switcher_next` `Alt+Tab`, `switcher_prev` `Alt+Shift+Tab`, `switcher_all` `Ctrl+Alt+Tab`, `close` `Alt+F4`, `desktop_left` `Ctrl+Alt+Left`, `desktop_right` `Ctrl+Alt+Right`, `desktop_1`..`desktop_4` `Super+F1`..`Super+F4`, `send_left` `Shift+Alt+Left`, `send_right` `Shift+Alt+Right`, `show_desktop` `Super+d`, `lower` `Alt+Escape`, `focus_left` / `focus_right` / `focus_up` / `focus_down` `Super+Shift+Left` / `Right` / `Up` / `Down`, `window_menu` `Alt+space`, `quit` `Ctrl+Alt+BackSpace`. An empty string disables a binding.
- Desktop wrap follows `DesktopSettings.wrap_desktops` (default true), not the `wrap=no` of `openbox-rc.xml`. Snap threshold default `10` px (`labwc-rc.xml` `<range>`), `snap_enabled` from `DesktopSettings`. Opacity: `transparency_enabled` (default true), `active_opacity` (default 100), `inactive_opacity` (default 88), range 20..=100.
- Title bar layout default `"close,iconify,max:"` (labwc syntax: buttons left of `:` on the left, right of `:` on the right), title centered (`window.label.text.justify center`).

## Review Focus

- The owner's X11 session after Task 1: every OpusDM process logs exactly as before; only a write to a closed stderr changes (no panic). Test in Task 1 (a child process with stderr closed logs and exits 0) plus the rule 4 checks in `run-x11.sh`.
- A window destroyed while the switcher is open on it, or an exclusive layer (Run) mapping while the switcher is open: the switcher drops the entry / closes, Alt release activates nothing dead, no stuck modifier. Tests in Task 3.
- A server-side-decorated window maximized, snapped or fullscreen: the client area excludes the title bar and borders, the title bar stays inside the usable area, fullscreen has no title bar, `windows/list` geometry is the frame. Tests in Task 6.
- A drag or clipboard source client that dies mid-transfer: the target gets cancel / no offer, the compositor never reads the pipe and never blocks. Test in Task 9.
- A screencopy client that sends a wrong buffer, or never calls `copy`, or dies with a frame pending: `failed` plus a log line, no leaked frame, the next capture works. Tests in Task 8.

---

## File map

```
crates/ipc/src/log.rs (or where err_log!/debug_log! live)   never panic on stderr write
crates/compositor/src/
  config.rs                 + built-in binding keys, snap_threshold, title_layout
  wm/settings.rs            WmSettings from DesktopSettings (pure)
  wm/actions.rs             lower, show desktop, relative desktop, send and follow, directional focus (pure parts + State fns)
  hotkeys.rs                Binding::BuiltIn(Action), table of all built-ins
  switcher/mod.rs           WindowSwitcher state, input
  switcher/model.rs         MRU order, filtering (pure)
  draw/mod.rs               shared cairo/pango helpers (moved from menu/render.rs)
  switcher/render.rs        switcher image
  snap.rs                   snap zones (pure) + preview rect
  opacity.rs                opacity rule (pure)
  decor/mod.rs              xdg-decoration handler, per-window decoration state
  decor/geometry.rs         insets, hit-test zones (pure)
  decor/theme.rs            window.* themerc keys (extends menu/theme.rs Rgb, parse)
  decor/render.rs           title bar image
  protocols/screencopy.rs   zwlr_screencopy_manager_v1 v3 server
  selection.rs              data device, primary selection, DnD handlers
  activation.rs             strict policy
  ipc/protocol.rs           window/set-geometry, window/set-opacity, reload, bindings/list, debug/key state, debug/pointer, debug/button
crates/compositor/tests/   headless.rs, ipc.rs, common/ (new helpers)
crates/wayland/src/native.rs   set_geometry, set_opacity, reload, builtin_bindings, NativeWindow.urgent/opacity
crates/hub/src/            desktop settings apply under native -> native::reload()
crates/ipc/src/bindings.rs     BindingError::HotkeyConflictsWithBuiltin, validate_hotkeys(.., builtins)
crates/lister/src/hotkey_editor.rs   builtins fetched under native
run-native.sh, docs/WAYLAND-PORT.md, CHANGES.md
```

---

### Task 1: Log macros never panic on a closed stderr

**Files:**
- Modify: the file in `crates/ipc` that defines `err_log!`, `debug_log!` (and `dlog!` if it writes to stderr), plus a test.

**Interfaces:**
- Produces: the macros write with `let _ = writeln!(std::io::stderr().lock(), ...)` (or an equivalent helper `pub fn write_stderr_line(line: &str)` in `opusdm_ipc`), output text byte-identical to today, a failed write ignored.

- [ ] **Step 1: Write the failing test** (in `crates/ipc`, `#[test] fn logging_to_a_closed_stderr_does_not_panic`): spawn the current test binary itself (`std::env::current_exe()`, an env marker selects a child mode) with `Stdio::null()` for stdout and a pipe for stderr whose read end the parent drops before the child logs; the child calls `err_log!` 100 times then exits 0; assert the child's exit status is success.
- [ ] **Step 2: Run it** (`cargo test -p opusdm-ipc logging_to_a_closed_stderr`): FAIL (the child panics, non-zero status).
- [ ] **Step 3: Implement.** Keep the exact output format (compare one line before / after in a unit test that captures through a pipe).
- [ ] **Step 4: Run** `cargo test -p opusdm-ipc`, `cargo clippy -p opusdm-ipc --all-targets -- -D warnings` (record pre-existing warnings, add none), then the project CLAUDE.md rule 4 checks in `run-x11.sh` (sandbox protocol) and a short `run-wayland.sh`: logs look the same.
- [ ] **Step 5: Commit** `ipc: log macros never panic on a closed stderr`, body: who sees the change (every OpusDM process), what changes (only the closed-stderr case), rule 2 not applicable.

---

### Task 2: Configuration, WM settings and `reload`

**Files:**
- Create: `src/wm/settings.rs`
- Modify: `src/config.rs`, `src/state.rs`, `src/backend/mod.rs`, `src/ipc/protocol.rs`, `src/hotkeys.rs` (table rebuilt on reload), `tests/ipc.rs`

**Interfaces:**
- Produces:
  - `CompositorConfig` gains one `String` field per built-in key of the Global Constraints (field names = the keys), `pub snap_threshold: u32` (default `10`, clamped `0..=100`), `pub title_layout: String` (default `"close,iconify,max:"`). `KNOWN_KEYS` lists them all; per-key fallback as today.
  - `pub struct WmSettings { pub wrap_desktops: bool, pub snap_enabled: bool, pub transparency_enabled: bool, pub active_opacity: u8, pub inactive_opacity: u8 }`, `pub fn WmSettings::from_desktop(ds: &DesktopSettings) -> WmSettings`; `State::wm: WmSettings`, `State::config: CompositorConfig`.
  - IPC `reload` (no data): re-reads `compositor.json` and `desktop.json` (count applied with `set_desktop_count`), rebuilds the hotkey built-ins keeping the hub table, re-applies opacity (Task 5); reply `{"ok":true,"errors":[...]}` with the config errors.
  - IPC `bindings/list` -> `{"ok":true,"bindings":[{"name":"switcher_next","key":"Alt+Tab"},...]}` (enabled built-ins only, Quit included when enabled).
- [ ] **Step 1: Failing tests:** unit: `parse("{}")` gives every default above; `{"snap_threshold": 500}` clamps to 100; `{"close": ""}` disables close. `WmSettings::from_desktop(&DesktopSettings::default())` equals `{true, true, true, 100, 88}`. Integration (`tests/ipc.rs`): `bindings/list` contains `{"name":"close","key":"Alt+F4"}`; write a `compositor.json` with `{"close": "Super+q"}` into the test's config dir, call `reload`, `bindings/list` now has `Super+q`.
- [ ] **Step 2-4:** run (fail), implement, run crate tests + clippy + headless/ipc loop 5 times.
- [ ] **Step 5: Commit** `compositor: built-in binding keys, WM settings and reload`.

---

### Task 3: Built-in window manager actions and bindings

**Files:**
- Create: `src/wm/actions.rs`
- Modify: `src/hotkeys.rs`, `src/wm/mod.rs`, `src/ipc/protocol.rs` (`debug/key` gains `"state": "press" | "release" | "tap"`, default `"tap"`), `tests/common/mod.rs`, `tests/headless.rs`

**Interfaces:**
- Consumes: Task 2 config and `WmSettings`; stage 2 `activate_window`, `switch_desktop`, `move_to_desktop`, `close_window`, `run_menu_action`.
- Produces:
  - `pub enum Action { SwitcherNext, SwitcherPrev, SwitcherAll, Close, DesktopLeft, DesktopRight, Desktop(u32), SendLeft, SendRight, ShowDesktop, Lower, Focus(Direction) }`, `pub enum Direction { Left, Right, Up, Down }`; `Binding` gains `BuiltIn(Action)`. Lookup order unchanged: Quit, WindowMenu, built-ins, Hub; conflicts logged as stage 2 (`... the built-in wins`).
  - Pure: `pub fn relative_desktop(current: u32, count: u32, delta: i32, wrap: bool) -> u32`; `pub fn directional_target(from: Rect, candidates: &[(WindowId, Rect)], dir: Direction) -> Option<WindowId>` (nearest window whose centre lies in the half-plane of `dir`, by centre distance; ties by stacking order, top first).
  - `State`: `pub fn lower_window(&mut self, id: WindowId) -> Result<(), String>` (bottom of `stack`, keyboard to the new topmost shown window: Openbox `Lower`, `FocusToBottom`, `Unfocus`); `pub fn toggle_show_desktop(&mut self)` (minimizes every shown window of the current desktop and remembers them; a second toggle restores exactly those still alive; any activation, desktop switch or newly mapped window leaves show-desktop mode without restoring); `pub fn send_and_follow(&mut self, id: WindowId, delta: i32) -> Result<(), String>` (spec: move to the previous / next desktop and follow it, wrap per `WmSettings`); `pub fn run_action(&mut self, action: Action)` acting on the active window where one is needed (none: `debug_log!`, nothing).
  - `Desktop(n)` with `n >= count`: `err_log!`, nothing. Switcher actions call `State::open_switcher` (Task 4; until then a `debug_log!` stub).
- [ ] **Step 1: Failing tests:** unit for `relative_desktop` (`(0,4,-1,true) == 3`, `(0,4,-1,false) == 0`, `(3,4,1,true) == 0`) and `directional_target` (left picks the nearest window to the left, none to the left is `None`). Headless: `Alt+F4` closes the active window (client receives `xdg_toplevel.close`); `Ctrl+Alt+Left` on desktop 0 goes to desktop 3 (wrap default); `Super+F2` goes to desktop 1; `Shift+Alt+Right` moves the active window to desktop 1 and desktop 1 becomes current with that window active; `Alt+Escape` with A over B puts A at the bottom and B gets the keyboard; `Super+d` twice: all windows minimized then restored; `Super+d` then mapping a new window: the earlier windows stay minimized.
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop.
- [ ] **Step 5: Commit** `compositor: built-in window manager bindings`.

---

### Task 4: Window switcher (Alt+Tab)

**Files:**
- Create: `src/switcher/mod.rs`, `src/switcher/model.rs`, `src/switcher/render.rs`, `src/draw/mod.rs`
- Modify: `src/menu/render.rs` (shared helpers moved to `draw/`), `src/menu/model.rs`, `src/input.rs`, `src/state.rs`, `src/backend/nested.rs`, `tests/headless.rs`

**Interfaces:**
- Consumes: Task 3 actions, stage 2 menu drawing (`ErrorThrottle`, cache-by-state pattern).
- Produces:
  - Focus history: `State::mru: Vec<WindowId>` (most recent first), updated wherever `active_window` changes, pruned on destroy.
  - Pure: `pub fn switcher_entries(mru: &[WindowId], windows: &[WindowView], all_desktops: bool, current: u32) -> Vec<WindowId>` (current desktop only unless `all_desktops`; minimized windows included, as Openbox `NextWindow`).
  - `pub struct WindowSwitcher { entries: Vec<WindowId>, selected: usize, hold_mods: Mods }`; `State::open_switcher(all: bool, backwards: bool)`: opens with the second entry selected (first when only one); while open, the binding key (with or without Shift) moves forward / backward, `Escape` cancels, any other key is swallowed with a `debug_log!`; releasing the last held modifier of the opening combo activates the selected entry (`activate_window`: switches desktop, restores, raises, focuses) and closes. Entries whose window is gone are dropped; an empty switcher closes. An exclusive layer mapping closes it with no action.
  - Drawing (nested only): a centred box `SWITCHER_WIDTH = 480`, rows of `SWITCHER_ROW_HEIGHT = 28` with the title (ellipsized) and the app id dimmed, selected row in the menu's active colours (`MenuTheme`), drawn above the window menu layer; cached by state like the menu.
  - Carried from stage 2, in the same refactor of `menu/`: the "Move to desktop" submenu opens on its first **enabled** entry; `MENU_SCALE` and the element placement use one constant; a render error is not retried every frame (cache the failure for the same state).
- [ ] **Step 1: Failing tests:** unit for `switcher_entries` (MRU order, other desktops filtered, `all_desktops` keeps them). Headless (with `debug/key` `"state"`): A, B, C mapped (C active): press `Alt`, tap `Tab`, release `Alt` -> B active; press `Alt`, tap `Tab` twice, release -> A; `Alt+Shift+Tab` goes the other way; `Escape` before release changes nothing; B destroyed while the switcher shows it selected -> release activates the next valid entry, no panic; `keys_received() == 0` for the client during all of it. Unit: `render_switcher` buffer size `SWITCHER_WIDTH x (rows * 28 + 2 * padding + 2 * border)`; menu submenu test: Right on "Move to desktop" for a window on desktop 0 selects `"2"`.
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop. **Step 5 (manual):** nested in Xvfb, three foot windows: open the switcher, screenshot, cycle, release; record PASS / FAIL in `CHANGES.md`.
- [ ] **Step 6: Commit** `compositor: Alt+Tab window switcher` (stage 2 menu fixes may be a separate commit `compositor: menu submenu selection and render retry`).

---

### Task 5: Edge snapping and opacity

**Files:**
- Create: `src/snap.rs`, `src/opacity.rs`
- Modify: `src/grabs.rs` (`MoveGrab`), `src/backend/nested.rs` (snap preview element, window alpha), `src/wm/mod.rs` (`WindowView.opacity`), `src/ipc/protocol.rs` (`window/set-opacity`, `window/set-geometry`), `src/ipc/protocol.rs` debug: `debug/pointer {"x","y"}` (absolute motion) and `debug/button {"button":"left"|"right"|"middle","state":"press"|"release"}`, `tests/common/mod.rs`, `tests/headless.rs`, `tests/ipc.rs`

**Interfaces:**
- Consumes: Task 2 `WmSettings`, `CompositorConfig.snap_threshold`; stage 1 maximize path and usable area.
- Produces:
  - Pure: `pub enum SnapZone { Left, Right, Top }`; `pub fn snap_zone(pointer: Point, area: Rect, threshold: i32) -> Option<SnapZone>` (pointer within `threshold` of the usable area's left / right / top edge); `pub fn snap_rect(zone: SnapZone, area: Rect) -> Rect` (left / right half; `Top` = maximize).
  - Move grab: while dragging, the zone under the pointer shows a preview (a translucent `SolidColorRenderElement` of the snap rect, nested only); on release in a zone the window is resized to the half (remembering the pre-snap geometry like maximize's `restore`) or maximized for `Top`; dragging a snapped window away restores its size. `snap_enabled == false`: no zones.
  - Pure: `pub fn window_opacity(settings: &WmSettings, active: bool, override_percent: Option<u8>) -> u8` (override wins; else `100` when transparency is off; else active / inactive). Applies to toplevels only: popups, layers, the menu and the switcher are opaque (picom's menu rule). `WindowInfo.opacity_override: Option<u8>`.
  - `WindowView` gains `"opacity": u8` (effective percent).
  - IPC `window/set-opacity {"id","opacity": 20..=100 | null}` (null clears the override; out of range: error); `window/set-geometry {"id","x","y","width","height"}` (unmaximizes / unsnaps, configures the size, moves; width / height below the client's min size are clamped by the client as usual; unknown id: error).
- [ ] **Step 1: Failing tests:** unit for `snap_zone` (x = 5 with threshold 10 is `Left`, x = 15 is `None`) and `snap_rect` halves of a 1920x1040 area; `window_opacity` (defaults: active 100, inactive 88; override 50 wins; transparency off gives 100). Headless: Alt+left drag (`debug/key` Alt press, `debug/button` press, `debug/pointer` to x = 2, release) leaves the window at the left half of the usable area (geometry from `windows/list`); with `desktop.json` `{"snap_enabled": false}` written before start, the same drag leaves it unsnapped. IPC: two windows: the inactive one reports `opacity` 88, the active 100; `window/set-opacity` 50 then `null`; `window/set-geometry` moves and resizes (client configure has the size).
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop. **Step 5 (manual):** nested: drag a foot to each edge (screenshot of the preview and the result), an inactive foot looks translucent.
- [ ] **Step 6: Commit** `compositor: edge snapping, window opacity, set-geometry` (two commits if the diff is large: snapping; opacity + set-geometry).

---

### Task 6: Server side decorations: protocol and geometry

**Files:**
- Create: `src/decor/mod.rs`, `src/decor/geometry.rs`
- Modify: `src/state.rs` (`XdgDecorationState`, delegate), `src/shell/xdg.rs` (placement, maximize, fullscreen, snap and set-geometry use the insets), `src/wm/mod.rs` (`WindowView.geometry` is the frame), `src/grabs.rs`, `tests/common/mod.rs` (decoration manager binding), `tests/headless.rs`

**Interfaces:**
- Consumes: Task 5 snap / set-geometry paths, stage 1 maximize / fullscreen / cascade.
- Produces:
  - `XdgDecorationHandler for State`: a client asking `ServerSide` gets `ServerSide`; `ClientSide` or unset stays client side (GTK4 apps draw their own). `WindowInfo.decorated: bool`.
  - Pure: `pub struct Insets { pub top: i32, pub left: i32, pub right: i32, pub bottom: i32 }`; `pub fn insets(decorated: bool, fullscreen: bool, theme: &DecorTheme) -> Insets` (`TITLE_HEIGHT = 24` + `border_width` on top, `border_width` on the other sides; zero when not decorated or fullscreen); `pub enum FrameZone { Title, Button(TitleButton), Border(Edges), Client }`, `pub enum TitleButton { Close, Iconify, Maximize }`; `pub fn frame_zone(frame: Rect, point: Point, insets: Insets, layout: &TitleLayout) -> FrameZone`; `pub struct TitleLayout { left: Vec<TitleButton>, right: Vec<TitleButton> }`, `pub fn TitleLayout::parse(s: &str) -> TitleLayout` (`"close,iconify,max:"`; unknown names skipped with `err_log!`); button size `TITLE_BUTTON_SIZE = 16`, spacing `2`.
  - Every geometry computation for a decorated window goes through the insets: cascade places the frame, maximize / snap / set-geometry give the client the area minus insets, fullscreen drops them; `WindowView.geometry` and the foreign-toplevel / pager see the frame.
- [ ] **Step 1: Failing tests:** unit for `insets`, `TitleLayout::parse` and `frame_zone` (a point on the close button of a 400x300 frame, on the title, on the right border, inside the client). Headless: a client asking server side gets `ServerSide`; maximized, its configure size is `area.w - 2 * border` by `area.h - 24 - 2 * border` and `windows/list` geometry equals the usable area; fullscreen configure equals the output; a client asking client side keeps stage 1 sizes (no change for GTK4).
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop.
- [ ] **Step 5: Commit** `compositor: xdg-decoration and frame geometry`, rule 2 answer if decoration state is owned per window.

---

### Task 7: Server side decorations: drawing and title bar input

**Files:**
- Create: `src/decor/theme.rs`, `src/decor/render.rs`
- Modify: `src/menu/theme.rs` (gradient flag parsing shared), `src/input.rs` (frame clicks), `src/backend/nested.rs` (decoration elements below each window's surfaces), `tests/headless.rs`

**Interfaces:**
- Consumes: Task 6 `Insets`, `frame_zone`, `TitleLayout`; stage 2 `open_window_menu`, `set_minimized`, maximize toggle; Task 3 `lower_window`.
- Produces:
  - `pub struct DecorTheme { active_title: (Rgb, Rgb), inactive_title: (Rgb, Rgb), active_label: Rgb, inactive_label: Rgb, active_button: Rgb, inactive_button: Rgb, hover_bg: (Rgb, Rgb), pressed_bg: (Rgb, Rgb), active_border: Rgb, inactive_border: Rgb, border_width: u32 }`, `pub fn DecorTheme::parse(themerc: &str) -> DecorTheme` with the OpusDMTheme defaults (`window.active.title.bg.color` `#414141` / `colorTo` `#202020`, inactive `#DEE0D8` / `#E0E0D8`, labels `#f8f8f8` / `#747474`, buttons `#e0e0e0` / `#747474`, hover `#398dc6` / `#236d83`, pressed `#235679` / `#154350`, borders `#181818` / `#404040`, `border.width` 4); `DecorTheme::load()` uses the same search path as `MenuTheme::load`.
  - Title bar image per decorated window: vertical gradient, buttons drawn as glyphs (close X, iconify bar, maximize box) in the layout order, the title centered in `Sans Bold 9` ellipsized; cached by `(title, active, width, hovered / pressed button)`.
  - Pointer on the frame (Openbox mousebinds): left press on a button + release on the same button runs it (Close = `close_window`, Iconify = `set_minimized`, Maximize = toggle); left drag on the title moves (`start_move_grab`); double click on the title (`DOUBLE_CLICK_MS = 500`) toggles maximize; right press on the title or a border opens the window menu at the pointer; middle press on the title lowers the window; left drag on a border resizes from that edge. Every frame click raises and focuses the window first (except middle).
- [ ] **Step 1: Failing tests:** unit: `DecorTheme::parse` of the repo themerc gives `active_title.0 == #414141`; `render_title` returns a `frame_width x (24 + border)` buffer. Headless (`debug/pointer`, `debug/button`): click on the close button of a decorated window -> the client gets `xdg_toplevel.close`; double click on the title -> maximized in `windows/list`; right press on the title -> window menu open (a `Down`, `Return` minimizes as in stage 2).
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop. **Step 5 (manual):** nested, `foot` (asks server side) next to a GTK4 Lister (client side): screenshot of active and inactive title bars, each button, double click, right click menu, edge resize.
- [ ] **Step 6: Commit** `compositor: draw server side title bars and handle their clicks`.

---

### Task 8: wlr-screencopy

**Files:**
- Create: `src/protocols/screencopy.rs`
- Modify: `src/state.rs`, `src/backend/mod.rs`, `src/backend/headless.rs`, `src/backend/nested.rs`, `tests/common/mod.rs` (a screencopy client helper), `tests/headless.rs`

**Interfaces:**
- Consumes: the existing render element list of `nested.rs` (windows, layers, decorations, menu, switcher; the cursor is not captured).
- Produces:
  - Global `zwlr_screencopy_manager_v1` version 3. `capture_output(overlay_cursor, output)` and `capture_output_region(.., x, y, w, h)` (region clipped to the output; empty: `failed`) create a frame that sends `buffer(Xrgb8888, w, h, w * 4)` then `buffer_done` (v3); no `linux_dmabuf` event. `copy` / `copy_with_damage` with an shm buffer of that exact format and size queue a `PendingCapture`; any other buffer: `failed` plus `err_log!` (the protocol error would kill `opusdm-shot`; deviation noted in `CHANGES.md`). `overlay_cursor` is ignored (`debug_log!`).
  - `pub struct PendingCapture { frame, buffer, output, region: Rect, with_damage: bool }`; `pub fn State::take_pending_captures(&mut self) -> Vec<PendingCapture>`; `pub fn State::complete_capture(&mut self, c: PendingCapture, pixels: Result<Vec<u8>, String>)` (pixels: Xrgb8888, top-down, `w * 4` stride; writes the shm buffer, sends `flags(0)`, `damage(0, 0, w, h)` when `with_damage`, then `ready` with the current time; `Err`: `failed` plus `err_log!`).
  - Backends service the queue once per loop iteration: nested renders the element list into an offscreen buffer and reads it back (`smithay::backend::renderer::ExportMem`); headless fills `HEADLESS_CAPTURE_PIXEL = [0x20, 0x20, 0x20, 0xff]` (B, G, R, X), since it has no renderer.
  - A frame destroyed or a client dead before servicing: dropped silently from the queue with a `debug_log!` (not a failure).
- [ ] **Step 1: Failing tests:** headless: a client captures the output (1920x1080): receives `buffer` 1920x1080 stride 7680, `buffer_done`, then `ready`, and the first pixel equals `HEADLESS_CAPTURE_PIXEL`; a region 10,10 100x50 gives a 100x50 buffer; a buffer of the wrong size gives `failed` and a second correct capture succeeds; a client that disconnects right after `copy` leaves the compositor running and a new client captures fine.
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop. **Step 5 (manual):** nested, `opusdm-shot` full screen and region (Ctrl+Print equivalent of the sandbox table): the saved image shows the windows, decorations and panel; open the image yourself.
- [ ] **Step 6: Commit** `compositor: wlr-screencopy for opusdm-shot`, rule 2 answer (who owns pending frames; what happens when the client or the frame goes).

---

### Task 9: Clipboard, primary selection, drag and drop

**Files:**
- Create: `src/selection.rs`
- Modify: `src/state.rs` (`PrimarySelectionState`, handlers, delegates; selection focus follows the keyboard), `src/input.rs`, `src/backend/nested.rs` (DnD icon element at the pointer), `tests/common/mod.rs`, `tests/headless.rs`

**Interfaces:**
- Produces: `SelectionHandler`, `DataDeviceHandler`, `ClientDndGrabHandler`, `ServerDndGrabHandler`, `PrimarySelectionHandler` for `State`; `set_data_device_focus` and `set_primary_focus` called wherever the keyboard focus changes (one helper `State::update_selection_focus(&mut self)` used by `focus_window`, `focus_layer` and the refocus paths). Client-to-client transfers only: the compositor never reads a pipe. A drag shows the client's DnD icon surface at the pointer; a drag whose source client dies is cancelled (`cancelled` to the source if alive, `leave` to the target).
- [ ] **Step 1: Failing tests:** headless: client A sets a `text/plain;charset=utf-8` selection "hello stage3"; client B takes the keyboard, receives the offer and reads "hello stage3"; same for the primary selection; A dies after setting the selection: B, focused again, sees no stale offer that hangs (receive gets EOF or no selection); DnD (`debug/pointer` / `debug/button`): A starts a drag from its button press, the pointer moves over B, release: B gets `drop` and reads the data; A dies mid-drag: B gets `leave`, the compositor keeps running.
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop. **Step 5 (manual):** nested: copy a path in a Lister, paste it into foot; middle-click paste between two foot windows; drag a scratch file from one Lister to another (copy job runs); `opusdm-shot` editor copy image, paste into a GTK app that accepts images if one is available, else NOT CHECKED.
- [ ] **Step 6: Commit** `compositor: clipboard, primary selection and drag and drop`.

---

### Task 10: Strict xdg-activation

**Files:**
- Modify: `src/activation.rs`, `src/input.rs` (input serial bookkeeping), `src/wm/mod.rs` (`WindowView.urgent`), `tests/headless.rs`

**Interfaces:**
- Consumes: stage 2 activation, Task 9 focus helper.
- Produces (owner decision 2026-11-25: strict):
  - `State::last_input: HashMap<ClientId, Serial>`: the serial of the last key event, keyboard enter or pointer button sent to each client.
  - A token is honoured only when it is younger than `ACTIVATION_TOKEN_TIMEOUT` and its creation data carries a serial no older than the creating client's `last_input` entry, and that client is the one holding the keyboard or the pointer at creation time (launcher, panel, notification popup click). Otherwise the request is refused with `debug_log!` and the target window gets `urgent = true` (no focus change, no desktop switch).
  - `WindowInfo.urgent: bool`, cleared when the window becomes active; `WindowView` gains `"urgent": bool`.
  - New windows mapping keep getting focus as today (spec: "new windows get focus"); a pending activation applies the same policy at map time.
- [ ] **Step 1: Failing tests:** headless: a token created with no serial -> A not activated, `windows/list` shows A `urgent: true`; a token created with the serial of the client's last key event (`debug/key` tap while it has focus, harness reads the serial) activates A and clears `urgent`; a token from client C that never received input, for A -> refused; stage 2 tests that relied on serial-less tokens are updated to carry a serial (say which in the report).
- [ ] **Step 2-4:** run (fail), implement, run tests + clippy + loop.
- [ ] **Step 5: Commit** `compositor: strict xdg-activation, urgent instead of focus stealing`.

---

### Task 11: Native client API and hub reload

**Files:**
- Modify: `crates/wayland/src/native.rs`, `crates/hub/src/` (the function that applies desktop settings: `apply_desktop_settings` / `compositor.rs` picom path)

**Interfaces:**
- Consumes: Task 2 `reload`, `bindings/list`; Task 5 `window/set-opacity`, `window/set-geometry`, `WindowView.opacity`; Task 10 `urgent`.
- Produces:
  - `native::reload() -> Result<Vec<String>, String>` (the config errors), `native::builtin_bindings() -> Result<Vec<(String, String)>, String>` (`(name, key)`), `native::set_opacity(id, Option<u8>)`, `native::set_geometry(id, Rect)`; `NativeWindow` gains `opacity: u8`, `urgent: bool` (missing keys parse as 100 / false so an older compositor still works).
  - Hub: when desktop settings are applied (opacity, snap, wrap, count) under native, instead of the picom path it calls `native::reload()` on the existing IoWorker (never on the GTK or owner thread), logging the returned errors. Off native: byte-identical behaviour.
- [ ] **Step 1: Failing tests:** `native.rs` fake-server tests for `reload` and `builtin_bindings` parsing; `NativeState::read` of a window without `opacity` / `urgent` gives 100 / false. Hub: a unit test that the native branch builds the `reload` request (or the code-path argument in the report if the hub function is not unit-testable).
- [ ] **Step 2-4:** run (fail), implement, `cargo test -p opusdm-wayland -p opusdm-hub`, clippy (no new warnings). **Step 5 (manual):** run-native: change transparency / inactive opacity in Settings (sandbox config copy): the inactive window's opacity changes at once; run-x11 + short run-wayland: Settings opacity still drives picom as before (rule 4 checks).
- [ ] **Step 6: Commit** `wayland, hub: native reload, opacity and geometry requests`.

---

### Task 12: Hotkey Editor: conflicts with the native built-ins

**Files:**
- Modify: `crates/ipc/src/bindings.rs`, `crates/lister/src/hotkey_editor.rs`

**Interfaces:**
- Consumes: Task 11 `native::builtin_bindings`.
- Produces:
  - `BindingError::HotkeyConflictsWithBuiltin { index: usize, builtin: String }`, message `"conflicts with the native compositor's built-in <builtin>; the built-in wins"`, `hotkey_index` covers it.
  - `pub fn validate_hotkeys(hotkeys: &[Hotkey], known: &HashSet<FunctionId>, builtins: &[(KeyCombo, String)]) -> Vec<BindingError>`: a global hotkey whose `KeyCombo::identity` equals a built-in's yields the new error. Every existing caller passes `&[]` unless under native; behaviour with `&[]` is identical to today (pinned by the existing tests).
  - Hotkey Editor under native: fetches the built-ins once on a background thread when the editor opens, then revalidates on the GTK thread; until they arrive (or on error, logged) it validates with `&[]`. Off native nothing changes.
- [ ] **Step 1: Failing tests:** `bindings.rs`: a global `Alt+Tab` with builtins `[(parse_key("Alt+Tab"), "switcher_next")]` gives `HotkeyConflictsWithBuiltin { index: 0, builtin: "switcher_next" }`; the same hotkey not global gives nothing; with `&[]` nothing.
- [ ] **Step 2-4:** run (fail), implement, `cargo test -p opusdm-ipc -p opusdm-lister`, clippy (no new warnings). **Step 5 (manual):** run-native: in the Hotkey Editor set a global `Alt+Tab` -> the conflict note appears; run-x11: the editor behaves as before (no note), plus the rule 4 Lister checks (the lister changed).
- [ ] **Step 6: Commit** `ipc, lister: hotkey conflicts with the native built-ins`.

---

### Task 13: Sandbox table and the stage 3 exit check

**Files:**
- Modify: `run-native.sh`, `docs/WAYLAND-PORT.md`, `CHANGES.md`

**Interfaces:**
- Produces in `run-native.sh`: the sandbox `compositor.json` maps every built-in that the host Openbox grabs to `Ctrl+Super+<key>` (for example `switcher_next` `Ctrl+Super+Tab`, `close` `Ctrl+Super+F4`, `desktop_left` `Ctrl+Super+Left`, `show_desktop` `Ctrl+Super+d`, `window_menu` stays `Super+Alt+space`), printed at start with the hub table; documented in `docs/WAYLAND-PORT.md` (Native compositor section: stage 3).
- [ ] **Step 1:** implement, `bash -n`, a run checking `~/.config/opusdm` untouched.
- [ ] **Step 2: Stage 3 exit check** in `./run-native.sh --debug` (private Xvfb host, sandbox bus only), each item PASS / FAIL / NOT CHECKED with the commit it ran on, in `CHANGES.md`:
  - project CLAUDE.md rule 4 (Lister, sandbox global hotkeys, hub second Lister / SOURCE / DEST / Copy job / function run);
  - every built-in binding of the sandbox table; the switcher (Alt-hold cycling, Shift backwards, all desktops, Escape); snapping left / right / top and unsnap; opacity active / inactive and a Settings change; `foot` server side title bars (each button, double click, right click menu, middle click lower, border resize) next to GTK4 client-side windows;
  - `opusdm-shot` full screen and region (open the images); clipboard Lister -> foot, primary selection, DnD Lister -> Lister;
  - pager: mini-windows at their real position and size in their desktop's box, decorated windows drawn with their frame size;
  - activation: a notification click focuses its window if the notification daemon's click carries an input serial; a self-made token marks urgent;
  - the log has no unexplained error line.
- [ ] **Step 3:** `cargo test --workspace` (no `DISPLAY`) and `cargo clippy -p opusdm-compositor -p opusdm-wayland -p opusdm-hub -p opusdm-ipc -p opusdm-lister --all-targets -- -D warnings`; pre-existing failures reported by name, not fixed.
- [ ] **Step 4: Commit** `compositor: stage 3 sandbox table and exit check`.
- [ ] **Step 5: Stop and report to the owner.** Stage 4 (DRM, XWayland, real session) gets its own plan.

---

## Calls made in this plan (owner may correct)

- One plan for all of stage 3 (owner, 2026-11-25). Strict xdg-activation with `urgent` (owner). Log macros fixed in `crates/ipc` as an isolated, first task (owner).
- Desktop wrap follows `DesktopSettings.wrap_desktops` (labwc and the settings default), not `openbox-rc.xml`'s `wrap=no`. `Ctrl+Alt+Up/Down` are not built in (desktops are one row). `W-e` (Execute Konqueror) and the root menus are not window manager bindings: not built in.
- `Shift+Alt+Left/Right` follow the window (spec), although `openbox-rc.xml` does not say so.
- `Ctrl+Alt+Tab` lists the windows of every desktop: Openbox's `panels=yes desktop=yes` has no meaning when panels and desktop are layer surfaces.
- Title bar shading (wheel on the title) and `ToggleOmnipresent` are not implemented (no spec line); the shade binding is ignored.
- Snapping shows a preview rectangle while dragging (labwc behaviour). Openbox's edge resistance is not implemented.
- Screencopy: a wrong buffer is answered with `failed` and a log line instead of the protocol error (keeps `opusdm-shot` alive); the cursor is never captured; the headless backend returns a fixed colour.
- No `wlr-data-control`: GDK does not use it; the panel clipboard center keeps today's Wayland behaviour.
