# opusdm-screen: multi-monitor manager design

Date: 2026-10-08
Status: implemented (see the notes at the end for what changed while building)

## Goal

A GTK4 app, similar to KDE Plasma "Display Configuration", to manage multiple
monitors on the X11 session: arrange them on a canvas, enable or disable them,
pick resolution, refresh rate, rotation, scale and the primary monitor, and
switch between extend, mirror (duplicate) and single-screen modes. Layouts are
saved as profiles per set of connected monitors and re-applied automatically
when monitors are plugged or unplugged.

## Decisions taken

- X11 only (no Wayland). Backend is the `xrandr` CLI, same style as
  `opusdm-input-config` which wraps `xinput` and `setxkbmap`.
- Hotplug detection uses x11rb RandR events, living in the `hub` (the daemon
  that already owns long-running session duties).
- Persistence: profiles keyed by the set of connected monitors.
- Applying a layout asks "Keep these settings?" and reverts after 15 seconds
  without confirmation, as KDE does.

## Out of scope (YAGNI)

Wayland, gamma and color management, night light, fractional per-monitor scale
beyond what `xrandr --scale` offers, per-monitor wallpaper.

## Crate layout

New crate `crates/screen`, package `opusdm-screen`, lib `opusdm_screen` and bin
`opusdm-screen`. Mirrors `crates/input-config`. Added to workspace members.

| File | Responsibility |
|------|----------------|
| `model.rs` | Plain data types, no I/O |
| `parse.rs` | Parse `xrandr --query --props` text into `Monitor` values |
| `commands.rs` | Build the single `xrandr` argument list for a `Layout` |
| `modes.rs` | Pure functions producing a `Layout`: extend, mirror, only |
| `backend.rs` | `ScreenBackend` trait and `XrandrBackend` |
| `fake.rs` | `FakeBackend` for tests and UI-less runs |
| `store.rs` | Profile persistence (JSON) and lookup by monitor set |
| `session.rs` | Edit state, apply with revert timer, rollback |
| `ui.rs` | GTK4 canvas, property panel, mode buttons, apply bar |
| `error.rs` | `ScreenError` (thiserror) |
| `main.rs` | Entry point |

## Data model

```rust
pub struct Mode { pub width: u32, pub height: u32, pub rates: Vec<f64> }

pub struct Monitor {
    pub name: String,          // xrandr output name, e.g. "HDMI-1"
    pub edid_id: String,       // stable id: hash of EDID, fallback to name
    pub modes: Vec<Mode>,      // sorted largest first
    pub preferred: Option<(u32, u32)>,
}

pub struct MonitorConfig {
    pub name: String,
    pub edid_id: String,
    pub enabled: bool,
    pub mode: (u32, u32),
    pub rate: Option<f64>,
    pub pos: (i32, i32),
    pub rotation: Rotation,    // Normal, Left, Right, Inverted
    pub scale: f64,            // xrandr --scale factor, 1.0 default
    pub primary: bool,
}

pub struct Layout { pub monitors: Vec<MonitorConfig> }
```

`Parsed` (what `query` returns) holds the connected monitors only; a
disconnected output is dropped by the parser. Duplicated screens have no field:
a layout is mirrored when several enabled monitors share the same `pos`
(`Layout::is_mirrored`).

`Layout` validation (fail fast, descriptive `ScreenError`): at least one
enabled monitor, at most one primary, mode and rate must exist in the monitor's
modes, scale must be positive.

## Backend

```rust
pub trait ScreenBackend {
    fn query(&self) -> Result<(Vec<Monitor>, Layout), ScreenError>; // hardware + current layout
    fn apply(&self, layout: &Layout) -> Result<(), ScreenError>;
}
```

- `XrandrBackend::query` runs `xrandr --query --props`; `parse.rs` is a pure
  function over the text so it is tested with captured fixtures.
- `apply` builds one `xrandr` invocation covering all outputs (`--output X
  --mode WxH --rate R --pos XxY --rotate normal|left|right|inverted --scale S
  [--primary] [--same-as Y] | --off`), so the change is atomic. Output goes
  through `commands.rs`, a pure function tested by comparing argument lists.
- Non-zero exit or stderr from `xrandr` becomes `ScreenError::Backend` with the
  command line and stderr as context. Nothing is swallowed.
- `FakeBackend` keeps a layout in memory and records `apply` calls.

## Modes (extend / mirror / only)

Pure functions on `(&[Monitor], &Layout) -> Layout`:

- `extend`: enable all connected monitors at preferred mode, placed left to
  right in current x order, tops aligned, first becomes primary if none is.
- `mirror`: enable all connected monitors at the largest `(w, h)` common to
  all of them, all at `pos (0,0)`, `mirror_of` pointing to the primary.
  If no common mode exists, return `ScreenError::NoCommonMode` and the UI shows
  it; scaling to fit is out of scope.
- `only(name)`: that monitor on, every other off, becomes primary.

## Profiles (store)

File `~/.config/opusdm/screens.json`:

```json
{ "profiles": [ { "key": ["edid-a", "edid-b"], "layout": { ... } } ] }
```

- Key is the sorted list of `edid_id` of connected monitors.
- `save(key, layout)` replaces the profile with the same key.
- `find(connected) -> Option<Layout>`. A saved layout is only returned if every
  output and mode in it still exists (validated against current `Monitor`s).
- Load errors (corrupt JSON) are reported, the file is not overwritten
  silently.
- Atomic write: temp file then rename.

## Session

`Session` holds the monitors, the committed layout and the pending (edited)
layout. Operations: `edit(Change)`, `apply()`, `confirm()`, `revert()`,
`save_profile()`.

- `apply()` stores the previous committed layout, calls the backend, and marks
  the state `Pending`. The UI starts a 15 s timer.
- `confirm()` commits the new layout (and saves the profile if the user ticked
  "remember for these monitors").
- `revert()` (button, timeout or window close while pending) re-applies the
  previous layout. If that revert fails, the error is surfaced, never ignored.
- The timer lives in the UI; `Session` only exposes the state, so it is
  testable with `FakeBackend` without GTK.

## UI

- Canvas: one rectangle per monitor, scaled to fit, label with name and
  resolution. Drag to move, snap to neighbouring edges (within 10 px canvas
  units) so no gaps or overlaps result. Disabled monitors drawn greyed.
- Panel (selected monitor): enabled switch, resolution and refresh rate
  dropdowns, rotation, scale, "primary" toggle.
- Top bar: Extend, Mirror, "Only <monitor>" buttons.
- Bottom bar: Apply, Reset, "Remember for these monitors" check.
- Revert dialog: "Keep these settings? Reverting in N s" with Keep and Revert.
- Follows `opusdm-ui` theme helpers like `input-config`.

## Hub integration

- `hub/src/desktop_menu.rs`: new "Screens" launcher entry next to "Input"
  (`launch_tool("opusdm-screen")`).
- `hub`: new `screen_hotplug.rs` module. It subscribes to RandR
  `ScreenChangeNotify` through x11rb, debounces 500 ms, then calls the screen
  crate library: `query`, `store.find(connected)`, and if a profile exists
  `apply` it. No profile means do nothing (never guess a layout). Errors are
  logged with context.
- Sibling binary lookup in `children.rs` already handles `opusdm-*` names.

## Testing

- `parse`: fixtures from real `xrandr --query --props` output (single monitor,
  dual, one disconnected, rotated, with and without EDID).
- `commands`: layout to argument list, including off, mirror, rotation, scale,
  primary.
- `modes`: extend ordering, mirror common mode and failure, only.
- `store`: round trip, replace same key, stale profile rejected, corrupt file.
- `session`: apply then confirm, apply then revert, revert failure, with
  `FakeBackend`.
- `hub` hotplug: debounce and profile selection tested against the fake
  backend; the X event subscription itself is covered by a manual check.
- UI is verified manually (canvas drag and snap).

## Build order (for the plan)

1. Crate skeleton, `model`, `error`.
2. `parse` with fixtures.
3. `commands`, `backend`, `fake`.
4. `modes`.
5. `store`.
6. `session`.
7. `ui` and `main`.
8. Hub: menu entry, then hotplug module.
9. `CHANGES.md` and `docs/usage.md` updates.

## Open risks

- `xrandr` EDID parsing: some outputs expose no EDID, so `edid_id` falls back
  to the output name, which makes profiles less stable across ports.
- Mirror with no common mode is rejected rather than scaled.

## Changes while building

- No `connected` and no `mirror_of` field (see Data model): the parser drops
  disconnected outputs and mirror is "same position".
- `MonitorConfig::scale` is the raw `xrandr --scale` factor (2.0 shrinks
  things), and the window explains it in a tooltip.
- `xrandr_args(layout, current)` also takes the current layout: `--scale` is
  only sent when the monitor is scaled now or will be, because a server
  without transform support refuses `--scale 1x1`. `XrandrBackend::apply`
  queries the current layout itself.
- `Session::apply` moves the layout so the top left enabled monitor is at 0,0.
- New `geometry.rs` (edge snapping, canvas view) and `reapply.rs` (the hub
  decision) in the library, both tested without GTK.
- Hotplug applies a profile only when the set of connected monitors differs
  from the last one seen, so a manual change with the same monitors (or an
  Apply still waiting for Keep) is not undone. It also runs once at hub start.
- The Keep question is a bar in the main window, not a dialog, so the
  countdown can be shown and the window can still be closed to revert.
- `opusdm-screen --demo` uses the fake backend.
- Not done: scaling a picture to fit when Duplicate finds no common
  resolution (still rejected), an internal hub command for the window.
