# opusdm-dock

A simple application launcher dock, in the spirit of the macOS Dock, but launcher only.

## Scope

In scope:

- Row (or column) of application launcher icons.
- Position on one screen edge, configurable.
- Icon size configurable.
- Pinned app list edited by drag and drop, context menu and the dock's own Settings window.
- Look: background opacity (0-100 %, the icons stay solid), background image (fill or tile), macOS style icon zoom on mouse over, shown or hidden.

Out of scope: window list, running-app indicators, previews, minimized windows, folder stacks, auto-hide, trash, Wayland.

## Architecture

- Crate `crates/dock`, binary `opusdm-dock`: its own process (since 2026-11-08), so a stuck hub main loop cannot freeze the dock, the dock can be restarted or updated alone, and a dock turned off costs no memory.
- `standalone.rs` runs it: reads `dock.json` (all its settings), `desktop.json` (only the panel edge) and `settings.json` (only the theme mode) and follows the three files with file monitors. It never asks the hub anything.
- Exit codes: `EXIT_DISABLED` (4) when `dock.json` turns it off (at start, before opening a display, or later), `EXIT_ALREADY_RUNNING` (3) when another dock owns the socket.
- Socket `dock.sock` next to the hub socket (`OPUSDM_DOCK_SOCK` overrides it), single instance (`instance.rs`), protocol `opusdm_ipc::dock::{DockRequest, DockReply}`: `Ping` and `SetHidden(bool)`, one reply each, 1 s client timeout. Thread `opusdm-dock-ipc` (`server.rs`) hands the requests to the GTK main loop and answers within 800 ms.
- Hub side, `crates/hub/src/dock_host.rs`: thread `opusdm-dock-supervisor` starts the dock while `dock.json` says `visible`, parks on `EXIT_DISABLED` until the hub's watch on `dock.json` wakes it, retries every 30 s on `EXIT_ALREADY_RUNNING`, restarts a crashed dock after 1 / 2 / 4 s (at most 3 times in 60 s, then waits for the next change of `dock.json`). Thread `opusdm-dock-link` sends "Hide All" / "Reveal" (`SetHidden`), also to a freshly started dock. The hub GTK main loop never talks to the dock socket.
- `opusdm_dock::attach(app, layout)` still builds the dock window in the process and returns a setter that applies a new layout live.
- Launching uses `gio::AppInfo` / `gtk::AppLaunchContext` (honours `Exec` field codes, `Terminal=true`, startup notification). No custom launcher.
- Icons come from the installed icon theme, like everywhere else in OpusDM (no font dependency).

## Window behaviour

- Window type `_NET_WM_WINDOW_TYPE_NORMAL` (deliberately not `DOCK`: that type tells the window manager to always keep the window on top, which no `_NET_WM_STATE_BELOW` can override) with `_NET_WM_STATE_BELOW`, `SKIP_TASKBAR` and `SKIP_PAGER`.
- **No strut**: the dock does NOT reserve screen space. Maximized windows cover it; it is visible only over the desktop.
- The window holds a `gtk::Fixed`: first the strip (`.opusdm-dock`, the background colour, the 1 px border, the optional image, faded as a whole by the opacity), then one `DockIcon` per app placed by `zoom.rs`. Square corners.
- With the classic look (opacity 100, no zoom) the window is exactly the strip, pixel for pixel the old dock, with the picom shadow as before. No compositor required.
- See-through (opacity below 100 or zoom) needs a compositor: the dock checks the `_NET_WM_CM_S<n>` owner at start and every 2 s (GTK does not report a compositor that stops or starts later) and, without one, draws the classic opaque strip and turns the zoom off (logged). The window sets `_NET_WM_WINDOW_OPACITY` to opaque (the global inactive opacity never fades the icons) and, when see-through, `_COMPTON_SHADOW = 0` on the client and on the openbox frame (picom reads the frame); the frame is found again after every map.
- Centered along its edge inside the monitor work area.
- The geometry is pinned with `_NET_MOVERESIZE_WINDOW` (openbox ignores the position of a plain `ConfigureWindow` once it has adopted the window), and the states are re-sent a few times over the first second after map, since openbox drops state messages for windows it has not adopted yet.

## Zoom

- The icon under the pointer reaches the zoom size (110-250 %, default 160 %), its neighbours less on a cosine bell 3 slots wide on each side. The row grows around the pointer: the point of the row under the pointer stays under it, so the aimed icon does not slide away (`zoom.rs`, unit tested).
- The window has room for the biggest zoom: headroom on the side away from the screen edge, and on both ends of the strip the most the row can grow. That room is transparent and, at rest, out of the X input region (only the strip takes the pointer), so it never steals clicks from the desktop. Entering the strip starts the zoom and opens the whole window to input; leaving the strip and the grown icons ends it.
- Each icon paintable is looked up once at the biggest size; an animation frame only changes sizes and positions. Animation on the GTK frame clock (tick callback while something moves, stopped once every icon settles), exponential ease with a 70 ms time constant. Measured in the Xephyr sandbox: under 0.5 ms of dock work per frame.
- A drag of an icon ends the zoom, so a drop lands on the row at rest (`Placement::insert_index` skips the zoom room). The CSS hover highlight is off with the zoom (the size is the feedback; the highlight would stay on an icon that slid away under a still pointer).

## Position

- Edges: `bottom` (default), `top`, `left`, `right`.
- The dock and the panel never share an edge. The Settings UI offers only the edges the panel does not use; when the panel is moved onto the dock's edge, the dock jumps to the opposite edge.
- For a left/right dock the usable range excludes the panel band, so the dock never overlaps the panel area.
- Multi-head: dock on the primary monitor.

## Icons

- Click: launch the app; tooltip shows the app name.
- Launch feedback: busy cursor while the app starts (no window tracking).
- Reorder by drag and drop inside the dock; dragging an icon out of the dock removes it.
- Add an app by dropping a `.desktop` file on the dock, or from the context menu.
- Context menu (right click): "Remove from Dock" plus the desktop actions (`Actions=`) declared by the app.
- Icon sizes: 32, 48, 64 (default 48).

## Configuration

- `~/.config/opusdm/dock.json` (`opusdm_ipc::dock::DockConfig`) holds every dock setting: `apps` (pinned desktop file ids, in order), `position`, `icon_size`, `visible`, `style` (`DockStyle`: `opacity`, `image`, `image_mode` fill or tile, `zoom_enabled`, `zoom_percent`) and `layout_migrated`. Missing fields keep the classic look.
- The layout used to live in `desktop.json` (`dock_position`, `dock_icon_size`, `dock_visible`, `dock_style`): the first load copies those keys once; `desktop.json` no longer has them.
- Every writer goes through `update_dock_config` (read fresh, change its own fields, atomic save): the dock (drag and drop, Settings window), the "Show the dock" box of `opusdm-settings`, the hub desktop menu. A malformed `dock.json` is logged and never overwritten.
- `visible`: off ends the dock process; on (from `opusdm-settings`, the desktop menu "Show Dock", or the file) makes the hub start it. The hub's "Hide All" / "Reveal" is separate (`SetHidden` over the socket), so "Reveal" never brings back a dock that is turned off.
- Missing `apps`: start with defaults (file manager, terminal, web browser), keeping only those installed.
- Settings: right click on the dock background, "Settings..." opens "Dock Settings" (`settings_window.rs`, sections in `settings_ui.rs`): edge, icon size, look, "Show the dock", pinned apps; Cancel / Apply. `opusdm-settings` only keeps "Show the dock" under PANEL.

## Implementation status

All six steps below are implemented.

1. Crate skeleton, dock window (`_NET_WM_WINDOW_TYPE_DOCK` + `_NET_WM_STATE_BELOW`, no strut), placement per edge, static icon row.
2. `dock.json` loading (`opusdm_ipc::DockConfig`), `gio::DesktopAppInfo` launch, tooltips, busy cursor. A missing `dock.json` is seeded with the installed defaults (file manager, terminal, browser) and written out.
3. `opusdm_ipc::DockPosition`/`DesktopSettings::{dock_position, dock_icon_size}`, `opusdm_dock::attach` returns a setter the hub calls on `DesktopSettingsChanged`; the dock never shares the panel's edge (`DockPosition::avoiding`).
4. Drag and drop: reorder within the dock, drop a `.desktop` file to pin it, drag an icon out to unpin it. Context menu: an icon's desktop actions plus "Remove from Dock"; the background's "Add Application...".
5. "Dock" tab in `opusdm-settings` (`opusdm_desktop_config::dock_ui`): edge (only the ones free of the panel), icon size, and the pinned-apps list with up/down/remove/add. The Panel tab's edge change updates the Dock tab's offered edges live.
6. Look: opacity, background image, icon zoom and the "Show the dock" checkbox, in `desktop.json` and in the Dock tab (`DockLookSection`).
7. Separate process (2026-11-08, spec `docs/superpowers/specs/2026-11-08-dock-process-design.md`): settings moved to `dock.json`, dock socket, hub supervisor and link, Settings window in the dock, Dock tab removed from `opusdm-settings`, "Show Dock" / "Hide Dock" in the desktop menu. Steps 3, 5 and 6 above describe the old layout.
