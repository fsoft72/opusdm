# Window snap (KDE-style edge tiling) - design

Date: 2026-10-07

## Goal

When a window is dragged towards the screen edges, an overlay shows where the
window will land (as in KDE Quick Tile). Releasing the button inside a zone
places the window there. Dragging a snapped window away restores its previous
size, as KDE does.

Applies to **all X11 windows**, not only OpusDM ones. The window manager is
Openbox, which owns the drag, so the feature lives outside it, in the hub.

## Zone map

Zones are active when the pointer is within `EDGE_THRESHOLD` (20 px) of a
monitor edge, computed on the monitor under the pointer, inside its work area
(`_NET_WORKAREA`, so panel and dock are excluded).

| Zone | Pointer position | Resulting rect |
|------|------------------|----------------|
| `Maximize` | top edge (between the two top corners) | window maximized |
| `TopLeft` / `TopRight` / `BottomLeft` / `BottomRight` | corners | 1/2 width x 1/2 height |
| `Left` / `Right` | middle third of the side edge | 1/2 width x full height |
| `LeftTop` / `RightTop` | upper third of the side edge | 1/2 width x 1/3 height, at the top |
| `LeftBottom` / `RightBottom` | lower third of the side edge | 1/2 width x 1/3 height, at the bottom |

The bottom edge between the corners has no zone (as in KDE). The corner zones
win over the side thirds.

## Components

New library crate `crates/snap` (`opusdm-snap`), three units:

1. **`zones`** (pure, unit tested): `Zone`, `zone_at(point, work_area) ->
   Option<Zone>`, `zone_rect(zone, work_area) -> Rect`. All the geometry lives
   here.
2. **`watcher`**: thread with its own `x11rb` connection. State machine
   `Idle -> Dragging -> Armed(zone)`. A drag is detected by polling
   `QueryPointer` (~16 ms) only while the left button is down: it is a window
   drag when the geometry of the toplevel under the pointer changes while the
   button is held. Emits `ZoneChanged(Option<Zone>)` and `Released(window,
   Zone)` on a channel. Ignores windows of type dock, desktop and splash, and
   windows that cannot be resized (min and max size hints equal).
3. **`overlay`**: override-redirect ARGB X11 window (composited by picom) that
   draws a translucent rounded rectangle in the accent colour. Fades in and
   animates the rect between zones. Draws only, no logic.

Hub integration: `crates/hub/src/snap.rs` starts the watcher at startup (same
pattern as `global_keys.rs`) and on `Released` places the window with
`opusdm_ui::x11` `move_resize`, or with `_NET_WM_STATE_MAXIMIZED_*` for
`Maximize`. Any existing maximized state is cleared first, so Openbox does not
refuse the new geometry.

## Restore on drag-away (KDE behaviour)

- On snap, the watcher stores the **pre-snap geometry** of the window in a map
  keyed by window id.
- When a drag starts on a window that is in the map and whose current geometry
  still equals the snapped rect, and the pointer has moved more than
  `RESTORE_THRESHOLD` (8 px), the window is restored to its stored size. The
  pointer keeps the same **proportional horizontal position** inside the window
  and the same vertical offset from the title bar, so the window follows the
  cursor.
- The entry is dropped when the restore happens, when the window is closed, or
  when its geometry no longer matches the snapped rect (moved or resized by
  other means).
- Re-snapping an already snapped window keeps the **original** pre-snap
  geometry, not the intermediate snapped one.

**Known risk:** resizing a window while Openbox is in the middle of its own
move loop. Openbox applies pointer deltas to the position captured at the drag
start, so the window may jump back on the next motion. The first task of the
plan is a live spike in Xephyr. Fallbacks, in order: (1) after restoring, send
`_NET_WM_MOVERESIZE` (move) to start a fresh drag at the current pointer;
(2) restore the size at button release instead of during the drag. The chosen
option is recorded in `CHANGES.md`.

## Multi-monitor

The zone is computed on the monitor under the pointer using the RandR monitor
list (`opusdm_ui::screen`) and the per-monitor part of `_NET_WORKAREA`. The
overlay appears on that monitor only.

## Settings

`snap_enabled` (default `true`) in `~/.config/opusdm/`, with a toggle in the
existing desktop settings, pushed live like the other settings. `EDGE_THRESHOLD`
and `RESTORE_THRESHOLD` are constants, not settings.

## Error handling

- No RandR or no `_NET_WORKAREA`: the watcher logs a clear error and disables
  snap. It does not panic and does not fail silently.
- X connection lost: retry with backoff, as the network watcher does.
- A window that vanishes between release and placement: the failed request is
  logged with the window id and ignored.

## Testing

- Unit tests for `zones`: every edge, corner and third, the corner-over-side
  priority, thresholds, work area with panel offsets, multi-monitor offsets.
- Unit tests for the restore bookkeeping (pre-snap map, proportional pointer
  position, re-snap keeps the original geometry), written as pure functions.
- `watcher` and `overlay` need an X server and are verified live in the
  Xephyr sandbox (`run-x11.sh`): drag to each zone, drag away, drag a
  third-party window, snap with the panel and dock present.

## Out of scope (for now)

Shift-drag layout grid, custom layouts, keyboard shortcuts (Meta+arrows), top
edge thirds, Wayland (see `docs/WAYLAND-PORT.md`).

## Implementation notes (after the build)

- The Openbox spike decided the restore strategy: resizing mid-drag does not work, so the move is
  cancelled with Escape (XTEST), the window is resized and `_NET_WM_MOVERESIZE` starts a new move.
- The crate places and restores windows itself (it needs XTEST and its own connection), so the hub
  only starts the watcher and switches it. `opusdm_ui::x11` is not used.
- The overlay is a window background plus four border rectangles, not a pixel buffer: no rounded
  corners, but it slides between zones at almost no cost.
- No RandR or no `_NET_WORKAREA`: falls back to the whole screen and keeps working, instead of
  disabling snap.
