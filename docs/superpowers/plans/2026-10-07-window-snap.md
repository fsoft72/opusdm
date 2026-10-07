# Window snap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dragging any X11 window to a screen edge shows a KDE-style overlay and releasing places the window in that zone; dragging a snapped window away restores its previous size.

**Architecture:** New crate `crates/snap` (`opusdm-snap`, no GTK). Pure geometry (`zones`, `restore`) and a pure drag state machine (`machine`) are unit tested. A thin X11 layer (`x11`: sampling and RandR, `overlay`: ARGB window) feeds the machine from a polling thread (`watcher`). The hub starts the watcher and executes the resulting placements through `opusdm_ui::with_x11`, so `opusdm-snap` does not depend on GTK.

**Tech Stack:** Rust 2021, `x11rb` 0.13 (xproto, randr, shape not needed), existing `opusdm_ui::x11` helpers, `opusdm_ipc::DesktopSettings`.

**Spec:** `docs/superpowers/specs/2026-10-07-window-snap-design.md`

## Global Constraints

- `EDGE_THRESHOLD` = 20 px, `RESTORE_THRESHOLD` = 8 px, poll interval 16 ms, polling only while the left button is down.
- Zones: `Maximize` (top edge between corners), four corners (1/2 x 1/2), `Left`/`Right` (middle third of the side edge, 1/2 width x full height), `LeftTop`/`RightTop` (upper third, 1/2 x 1/3 at the top), `LeftBottom`/`RightBottom` (lower third, 1/2 x 1/3 at the bottom). No zone on the bottom edge between corners. Corners win over side thirds.
- Work area excludes panel and dock (`_NET_WORKAREA`), computed on the monitor under the pointer.
- Ignore windows of type dock, desktop, splash, and windows with equal min and max size hints.
- `snap_enabled` defaults to `true`, stored in `DesktopSettings` (`~/.config/opusdm/desktop.json`), old files must still load.
- Repo rules: comments on every function, constants UPPER_CASE, early one-line exits, English only, no em dash, files end with a newline, update `CHANGES.md`, `pnpm` not relevant, commit per task on branch `snapping`, never `--no-verify`, no AI attribution lines, no push.

## Review Focus

- Pointer on a corner exactly at an edge/third boundary: corner must win, thirds must not overlap or leave gaps (tests in Task 1).
- Work area with a top panel and a second monitor with an x offset: zones follow the monitor, not the screen (Task 1).
- Click on a titlebar without moving, or a rubber-band selection inside a window: nothing snaps and nothing is restored (Task 3).
- Window closed or moved by other means while snapped: stale restore entry must not resize it later (Task 3).
- Re-snapping an already snapped window keeps the ORIGINAL pre-snap size (Task 3).
- No RandR, no `_NET_WORKAREA`, no compositor: snap disables itself or draws nothing, logs once, never panics (Task 5).

---

### Task 1: Pure zone geometry

**Files:**
- Create: `crates/snap/Cargo.toml`, `crates/snap/src/lib.rs`, `crates/snap/src/zones.rs`
- Modify: `Cargo.toml` (add `"crates/snap"` to workspace members)

**Interfaces:**
- Produces: `pub struct Rect { x, y, w, h: i32 }` (`Copy, PartialEq, Eq, Debug`), `pub enum Zone { Maximize, TopLeft, TopRight, BottomLeft, BottomRight, Left, Right, LeftTop, RightTop, LeftBottom, RightBottom }`, `pub const EDGE_THRESHOLD: i32 = 20`, `pub fn zone_at(point: (i32, i32), area: Rect) -> Option<Zone>`, `pub fn zone_rect(zone: Zone, area: Rect) -> Rect`, `Rect::contains(&self, (i32, i32)) -> bool`.

- [ ] **Step 1: Write the failing tests** in `zones.rs` (`#[cfg(test)]`): on `AREA = Rect{x:0,y:30,w:1920,h:1050}` check `zone_at((0,30))==TopLeft`, `(960,30)==Maximize`, `(1919,30)==TopRight`, `(0,1079)==BottomLeft`, `(1919,1079)==BottomRight`, `(0,400)==LeftTop` (upper third is y 30..380 minus corner zone), `(0,600)==Left`, `(0,900)==LeftBottom`, `(1919,600)==Right`, `(960,1079)==None`, `(960,500)==None`, a point 21 px from the left edge is `None`; with a second monitor `Rect{x:1920,...}` the left edge of that monitor is `Left` zone of that monitor; `zone_rect` for each zone equals the spec table (e.g. `LeftTop` = `Rect{x:0,y:30,w:960,h:350}`, `Maximize` = the whole area); thirds partition the side edge without gaps: for every y in the area the zone on `x=0` is `Some` and `zone_rect` of it contains y (apart from the corner bands).
- [ ] **Step 2: Run** `cargo test -p opusdm-snap zones` and see it fail (crate missing).
- [ ] **Step 3: Implement.** `zone_at`: exit `None` if the point is outside `area`; `near_left = x - area.x < EDGE_THRESHOLD`, `near_right`, `near_top`, `near_bottom` likewise. Corner band = `EDGE_THRESHOLD * 4` (80 px) along the edge from a corner: a point near a side edge and within the band of a vertical end is the corner zone (top/bottom); a point near top and within the band of a horizontal end is the corner zone. Otherwise near top = `Maximize`; near left/right: split the area height into thirds by `(y - area.y) * 3 / area.h` (0 = `*Top`, 1 = middle, 2 = `*Bottom`); near bottom only = `None`. `zone_rect` uses `area.w / 2`, `area.h / 2`, `area.h / 3`; right-side rects start at `area.x + area.w / 2` and take the remaining width so odd sizes leave no gap.
- [ ] **Step 4: Run** `cargo test -p opusdm-snap` all pass. `cargo clippy -p opusdm-snap -- -D warnings` clean.
- [ ] **Step 5: Commit** `git add Cargo.toml Cargo.lock crates/snap && git commit -m "feat(snap): zone geometry"`.

### Task 2: Restore geometry

**Files:** Create `crates/snap/src/restore.rs`; modify `lib.rs` (`pub mod restore;`).

**Interfaces:**
- Consumes: `Rect`.
- Produces: `pub fn restore_rect(stored: Rect, snapped: Rect, press: (i32, i32), pointer: (i32, i32), area: Rect) -> Rect`.

- [ ] **Step 1: Failing tests.** Snapped `Left` rect `{0,30,960,1050}`, stored `{300,200,600,400}`, pointer at `(480, 40)`, press at `(480, 40)`: result width/height are 600x400, `x = 480 - 0.5*600 = 180`, `y = 40 - (press.y - snapped.y) = 30`. Pointer at the far left `(0,..)` clamps `x` to `area.x`; pointer near the right clamps so the window stays inside the area; `y` never goes above `area.y`.
- [ ] **Step 2: Run** fail. **Step 3: Implement** with ratio `(press.x - snapped.x) as f64 / snapped.w as f64`, `x = pointer.x - (ratio * stored.w)`, `y = pointer.y - (press.y - snapped.y)`, then clamp into `area`. **Step 4: Run** pass.
- [ ] **Step 5: Commit** `feat(snap): restore geometry on drag-away`.

### Task 3: Drag state machine

**Files:** Create `crates/snap/src/machine.rs`; modify `lib.rs`.

**Interfaces:**
- Consumes: `Rect`, `Zone`, `zone_at`, `zone_rect`, `restore_rect`, `RESTORE_THRESHOLD`.
- Produces: `pub type WindowId = u32;` `pub struct Sample { pub button: bool, pub pointer: (i32, i32), pub window: Option<(WindowId, Rect)>, pub area: Rect }` (`window` is the FRAME rect of the active window, `None` when it is ignored), `pub enum Action { Zone(Option<Zone>), Place { window: WindowId, zone: Zone, rect: Rect }, Restore { window: WindowId, rect: Rect } }`, `pub struct Machine`, `Machine::new()`, `Machine::step(&mut self, sample: &Sample) -> Vec<Action>`, `Machine::forget(&mut self, window: WindowId)`.

States: `Idle`; `Pressed { window, start: Rect, press: (i32,i32) }`; `Dragging { window, zone: Option<Zone>, restored: bool }`. `Machine` also holds `snapped: HashMap<WindowId, Snapped { pre: Rect, at: Rect }>`.

Rules (each one gets a test first):
1. `button` false in `Idle` -> no actions.
2. Press: `Idle -> Pressed` with the sample window; if the window is `None` stay `Idle`. If the active window id changes while pressed, re-baseline.
3. `Pressed -> Dragging` when the window size equals `start` size, the position differs by 3 px or more (a move, not a resize). A press with no movement, or a size change, never reaches `Dragging` (rubber band, resize).
4. Restore: on that same transition, if the window is in `snapped`, `start` equals `at` (2 px tolerance) and the pointer moved more than `RESTORE_THRESHOLD` from `press`, emit `Restore { window, rect: restore_rect(pre, at, press, pointer, area) }`, drop the entry, `restored = true`. If the entry no longer matches `at`, drop it and do nothing.
5. `Dragging`: every step compute `zone_at(pointer, area)`; when it differs from the last, emit `Action::Zone(new)`.
6. Release in `Dragging` with `zone = Some(z)`: emit `Zone(None)` then `Place { window, zone: z, rect: zone_rect(z, area) }`; store `snapped[window] = Snapped { pre, at: rect }` where `pre` is the existing entry's `pre` if the window was already snapped (re-snap keeps the original), else the window rect at the drag start, or the restored size when a restore just happened. Release without zone: emit `Zone(None)` only if one was shown, go `Idle`.
7. `forget(window)` removes the entry (called when the window is closed).

- [ ] **Step 1: Write one test per rule** driving `step` with hand-built `Sample`s (rubber band: button down, window rect constant, pointer moves 200 px -> no actions; resize: size changes -> no actions; drag to the left edge and release -> `[Zone(Some(Left)), Zone(None), Place{..}]` in order; second snap keeps first `pre`; drag away from snapped -> `Restore` with size of `pre`; stale entry (rect differs) -> no `Restore`).
- [ ] **Step 2: Run** fail. **Step 3: Implement.** **Step 4: Run** pass + clippy.
- [ ] **Step 5: Commit** `feat(snap): drag state machine`.

### Task 4: Live spike, Openbox and restore mid-drag (decides the restore strategy)

**Files:** Create `crates/snap/examples/spike_restore.rs` (throwaway, deleted at the end of the task).

- [ ] **Step 1:** Start the sandbox: `Xephyr :9 -ac -screen 1280x800 &`, then `DISPLAY=:9 openbox --config-file openbox-rc.xml &` and `DISPLAY=:9 xterm &`. The example drives the pointer with the XTEST extension (`x11rb::protocol::xtest::fake_input`): press on the xterm titlebar, move 100 px, call `_NET_MOVERESIZE_WINDOW` to a smaller size under the pointer, move the pointer another 50 px, read the window geometry.
- [ ] **Step 2:** Record in the task result whether the window follows the pointer correctly (strategy 0, size and position set mid-drag), jumps (try strategy 1: send `_NET_WM_MOVERESIZE` move after the resize), or neither (strategy 2: restore on release).
- [ ] **Step 3:** Write the outcome as the "Restore strategy" paragraph at the end of this plan, delete the example, commit `docs: restore strategy decided by the Openbox spike`. Task 7 implements the chosen one.

### Task 5: X11 layer, overlay and watcher

**Files:** Create `crates/snap/src/x11.rs`, `crates/snap/src/overlay.rs`, `crates/snap/src/watcher.rs`; modify `lib.rs`, `Cargo.toml` of the crate (`x11rb = { workspace = true, features = ["randr"] }`, `opusdm-ipc` for `err_log!`).

**Interfaces:**
- Consumes: `Machine`, `Sample`, `Action`.
- Produces: `pub enum Placement { Place { window: u32, zone: Zone, rect: Rect }, Restore { window: u32, rect: Rect } }`, `pub struct Watcher`, `Watcher::start(on_placement: Box<dyn Fn(Placement) + Send>) -> Option<Watcher>`, `Watcher::set_enabled(&self, bool)`; `overlay::render_rect(w: u32, h: u32, radius: u32, border: u32, fill: [u8; 4], line: [u8; 4], scale: f32) -> Vec<u8>` (premultiplied BGRA); in `x11.rs`: `fn frame_rect(...)`, `fn is_snappable(...)`, `fn monitors_and_workarea(...)` (RandR monitors intersected with `_NET_WORKAREA`; with no RandR a single monitor equal to the root).

- [ ] **Step 1: Failing unit tests** for the pure parts: `render_rect` (size is `w*h*4`, the centre pixel is the fill colour, a corner pixel is transparent, the border pixels use `line`, `scale = 0.0` gives all zero), `x11::intersect_workarea` (monitor rect with workarea of a top panel), `x11::frame_from_client` (client geometry plus `_NET_FRAME_EXTENTS` gives the frame rect).
- [ ] **Step 2: Run** fail. **Step 3: Implement** the pure parts, then the X code: sampling reads `QueryPointer` (button 1 is `KeyButMask::BUTTON1`), `_NET_ACTIVE_WINDOW`, client geometry via `translate_coordinates` to the root, `_NET_FRAME_EXTENTS`, `_NET_WM_WINDOW_TYPE`, `WM_NORMAL_HINTS`. The overlay is an override-redirect window on a 32-bit ARGB visual with its own colormap, painted with `put_image` (ZPixmap, depth 32); `tick` interpolates rect and alpha over 120 ms and repaints only when something changed; if no compositor owns `_NET_WM_CM_S{n}` it is never mapped (one log line). The watcher thread sleeps on a condition while the button is up (`QueryPointer` every 50 ms for the button only) and polls every 16 ms while it is down. A lost connection retries with 1, 2, 4, 8, 30 s backoff and logs each failure; missing RandR falls back to the root rect.
- [ ] **Step 4: Run** the tests, `cargo clippy -p opusdm-snap -- -D warnings`.
- [ ] **Step 5: Commit** `feat(snap): X11 sampling, overlay and watcher`.

### Task 6: Setting, hub integration and UI toggle

**Files:**
- Modify: `crates/ipc/src/desktop.rs` (field `snap_enabled: bool`, default `true`, test for old files), `crates/hub/Cargo.toml` (`opusdm-snap`), `crates/hub/src/lib.rs`, `crates/hub/src/main.rs` or `toolbar.rs` (start the watcher in `build_ui` next to `apply_virtual_screens`, and on `DesktopSettingsChanged` call `set_enabled`), `crates/desktop-config/src/ui.rs` (`WindowsSection` gets a "Snap windows to the screen edges" check button, getter `snap_enabled()`), `crates/theme-config/src/main.rs` (pass and save the value).
- Create: `crates/hub/src/snap.rs`.

**Interfaces:**
- Consumes: `Watcher::start`, `Placement`, `opusdm_ui::with_x11`, `X11::move_resize`, `X11::add_wm_states`, `X11::send_to_root`.
- Produces: `pub fn start(enabled: bool) -> Option<Watcher>`, `pub fn client_geometry(rect: Rect, extents: (i32, i32, i32, i32)) -> (i32, i32, u32, u32)` (frame rect to the client geometry `_NET_MOVERESIZE_WINDOW` expects, never below 1x1).

- [ ] **Step 1: Failing tests:** `desktop.rs` (default is `true`, `{"icon_size":64}` loads with `true`, `false` roundtrips), `hub::snap::client_geometry` (a 960x1050 frame with extents `(1,1,24,1)` gives a 958x1025 client at the frame origin), `WindowsSection` seeded value is reported back (guarded by `gtk::init()` like the existing tests).
- [ ] **Step 2: Run** fail. **Step 3: Implement.** `Placement::Place` with `Zone::Maximize` sends `_NET_WM_STATE_ADD` for `_NET_WM_STATE_MAXIMIZED_VERT` and `_HORZ`; any other zone first removes both maximized states, then `move_resize`. `Placement::Restore` follows the strategy decided by Task 4 (see the end of this plan). Errors are logged with the window id and ignored.
- [ ] **Step 4: Run** `cargo test --workspace` and `cargo clippy --workspace -- -D warnings`.
- [ ] **Step 5: Commit** `feat(snap): hub integration and Windows setting`.

### Task 7: Live verification, docs and changelog

**Files:** Modify `CHANGES.md`, `docs/usage.md`, `FEATURES.md` (only if it lists snapping); scripts untouched.

- [ ] **Step 1:** Run `./run-x11.sh :9`, drive real drags with XTEST (extend a throwaway example, not committed) to every zone, drag away from a snapped window, snap an xterm and the lister, snap with panel and dock present, toggle the setting off. Record what was seen and what was NOT checked.
- [ ] **Step 2:** `CHANGES.md` entry (dated 2026-10-07), `docs/usage.md` paragraph in the Windows tab section, including the known limits: the per-monitor work area is the intersection of the monitor with `_NET_WORKAREA`, no overlay without a compositor.
- [ ] **Step 3: Commit** `docs(snap): changelog and usage`.

## Restore strategy

Decided by the Task 4 spike, run on a nested Xephyr with Openbox and an xterm, pointer driven with XTEST:

- Resizing the window in the middle of Openbox's move loop does NOT work: the next pointer motion makes Openbox reapply its cached size and the original position plus the pointer delta. A `_NET_WM_MOVERESIZE` sent while the loop runs is ignored.
- Restoring at button release works, but is not the KDE behaviour.
- **Chosen: cancel and restart the move.** On `Action::Restore`: (1) send an `Escape` key press and release through XTEST (Openbox cancels its move loop and puts the window back where the drag started), (2) `_NET_MOVERESIZE_WINDOW` to the restored frame rectangle, (3) `_NET_WM_MOVERESIZE` with the pointer position, direction 8 (move), button 1, source 1, which starts a fresh move loop that follows the pointer. Measured: the window followed the pointer exactly (expected client position (364,353), got (364,353)).
- Cost: the XTEST extension (`x11rb` feature `xtest`), and about 300 ms of steps with sleeps in the placement code, run off the GTK main loop.
- NOT verified: the same sequence with a physical mouse. The spike held the button down with XTEST, a real device is a different slave device. To be checked by the user on the real session, with fallback strategy 2 (restore at release) selectable with the constant `RESTORE_DURING_DRAG` in `crates/hub/src/snap.rs`.
