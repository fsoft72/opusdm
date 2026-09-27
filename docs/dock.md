# opusdm-dock

A simple application launcher dock, in the spirit of the macOS Dock, but launcher only.

## Scope

In scope:

- Row (or column) of application launcher icons.
- Position on one screen edge, configurable.
- Icon size configurable.
- Pinned app list edited by drag and drop, context menu and the Settings tool.

Out of scope: zoom, transparency, window list, running-app indicators, previews, minimized windows, folder stacks, auto-hide, trash, Wayland.

## Architecture

- New crate `crates/dock`, binary `opusdm-dock`, spawned by the hub.
- Exposes `opusdm_dock::attach(app, settings)` following the pattern of `opusdm_panel::attach`, and returns a setter so the hub can re-dock it live on `DesktopSettingsChanged`.
- Launching uses `gio::AppInfo` / `gtk::AppLaunchContext` (honours `Exec` field codes, `Terminal=true`, startup notification). No custom launcher.
- Icons come from the installed icon theme, like everywhere else in OpusDM (no font dependency).

## Window behaviour

- Window type `_NET_WM_WINDOW_TYPE_DOCK` with `_NET_WM_STATE_BELOW`.
- **No strut**: the dock does NOT reserve screen space. Maximized windows cover it; it is visible only over the desktop.
- Opaque background with rounded corners (CSS). No compositor required.
- Centered along its edge inside the monitor work area.

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

- `~/.config/opusdm/dock.json`: pinned app ids (desktop file ids), in order.
- Position and icon size live in `desktop.json` (`DockSettings`, next to `PanelPosition`).
- Missing `dock.json`: start with defaults (file manager, terminal, web browser), keeping only those installed.
- New "Dock" tab in `opusdm-theme` (position, icon size, pinned apps list), applied live.

## Implementation plan

1. Crate skeleton, dock window (type, layer, no strut), placement per edge, static icon row from a hard-coded list.
2. `dock.json` loading, `gio::AppInfo` launch, tooltips, busy cursor.
3. `DockSettings` in `opusdm-ipc`, hub spawn and live re-dock on settings change, panel-edge exclusion.
4. Drag and drop (reorder, add, remove) and context menu.
5. "Dock" tab in `opusdm-theme`.
