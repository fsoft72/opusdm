# OpusDM compositor, stage 4 (real session) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `opusdm-compositor --drm` runs the owner's desktop on real hardware (udev, libseat, DRM/GBM, libinput), with every connected monitor laid out from `screens.json` and re-laid out on hot plug, keyboard and pointer set from `input.json`, X11 apps through XWayland, idle-notify and idle-inhibit, an `opusdm-session-native` script and an "OpusDM (native)" login entry, packaged as its own `opusdm-compositor` package - offered at the login screen only after the full project CLAUDE.md checklist passes on a spare VT.

**Architecture:** a third backend `backend/drm.rs` next to `nested.rs` and `headless.rs`, sharing one scene builder (`backend/scene.rs`) so the window, the DRM outputs and screencopy draw the same elements. `State` becomes multi-output (outputs, usable areas, layers and placement per output; desktops span all outputs as in Openbox / labwc). Output layout and input settings stay owned by the existing files and pure libraries (`opusdm-screen`, `opusdm-input-config`, used without GTK); the compositor reads them and serves `outputs/*` and `input/*` on the private IPC; the clients take the native path only under `under_native()`. XWayland through Smithay's `XWayland` + `X11Wm`, fed into the same window manager paths as xdg toplevels.

**Tech Stack:** Rust (edition 2021, rust-version 1.92), Smithay 0.7 with features `backend_winit`, `desktop`, `wayland_frontend` plus `backend_drm`, `backend_gbm`, `backend_egl`, `backend_libinput`, `backend_udev`, `backend_session_libseat`, `renderer_gl`, `xwayland` (add only what the build names). System (checked installed 2026-11-25): libinput-dev, libseat-dev, libudev-dev, libgbm-dev, libdrm-dev, libegl1-mesa-dev, xwayland. Tests: the stage 1-3 headless harness, `wayland-client` 0.31, smithay-client-toolkit 0.20, `x11rb` (dev-dependency, X11 test client), installed `xterm` for manual checks.

**Spec:** `docs/superpowers/specs/2026-11-25-opusdm-compositor-design.md` (stage 4, "Configuration", "Process model", "Packaging", "Anti-goals"). Stage 3 plan and ledger: `docs/superpowers/plans/2026-11-25-compositor-stage3.md`, `.superpowers/sdd/2026-11-25-compositor-stage3/progress.md`. Smithay reference: anvil v0.7.0 `udev.rs` (DRM, libseat, hot plug, VT switch) and `xwayland` / `shell/x11.rs`. Key code facts (2026-11-25): screen model and profiles `crates/screen/src/{model,store,modes,parse}.rs` (`Layout`, `MonitorConfig`, `Monitor`, `Profiles::find`, `Layout::remapped_to`, `modes::extend`, xrandr EDID id = hex chars `16..32` of the EDID, `parse.rs:146`); input model and store `crates/input-config/src/{model,store,change,labwc}.rs` (`InputSettings`, `input_path()`, `load_from`, `labwc::fold`); clients pick a backend in `crates/screen/src/backend.rs` and `crates/input-config/src/backend.rs::for_session`; hub reapply loops `crates/hub/src/screen_hotplug.rs`, `crates/hub/src/input_apply.rs`; session files `dist/opusdm-session-wayland`, `dist/opusdm-wayland.desktop`, `scripts/build-deb.sh`.

## Global Constraints

- Branch `opus-wayland`. Never push, never `--no-verify`, never a git worktree. Commit only the files the task touched; one risky change per commit (project CLAUDE.md rule 6).
- Every stage 1-3 Global Constraint holds: no `unwrap` / `expect` / `panic!` / panicking indexing outside tests; `err_log!` / `debug_log!` with tag `"opusdm-compositor"`; English, no em dash character, doc comment on every function, `UPPER_CASE` constants, one-line early exits, files end with a newline; `CHANGES.md` entry per task with what was NOT verified; rule 2 ownership answer in commit bodies that create or change ownership.
- **Anti-goal: no regression of the X11, labwc or Wayfire sessions.** Client changes are taken only under `under_native()` (evaluated once per process). Task 1 changes the build of `opusdm-screen` and `opusdm-input-config` only through a default-on feature: default builds must be identical.
- **Anti-goal: never lose the session with no way back.** The X11, labwc and Wayfire entries stay installed. `Ctrl+Alt+F1..F12` switch VT and `Ctrl+Alt+BackSpace` quits through libseat in every state (menu, switcher, grabs, no hub). A compositor that exits for any reason gives the VT back (libseat) and SIGTERMs its children.
- **The agent never runs anything on a real VT, never uses sudo, never touches DISPLAY=:0.** Every DRM check is an owner step: the agent prepares the build and the script, asks the owner to run it from a spare VT, then reads the log and captures it wrote. Nested / headless checks follow `.superpowers/sdd/2026-11-25-compositor-stage3/sandbox-protocol.md` (private Xvfb, sandbox D-Bus only, own pids, sha256 of `~/.config/opusdm` before / after).
- Target hardware (spec): one AMD iGPU (`amdgpu`), panel `eDP-1` 1920x1080, scale 1, external monitors by hot plug. Single GPU: no multi-GPU, no NVIDIA. Fractional or `MonitorConfig.scale != 1.0` is stage 5: ignored with an `err_log!` naming the monitor.
- Configuration files stay the single source of truth, read only by the compositor: `screens.json` (one layout per set of connected monitors), `input.json`, `desktop.json`, `compositor.json`. The compositor never writes them.
- EDID id for native monitors = the xrandr one (lowercase hex of the EDID, chars `16..32`; the connector name when there is no EDID), so profiles saved under X11 match.
- Packaging (spec): `opusdm-compositor` is its own package; the main package `Recommends` it; labwc, wayfire, wlr-randr and picom stay in `Recommends`. The "OpusDM (native)" entry ships only after Task 12 passes.

## Review Focus

- VT switch away and back (and a held `Ctrl+Alt` when leaving): rendering and input resume, no key or button stays pressed, open grabs / menu / switcher end cleanly. Test in Task 3 (`debug/session` pause / resume on headless runs the same handler).
- A monitor unplugged while windows and the panel live on it: windows move fully onto a remaining output, its layer surfaces get `closed`, nothing is left off-screen, the active window stays valid. Test in Task 4.
- A `screens.json` profile that cannot be applied (mode not offered, all monitors off, overlapping or negative positions, `scale != 1`): a usable layout is still set (at least one output on), the problem is logged and returned in `outputs/apply` / `reload` errors. Test in Task 5.
- An `input.json` keyboard layout that xkb cannot compile: the previous keymap stays, the error is logged and returned by `input/apply`. Test in Task 7.
- An X11 client asking for focus (`_NET_ACTIVE_WINDOW`) without user input, or an override-redirect window covering the screen: the strict activation policy applies (urgent, no focus steal); override-redirect windows never take the keyboard. Test in Task 9.

---

## Before execution: owner decisions carried from stage 3

Not tasks of this plan unless the owner decides; the controller asks before Task 1.

1. Strict xdg-activation changes three native flows (notification center click, Lister Sidecar refocus, hub windows shown by a hotkey: urgent instead of raise). Options: keep; a short grace after a forwarded hub hotkey; a panel-made token for the notification center.
2. Smithay 0.7.0 `GL_INVALID_VALUE` after a hidden window is resized twice (stale damage on a cached texture). On DRM the same import path runs. Options: workaround (import hidden windows' surfaces each frame) as an extra task after Task 3; report upstream; leave.

---

## File map

```
crates/screen/Cargo.toml, crates/input-config/Cargo.toml   feature "gtk" (default) gates GTK deps / modules
crates/screen/src/parse.rs          pub fn edid_id_from_hex (extracted, same behaviour)
crates/screen/src/backend.rs        NativeBackend (outputs/list, outputs/apply) under native
crates/input-config/src/backend.rs  NativeBackend (input/apply) under native
crates/hub/src/screen_hotplug.rs, input_apply.rs   do nothing under native (logged once)
crates/wayland/src/native.rs        outputs_list, outputs_apply, input_apply
crates/compositor/
  Cargo.toml                features above; opusdm-screen, opusdm-input-config (no gtk)
  src/args.rs               BackendKind::Drm, --drm, --outputs for headless
  src/backend/scene.rs      one scene builder per output (moved from nested.rs)
  src/backend/drm.rs        udev + libseat + DRM/GBM + libinput, VT switch, hot plug
  src/outputs.rs            OutputSet: outputs, layout, usable areas, rehome (pure parts)
  src/layout.rs             choose_layout, monitors from connectors (pure)
  src/input_config.rs       xkb + libinput settings from InputSettings (pure parts)
  src/xwayland.rs           XWayland spawn, X11Wm handler, X11 windows in the WM
  src/idle.rs               idle-notify, idle-inhibit
  src/ipc/protocol.rs       outputs/list, outputs/apply, input/apply, debug/outputs, debug/session
dist/opusdm-session-native, dist/opusdm-native.desktop, scripts/native-vt-check.sh
scripts/build-deb.sh, dist/install.sh, docs/WAYLAND-PORT.md, CHANGES.md
```

---

### Task 1: GTK-free `opusdm-screen` and `opusdm-input-config` libraries

**Files:** Modify `crates/screen/Cargo.toml`, `crates/input-config/Cargo.toml`, `crates/input-config/src/lib.rs`, `crates/screen/src/parse.rs`.

**Interfaces:**
- Produces: feature `gtk` (in `default`) on both crates; `gtk4`, `opusdm-ui`, `async-channel` become optional deps enabled by it; `input-config` modules `ui` and `worker` are `#[cfg(feature = "gtk")]`; both binaries `required-features = ["gtk"]`. `pub fn edid_id_from_hex(hex: &str, fallback: &str) -> String` in `crates/screen/src/parse.rs` (chars `16..32` when long enough, else `fallback`), used by `parse.rs:146`.
- [ ] **Step 1: Failing test** `edid_id_from_hex_matches_the_xrandr_parse`: for the two EDIDs of the `DUAL` fixture it returns `"06afa68400000000"` / `"0469a3273c390000"`; `edid_id_from_hex("abc", "DP-2") == "DP-2"`.
- [ ] **Step 2:** run (fail), implement, `cargo test -p opusdm-screen -p opusdm-input-config`; `cargo build -p opusdm-screen -p opusdm-input-config --no-default-features --lib` succeeds and `cargo tree -p opusdm-screen --no-default-features -e normal | grep -c gtk4` prints `0`.
- [ ] **Step 3:** default build unchanged: `cargo build --workspace` ok; `opusdm-screen` and `opusdm-input-config` binaries still built by `scripts/build-release.sh` (run its build step only). No manual check needed (no code path changes); CHANGES.md says so.
- [ ] **Step 4: Commit** `screen, input-config: libraries usable without GTK`.

---

### Task 2: Shared scene builder and DRM build flags

**Files:** Create `src/backend/scene.rs`. Modify `Cargo.toml`, `src/args.rs`, `src/backend/mod.rs`, `src/backend/nested.rs`.

**Interfaces:**
- Produces: `pub fn scene_elements<R>(renderer: &mut R, state: &State, output: &Output) -> Vec<SceneElement<R>>` (everything `_scene_elements` draws today: windows with alpha and frames, layers, snap preview, DnD icon excluded as now, menu, switcher), and `pub fn cursor_elements(..)` for the backends that draw a software cursor. `nested.rs` calls them; screencopy still uses `scene_elements`. `BackendKind::Drm` and `--drm` (exclusive with `--nested` / `--headless`; `--size` refused with `--drm`). Smithay features added in `Cargo.toml` (Tech Stack list).
- [ ] **Step 1: Failing tests:** unit `parse_args(["--drm"])` gives `Drm`; `["--drm","--size","800x600"]` is an error naming `--size`; `["--drm","--nested"]` is an error.
- [ ] **Step 2:** run (fail), implement (the move is behaviour-neutral: same element order, same alpha), run crate tests + clippy + headless/ipc loop 5 times. `--drm` prints `"--drm: not implemented yet"` via `err_log!` and exits 2 until Task 3.
- [ ] **Step 3 (manual):** nested in the private Xvfb: foot with title bar, switcher, window menu, snap preview, opacity, `opusdm-shot` full screen: same as the stage 3 screenshots (compare side by side, open them).
- [ ] **Step 4: Commit** `compositor: one scene builder for every backend, DRM build features`.

---

### Task 3: DRM backend, one output, VT switching

**Files:** Create `src/backend/drm.rs`, `examples/capture.rs`, `scripts/native-vt-check.sh`. Modify `src/backend/mod.rs`, `src/input.rs` (VT bindings), `src/ipc/protocol.rs` (`debug/session`), `src/state.rs`, `tests/headless.rs`.

**Interfaces:**
- Consumes: Task 2 `scene_elements`, `cursor_elements`.
- Produces:
  - `drm::run(args)`: `LibSeatSession`, udev primary GPU (`primary_gpu(seat)`, else the first card), `DrmDevice` + `GbmAllocator` + `GlesRenderer`, one `DrmCompositor` on the first connected connector at its preferred mode (pure `pub fn pick_mode(modes: &[ModeInfo], preferred: Option<usize>) -> Option<usize>`: preferred, else the largest area, then the highest refresh), rendering on vblank, software cursor, `LibinputInputBackend` with `LibinputSessionInterface`. Screencopy captures serviced from the same scene (Task 8 of stage 3 queue).
  - Session pause: DRM deactivated, libinput suspended, `State::session_paused(&mut self)` releases every held key and button to clients, ends grabs, closes menu and switcher; resume: libinput resumed, DRM reset, full redraw. Headless `debug/session {"active": bool}` calls the same `session_paused` / `session_resumed`.
  - Safety bindings: `Ctrl+Alt+F1..F12` call `session.change_vt(n)` (nested / headless: `debug_log!`, nothing), never overridable, working with menu / switcher / grabs open.
  - SIGTERM / `session/quit` / `Ctrl+Alt+BackSpace`: children get SIGTERM, outputs are released, the loop ends, exit code 0.
  - `scripts/native-vt-check.sh`: run by the owner on a spare VT (`Ctrl+Alt+F3`, log in): refuses when `$XDG_SESSION_TYPE` is not `tty` or `DISPLAY` / `WAYLAND_DISPLAY` is set; private `XDG_CONFIG_HOME` copy as `run-native.sh` does (`cp -RL`, refuses symlinks); runs `opusdm-compositor --drm --debug --startup <hub>` with the sandbox hotkey and built-in tables; writes `~/opusdm-vt-check-<date>/compositor.log` and, every 30 s, a capture into the same folder made by `cargo run --example capture` (new `crates/compositor/examples/capture.rs`: binds `zwlr_screencopy_manager_v1` through `opusdm_wayland::screencopy`, captures every output, writes binary PPM `P6` files `capture-<output>-<HHMMSS>.ppm`, no new dependency; `opusdm-shot` has no command line mode); `Ctrl+Alt+BackSpace` ends it.
- [ ] **Step 1: Failing tests:** unit `pick_mode` (preferred wins; none preferred: 2560x1440@60 beats 1920x1080@144; same size: higher refresh). Headless: press `Ctrl`, `Alt`, `a` (client focused), then `debug/session {"active": false}` -> the client receives the releases of all three keys and `keys_received()` balance is zero; resume works and a new key reaches the client. Headless: `Ctrl+Alt+F2` with the window menu open logs `"VT switch to 2"` (headless form) and the menu stays usable afterwards.
- [ ] **Step 2-4:** run (fail), implement, tests + clippy + loop. `bash -n scripts/native-vt-check.sh`; `cargo build -p opusdm-compositor --example capture` (`opusdm-wayland` as a dev-dependency); the example run against the nested compositor in the private Xvfb writes a PPM that ImageMagick opens and that shows the desktop.
- [ ] **Step 5 (owner step):** the agent asks the owner to run `scripts/native-vt-check.sh` on a spare VT and check: desktop, panel, dock appear on `eDP-1`; a Lister opens; `Ctrl+Alt+F1`/`F2` back to the X11 session and return; `Ctrl+Alt+BackSpace` ends it and the VT is usable. The agent reads the log folder (open the captures) and records each item PASS / FAIL / NOT CHECKED in CHANGES.md.
- [ ] **Step 6: Commit** `compositor: DRM backend with libseat, libinput and VT switching`, rule 2 answer (who owns the session, DRM device, GBM allocator; what happens on pause, on device loss, on exit).

---

### Task 4: Several outputs in `State`

**Files:** Create `src/outputs.rs`. Modify `src/state.rs`, `src/shell/layer.rs`, `src/shell/xdg.rs`, `src/snap.rs`, `src/wm/mod.rs`, `src/protocols/foreign_toplevel.rs`, `src/protocols/workspace.rs`, `src/backend/headless.rs`, `src/args.rs` (`--outputs`), `src/ipc/protocol.rs` (`debug/outputs`), `tests/common/mod.rs`, `tests/headless.rs`.

**Interfaces:**
- Produces:
  - `pub struct OutputSet` in `State::outputs` (name -> `Output`, its rect in the global space, its usable area). Pure: `pub fn output_for(rects: &[(String, Rect)], point: Point) -> Option<String>` (containing output, else nearest by distance to its rect); `pub fn rehome(window: Rect, from: Rect, to: Rect) -> Rect` (same relative position, clamped fully inside `to`, size shrunk to `to` if larger).
  - Placement per output: cascade on the output under the pointer; maximize / snap / fullscreen on the output that contains the window's centre; usable area per output from that output's exclusive layers. Layer surfaces bind to their requested output, else the output under the pointer at creation.
  - Desktops span all outputs (Openbox / labwc): one desktop index for the whole space; ext-workspace group and foreign-toplevel send `output_enter` for every output a window / group touches.
  - Output removed: its windows go through `rehome` to the primary (else first) remaining output, its layer surfaces get `closed`, the active window stays valid, `windows/list` and the pager update.
  - Headless `--outputs WxH[+X+Y][,WxH+X+Y...]` (default one output as today); `debug/outputs {"remove": name}` / `{"add": {"name","width","height","x","y"}}` headless only.
- [ ] **Step 1: Failing tests:** unit `output_for` (inside; outside picks nearest), `rehome` (relative position kept; too large shrinks; result inside `to`). Headless with `--outputs 1920x1080+0+0,1280x1024+1920+0`: a window maximized on the second output gets 1280x1024 minus that output's exclusive zones; a panel layer bound to output 1 reduces only output 1's usable area; snap left on output 2 gives `x = 1920`; removing output 2 moves its window fully inside output 1 and its layer gets `closed`; stage 1-3 headless tests unchanged with one output.
- [ ] **Step 2-4:** run (fail), implement, tests + clippy + loop.
- [ ] **Step 5: Commit** `compositor: several outputs, placement and layers per output`, rule 2 answer (who owns outputs; what happens to windows and layers of a removed output).

---

### Task 5: Layouts from `screens.json`, hot plug, `outputs/*`

**Files:** Create `src/layout.rs`. Modify `Cargo.toml` (`opusdm-screen` with `default-features = false`), `src/backend/drm.rs`, `src/backend/headless.rs`, `src/state.rs`, `src/ipc/protocol.rs`, `tests/ipc.rs`, `tests/headless.rs`.

**Interfaces:**
- Consumes: Task 1 `edid_id_from_hex`, `Profiles::find`, `Layout::remapped_to`, `modes::extend`; Task 4 `OutputSet`, `rehome`.
- Produces:
  - Pure: `pub fn monitor_from_connector(name: &str, edid: Option<&[u8]>, modes: &[(u32, u32, f64)], preferred: Option<(u32, u32)>) -> Monitor` (EDID id = `edid_id_from_hex(lowercase hex, name)`); `pub fn choose_layout(monitors: &[Monitor], profiles: Option<&Profiles>) -> (Layout, Vec<String>)` (saved profile remapped to these monitors, else `modes::extend` of the defaults; then sanitised: a mode the monitor lacks -> its default with an error; `scale != 1.0` -> treated as 1.0 with an error; no monitor enabled -> the first one enabled with an error; positions shifted to origin; overlapping non-mirrored monitors -> laid out left to right with an error); `pub fn rate_mode(mode: &Mode, rate: Option<f64>) -> f64` (nearest listed rate).
  - Applying a `Layout`: DRM sets mode, position, transform (rotation) per connector, disables the others; headless / nested move and resize their virtual outputs (nested: one output, positions only). At startup and on every udev connector change the DRM backend rebuilds the monitor list and applies `choose_layout(.., load_from(profiles_path()))`.
  - IPC `outputs/list` -> `{"ok":true,"monitors":[{"name","edid_id","modes":[{"width","height","rates":[..]}],"preferred":[w,h]|null}],"layout":<Layout JSON>}`; `outputs/apply {"layout": <Layout JSON>}` -> applies the sanitised layout, reply `{"ok":true,"errors":[..]}` (it does not write `screens.json`; the client saves as today). `reload` also re-reads `screens.json`.
- [ ] **Step 1: Failing tests:** unit `monitor_from_connector` with the `DUAL` fixture EDID bytes gives `edid_id == "06afa68400000000"`; `choose_layout` cases: saved profile for these monitors (positions kept), a profile saved under other port names (remapped), no profile (extend, left to right), mode not offered, `scale` 2.0, all disabled, overlap - each with the expected layout and one error line. Headless (`--outputs` two outputs, `screens.json` in the test config dir with a swapped order): `outputs/list` shows the swapped positions; `outputs/apply` with output 2 disabled rehomes its windows; a bad layout returns errors and keeps at least one output on.
- [ ] **Step 2-4:** run (fail), implement, tests + clippy + loop.
- [ ] **Step 5 (owner step):** `scripts/native-vt-check.sh` with an external monitor: plug it while running (the saved X11 profile for laptop + monitor is applied), unplug it (windows come back to `eDP-1`). The agent reads the log and captures, records PASS / FAIL / NOT CHECKED.
- [ ] **Step 6: Commit** `compositor: screens.json layouts, hot plug and outputs IPC`.

---

### Task 6: Screen settings and hub under native

**Files:** Modify `crates/wayland/src/native.rs`, `crates/screen/src/backend.rs`, `crates/hub/src/screen_hotplug.rs`.

**Interfaces:**
- Consumes: Task 5 `outputs/list`, `outputs/apply`.
- Produces: `native::outputs_list() -> Result<serde_json::Value, String>`, `native::outputs_apply(layout_json) -> Result<Vec<String>, String>`; `opusdm_screen::backend::NativeBackend` (`query` maps `outputs/list` to `Parsed { monitors, layout }`, `apply` calls `outputs_apply`, errors to `ScreenError`), chosen by the existing backend selection only under `under_native()`. Hub `screen_hotplug::start()` under native: one `debug_log!` ("the native compositor applies screens.json itself") and no thread.
- [ ] **Step 1: Failing tests:** `native.rs` fake-server tests for both calls; `NativeBackend::query` parses a recorded `outputs/list` reply into the expected `Monitor` / `Layout`; selection returns the native backend only when `under_native()` is forced true in the test seam the crate already uses (or a new `for_native()` constructor tested directly).
- [ ] **Step 2-4:** run (fail), implement, `cargo test -p opusdm-wayland -p opusdm-screen -p opusdm-hub`, clippy (no new warnings).
- [ ] **Step 5 (manual):** run-native: the Screens settings list the virtual output, apply a position change (the revert timer works); run-x11 and short run-wayland: Screens settings behave as before (rule 4 checks, hub changed).
- [ ] **Step 6: Commit** `screen, hub, wayland: screen settings through the native compositor`.

---

### Task 7: Keyboard and pointer settings, `input/apply`

**Files:** Create `src/input_config.rs`. Modify `Cargo.toml` (`opusdm-input-config`, `default-features = false`), `src/backend/drm.rs`, `src/backend/nested.rs`, `src/state.rs`, `src/ipc/protocol.rs`, `crates/wayland/src/native.rs`, `crates/input-config/src/backend.rs`, `crates/hub/src/input_apply.rs`, `tests/ipc.rs`.

**Interfaces:**
- Produces:
  - Pure: `pub fn xkb_names(k: &KeyboardSettings) -> (String, String, String)` (layouts joined with `,`, variants joined with `,`, options = `switch_option` + `extra_options` joined with `,`, empty parts dropped); `pub fn repeat(k: &KeyboardSettings) -> (i32, i32)` (rate, delay).
  - At startup, on `input/apply` and on every libinput device added: keyboard via `KeyboardHandle::set_xkb_config` + `change_repeat_info`; a keymap that fails to compile keeps the previous one, `err_log!`, error in the reply. Pointer (DRM only): accel speed and flat / adaptive profile, left-handed, natural scroll for mice; tap, natural scroll, disable-while-typing, speed for touchpads; a device that refuses a setting logs it.
  - IPC `input/apply` (no data: re-read `input.json`; `{"settings": <InputSettings JSON>}`: apply those without saving) -> `{"ok":true,"errors":[..]}`.
  - Clients: `native::input_apply(settings: Option<&serde_json::Value>)`; `opusdm_input_config::backend::SessionBackend::Native` (chosen only under `under_native()`): keeps the folded settings (`labwc::fold`), `apply(change)` sends the whole settings, `read_current` = the folded settings (initial: `input.json` or defaults). Hub `input_apply::start()` under native: one `debug_log!`, no loop.
- [ ] **Step 1: Failing tests:** unit `xkb_names` (`[us, it(intl)]` + `grp:alt_shift_toggle` -> `("us,it", ",intl", "grp:alt_shift_toggle")`), `repeat`. IPC (headless): `input/apply` with layout `"zz"` returns an error and the keymap still produces `a` for keycode 38 (`debug/key` + client keysym); with `"it"` a later `debug/key` of keycode 39 reaches the client as the Italian keysym. Client: `native.rs` fake-server test; `SessionBackend::Native.apply` sends the folded settings.
- [ ] **Step 2-4:** run (fail), implement, `cargo test -p opusdm-compositor -p opusdm-input-config -p opusdm-wayland -p opusdm-hub`, clippy.
- [ ] **Step 5 (manual + owner step):** run-native: Input settings change the keyboard layout and repeat at once; run-x11 / short run-wayland: Input settings as before. Owner on the VT: touchpad tap and natural scroll follow Settings.
- [ ] **Step 6: Commit** `compositor, input-config, hub: input settings through the native compositor`.

---

### Task 8: XWayland, X11 windows on screen

**Files:** Create `src/xwayland.rs`. Modify `src/state.rs`, `src/backend/mod.rs`, `src/child.rs` (`DISPLAY` for children), `src/wm/mod.rs`, `src/input.rs`, `Cargo.toml` (`x11rb` dev-dependency), `tests/common/mod.rs`, `tests/headless.rs`.

**Interfaces:**
- Produces: XWayland spawned at startup on every backend except when `--no-xwayland` (tests that do not need it); `XWaylandEvent::Ready` starts `X11Wm`; `DISPLAY=:<n>` exported to children (the hub and everything it starts). `XwmHandler for State`: toplevels map with cascade placement, focus on map (same rule as xdg), raise, move / resize (X11 configure requests honoured for unmapped windows, clamped to the output when mapped), close (`WM_DELETE_WINDOW`, else `kill_client`), minimize; override-redirect windows placed where they ask, above toplevels, never focused. X11 windows live in the same `Space` / `WindowInfo` map as xdg ones (one `WindowId` space). If `Xwayland` is missing or dies: `err_log!`, the compositor keeps running, `DISPLAY` is not exported (startup) or stays dead (later).
- [ ] **Step 1: Failing tests:** headless (Xwayland installed): an `x11rb` client creates and maps a 400x300 window -> it appears in `windows/list` with its title and gets keyboard focus (X11 `FocusIn`); an override-redirect window does not take focus; `window/close` sends `WM_DELETE_WINDOW`; a client without `WM_DELETE_WINDOW` is killed. Unit: none needed beyond reuse.
- [ ] **Step 2-4:** run (fail), implement, tests + clippy + loop.
- [ ] **Step 5 (manual):** nested in the private Xvfb: `xterm` from the run dialog appears, takes keys, moves with Alt+drag, closes from the title bar.
- [ ] **Step 6: Commit** `compositor: XWayland and X11 windows`, rule 2 answer (who owns the XWayland process and the X11Wm; what happens when Xwayland dies).

---

### Task 9: X11 windows as first-class windows

**Files:** Modify `src/xwayland.rs`, `src/decor/mod.rs`, `src/activation.rs`, `src/selection.rs`, `src/wm/mod.rs`, `src/protocols/foreign_toplevel.rs`, `tests/headless.rs`.

**Interfaces:**
- Produces: `windows/list` / foreign-toplevel / pager for X11 windows (title, `WM_CLASS` class as app id, `_NET_WM_PID` as pid, desktop, geometry = frame); `window/kill` by that pid; move to desktop, minimize, maximize and fullscreen through `_NET_WM_STATE` requests and the window menu; server side decoration for X11 toplevels unless `_MOTIF_WM_HINTS` asks for none; `_NET_ACTIVE_WINDOW` requests go through the strict activation policy (honoured only from the client holding keyboard or pointer input, else `urgent`); clipboard and primary selection bridged both ways through Smithay's X11 selection support; transient windows placed centred on their parent and kept above it.
- [ ] **Step 1: Failing tests:** headless: an X11 window shows `pid`, `app_id` from `WM_CLASS` and `desktop` in `windows/list`; `window/move-to-desktop` hides it; `_NET_WM_STATE_FULLSCREEN` makes it cover the output with no frame; a `_NET_ACTIVE_WINDOW` from a background X11 client marks the target urgent and does not move focus; Wayland client A sets the clipboard, the X11 client reads it with `XConvertSelection` (and the reverse); a window with Motif "no decorations" has no frame.
- [ ] **Step 2-4:** run (fail), implement, tests + clippy + loop.
- [ ] **Step 5 (manual):** nested: `xterm` in the panel tasklist and the pager, Force kill from the window menu, copy from foot paste into xterm and back, xterm fullscreen.
- [ ] **Step 6: Commit** `compositor: X11 windows in the tasklist, pager, clipboard and activation policy`.

---

### Task 10: idle-notify and idle-inhibit

**Files:** Create `src/idle.rs`. Modify `src/state.rs`, `src/input.rs`, `tests/headless.rs`.

**Interfaces:**
- Produces: `ext_idle_notifier_v1` (Smithay `IdleNotifierState`) with activity reported on every key, button, motion and touch; `zwp_idle_inhibit_manager_v1` (Smithay `IdleInhibitManagerState`): an inhibitor whose surface is shown (mapped, on the current desktop, not minimized) suspends idle; hidden or destroyed inhibitors stop counting.
- [ ] **Step 1: Failing tests:** headless: a client asks an idle notification with a 200 ms timeout -> `idled` after about 200 ms with no input, `resumed` after a `debug/key`; with an inhibitor on a shown surface no `idled` within 600 ms; minimizing that surface lets `idled` arrive.
- [ ] **Step 2-4:** run (fail), implement, tests + clippy + loop.
- [ ] **Step 5: Commit** `compositor: idle-notify and idle-inhibit`.

---

### Task 11: Native session script and login entry (not installed yet)

**Files:** Create `dist/opusdm-session-native`, `dist/opusdm-native.desktop`. Modify `docs/WAYLAND-PORT.md`.

**Interfaces:**
- Produces: `opusdm-session-native` modelled on `dist/opusdm-session-wayland`: `PATH`, `XDG_SESSION_TYPE=wayland`, `XDG_CURRENT_DESKTOP=OpusDM`, `unset GDK_BACKEND`, portal config copy, kwallet stop, `dbus-update-activation-environment --systemd WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE` run by the compositor's startup command before the hub, then `exec opusdm-compositor --drm --startup "<this script> --hub"`; `--hub` mode updates the activation environment, restarts the portals and `exec`s the hub (as the Wayfire path does). `opusdm-native.desktop`: `Name=OpusDM (native)`, `Exec=@PREFIX@/opusdm-session-native`, `DesktopNames=OpusDM`. Not added to `build-deb.sh` / `install.sh` until Task 13.
- [ ] **Step 1:** `sh -n dist/opusdm-session-native`; a test run of `dist/opusdm-session-native --hub` with a fake `HERE/bin/opusdm-hub` (prints its env) in a temp dir shows the expected environment.
- [ ] **Step 2: Commit** `dist: opusdm-session-native and its login entry`.

---

### Task 12: Spare VT exit check (owner step)

**Files:** Modify `CHANGES.md`, `scripts/native-vt-check.sh` (session mode).

- [ ] **Step 1:** `native-vt-check.sh --session` runs `dist/opusdm-session-native` instead of the bare compositor, with the owner's real hotkey table (no sandbox remapping: nothing else grabs keys on the VT) and the private config copy.
- [ ] **Step 2 (owner):** on a spare VT, the full project CLAUDE.md checklist with the real combos: Lister (Enter in path bar, double click folder, Backspace / parent, Alt+Left/Right, F-keys); global hotkeys (`Ctrl+backslash`, `Ctrl+Shift+space`, `Ctrl+Print`, `Alt+1` / `Alt+2`); hub (second Lister, SOURCE / DEST, Copy job, function run); built-ins (`Alt+Tab` hold / Shift, `Alt+F4`, `Super+d`, `Ctrl+Alt+Left/Right`, `Shift+Alt+Right`, `Alt+space`); snapping; title bars on foot; an X11 app (`xterm`); external monitor plug / unplug; touchpad settings; VT switch away and back; `Ctrl+Alt+BackSpace`.
- [ ] **Step 3:** the agent reads the log folder, opens every capture, records each item PASS / FAIL / NOT CHECKED with the commit in CHANGES.md; every log line with `ERROR` or `WARN` (tracing lines included) is explained or listed as FAIL. A FAIL is not fixed in this task: it goes back to the controller.
- [ ] **Step 4:** `cargo test --workspace` (no DISPLAY, temp HOME / XDG_CONFIG_HOME) and clippy on the touched crates; pre-existing items by name.
- [ ] **Step 5: Commit** `compositor: stage 4 spare VT exit check`.

---

### Task 13: Packaging

Only when Task 12 has no open FAIL (or the owner accepted each one).

**Files:** Modify `scripts/build-deb.sh`, `dist/install.sh`, `docs/WAYLAND-PORT.md`, `CHANGES.md`.

**Interfaces:**
- Produces: a second package `opusdm-compositor` from `build-deb.sh`: `/opt/opusdm/bin/opusdm-compositor`, `/opt/opusdm/opusdm-session-native`, `/usr/share/wayland-sessions/opusdm-native.desktop`; `Depends` from `dpkg-shlibdeps` on the compositor binary only (libinput, libseat, gbm, EGL, xkbcommon), `Recommends: xwayland`, `Depends: opusdm (= same version)`. The `opusdm` package adds `opusdm-compositor` to `Recommends` and keeps every other entry unchanged. `install.sh` installs the native entry only when the compositor binary is present.
- [ ] **Step 1:** `bash -n` both scripts; build both packages into a temp dir; `dpkg-deb -I` / `-c` show the fields and files above; `opusdm` package contents identical to before except the `Recommends` line (diff the two `-c` listings).
- [ ] **Step 2: Commit** `packaging: opusdm-compositor package and the native session entry`.
- [ ] **Step 3: Stop and report to the owner.** Stage 5 (screen lock, screen sharing, keyboard-shortcuts-inhibit, fractional scale) gets its own plan.

---

## Calls made in this plan (owner may correct)

- XWayland starts with the compositor, not lazily on the first X11 client: Smithay 0.7.0 `XWayland::spawn` opens the sockets and starts the server at once, and a lazy start would need our own socket listener. `--no-xwayland` exists for tests.
- Desktops span all monitors (Openbox / labwc), not one set per monitor.
- `MonitorConfig.scale` other than 1.0 is ignored with a logged error until stage 5 (its xrandr meaning, "draw larger and shrink", has no exact Wayland equivalent).
- Native monitors use the xrandr EDID id, so the layouts saved under X11 apply unchanged.
- `outputs/apply` and `input/apply` never write files; the settings apps keep saving them as today.
- DRM cursor plane and direct scan-out are not used in this stage (software cursor through the scene); they are optimisations for later.
- Every real-hardware check is an owner step on a spare VT; the agent prepares, then reads logs and captures.
