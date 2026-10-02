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

- Window type `_NET_WM_WINDOW_TYPE_NORMAL` (deliberately not `DOCK`: that type tells the window manager to always keep the window on top, which no `_NET_WM_STATE_BELOW` can override) with `_NET_WM_STATE_BELOW`, `SKIP_TASKBAR` and `SKIP_PAGER`.
- **No strut**: the dock does NOT reserve screen space. Maximized windows cover it; it is visible only over the desktop.
- Opaque background with a thin border and square corners: rounded corners would need transparency or an X shape mask, so they are left out. No compositor required.
- Centered along its edge inside the monitor work area.
- The geometry is pinned with `_NET_MOVERESIZE_WINDOW` (openbox ignores the position of a plain `ConfigureWindow` once it has adopted the window), and the states are re-sent a few times over the first second after map, since openbox drops state messages for windows it has not adopted yet.

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
- New "Dock" tab in `opusdm-settings` (position, icon size, pinned apps list), applied live.

## Implementation status

All five steps below are implemented.

1. Crate skeleton, dock window (`_NET_WM_WINDOW_TYPE_DOCK` + `_NET_WM_STATE_BELOW`, no strut), placement per edge, static icon row.
2. `dock.json` loading (`opusdm_ipc::DockConfig`), `gio::DesktopAppInfo` launch, tooltips, busy cursor. A missing `dock.json` is seeded with the installed defaults (file manager, terminal, browser) and written out.
3. `opusdm_ipc::DockPosition`/`DesktopSettings::{dock_position, dock_icon_size}`, `opusdm_dock::attach` returns a setter the hub calls on `DesktopSettingsChanged`; the dock never shares the panel's edge (`DockPosition::avoiding`).
4. Drag and drop: reorder within the dock, drop a `.desktop` file to pin it, drag an icon out to unpin it. Context menu: an icon's desktop actions plus "Remove from Dock"; the background's "Add Application...".
5. "Dock" tab in `opusdm-settings` (`opusdm_desktop_config::dock_ui`): edge (only the ones free of the panel), icon size, and the pinned-apps list with up/down/remove/add. The Panel tab's edge change updates the Dock tab's offered edges live.
