# Wayland port: exploration notes

Status: **implemented** (2026-10-10) for wlroots compositors, labwc first.
See "Decisions", "Implementation plan" and "What works where" at the end;
the analysis below is from 2026-10-05.

## Summary

- Difficulty: **medium-high**. Days of work, not hours.
- The size of the code to touch is small (about 2200 lines). The difficulty is
  that the most distinctive features (desktop window, panel, dock, tasklist,
  global hotkeys, absolute positioning) are outside the Wayland security
  model by design.
- Realistic target: a **wlroots-based compositor** (labwc, sway, river) or KDE.
  GNOME/Mutter cannot support the desktop, panel and tasklist features.

## What is already portable

- The whole UI is GTK4 (gtk4-rs), which runs natively on Wayland.
- Lister, `fsops`, `ftp`, `iconview`, themes, `ipc` and all ordinary windows
  do not depend on X11.
- There is already a soft degradation: every `downcast::<X11Surface>()` fails
  without panicking and the helpers become no-ops outside X11. On Wayland
  OpusDM would start, but lose features.
- The StatusNotifierItem tray (`panel/src/sni.rs`) is already implemented and
  is the right tray mechanism on Wayland.

## X11 code inventory

| File | Lines | Role |
|---|---|---|
| `crates/ui/src/x11.rs` | 232 | Shared EWMH helpers: geometry, `_NET_WM_STATE`, restack, all-desktops |
| `crates/ui/src/screen.rs` | 256 | Monitor geometry (RandR) |
| `crates/buttonbank/src/x11.rs` | 67 | Skip taskbar, all desktops, read/set window position |
| `crates/hub/src/desktop.rs` | 87 | Hub window turned into the WM desktop |
| `crates/hub/src/global_keys.rs` | 282 | System-global hotkeys via `XGrabKey` |
| `crates/panel/src/ewmh.rs` | 392 | EWMH observer: window list, desktops, activate, iconify |
| `crates/panel/src/systray.rs` | 419 | XEmbed system tray |
| `crates/panel/src/window.rs` | 200 | Panel window placement and struts |
| `crates/dock/src/{placement,window}.rs` | 266 | Dock placement |

Crates with X11 dependencies: `ui` (x11rb, randr), `hub` (x11rb, gdk4-x11),
`panel` (x11rb, gdk4-x11), `dock` (x11rb, gdk4-x11), `buttonbank` (x11rb,
gdk4-x11).

## What does not port directly

| Feature | Today (X11) | On Wayland |
|---|---|---|
| **Global hotkeys** (`global_keys.rs`) | Passive `XGrabKey` on the root window | No equivalent. Use the `org.freedesktop.portal.GlobalShortcuts` portal (GNOME and KDE yes, wlroots depends on the portal backend), or compositor-side bindings that run an IPC command. |
| **Desktop as a window** (`desktop.rs`: type DESKTOP, BELOW, sticky, monitor-sized) | EWMH hints | There is no desktop window type. Use `wlr-layer-shell` (BACKGROUND layer, via `gtk4-layer-shell`). Mutter does **not** support layer-shell: wlroots and KDE only. |
| **Panel and dock** (struts, edge anchoring) | `_NET_WM_STRUT` and absolute placement | `layer-shell` with an exclusive zone. Clean fit, same compositor limits. |
| **Tasklist and pager** (`ewmh.rs`) | `_NET_CLIENT_LIST`, client messages | A client cannot list or control other clients' windows. Needs `wlr-foreign-toplevel-management` (wlroots) or `ext-foreign-toplevel-list` plus compositor-specific protocols. Impossible on GNOME. |
| **Absolute positioning** (button banks, `move_to`, `position`, `pin_geometry`) | X coordinates | A client can neither place itself nor read its position. Banks with a saved position lose meaning unless they become layer-shell surfaces with margins. |
| **XEmbed tray** (`systray.rs`) | `_NET_SYSTEM_TRAY` selection | Does not exist. Keep SNI only. |
| **Sandbox and tests** (`run-x11.sh`, openbox, picom, Xephyr) | Nested X server | Needs a nested wlroots compositor (`sway` or `labwc` headless / nested) to test. |

## Compositor choice

- **Openbox is X11 only.** It has no Wayland backend and upstream is
  essentially dormant, so none is coming.
- Its Wayland counterpart is **labwc** (wlroots): same stacking model,
  Openbox-compatible themes (`themerc`) and an `rc.xml` close to Openbox's. The
  project's `openbox-rc.xml` should port with small edits (keybindings and
  window rules). labwc supports `layer-shell` and `foreign-toplevel`.
- **XWayland shortcut**: under labwc OpusDM can run unchanged through
  XWayland, but only partially. X11 clients cannot see native Wayland
  windows, so the tasklist (`_NET_CLIENT_LIST`) lists only XWayland apps and
  `XGrabKey` only sees keys pressed while an X11 window has focus.
- Reasonable target for a desktop-manager replacement: a wlroots compositor
  (labwc, sway, river), not an app on top of GNOME.

## Proposed strategy

1. **Window-system backend abstraction.** A trait with X11, layer-shell and
   no-op implementations. Today the calls are scattered over 5 crates but few
   and well isolated.
2. **Hotkeys.** GlobalShortcuts portal, plus a CLI/IPC fallback so the user can
   bind keys in the compositor config (the IPC already exists).
3. **Desktop, panel, dock.** `gtk4-layer-shell` on Wayland. The GTK layout is
   unchanged, only the anchoring changes. Moderate effort.
4. **Tasklist and pager.** `foreign-toplevel` where available, otherwise hide
   the widgets.
5. **Button banks.** Rethink positioning (layer-shell with margins, or plain
   windows with no stored position).
6. **Test environment.** Add a `run-wayland.sh` sibling of `run-x11.sh` using a
   nested wlroots compositor.

## Effort estimate (rough)

- Starts and runs on Wayland with reduced features (lister, ordinary windows,
  SNI tray): **1-2 days**, mostly free thanks to the existing no-ops.
- Reasonable parity on wlroots/KDE (layer-shell, foreign-toplevel, hotkey
  portal): **1-2 weeks**, plus the headless test setup.
- Parity on GNOME: **not reachable** for desktop, panel and tasklist, because of
  compositor limits and not of the code.

## Global hotkeys today

- There are **no default global hotkeys**: `hotkeys.json` is empty (`[]`) and
  `Bindings::default()` has none.
- The mechanism is complete but X11 only (`crates/hub/src/global_keys.rs`).
  Each entry of `hotkeys.json` looks like
  `{ "key": "Ctrl+Alt+U", "function": "<id>", "global": true }`.
- With `global: true` the hub grabs the key on the root window with
  `XGrabKey`, so it fires with any window focused. A global hotkey needs at
  least Ctrl, Alt or Super: otherwise it is dropped with a log line and the
  editor validation reports it (`HotkeyGlobalNeedsModifier`).
- Non-global hotkeys only work inside listers.
- Configured from the Hotkey Editor (`lister/src/hotkey_editor.rs`).
- Typical uses (see FEATURES.md F-03 and the hide/reveal feature): open a new
  lister from any window, `Toggle Hide`.
- FEATURES.md F-03 already documents that Wayland is unsupported for global
  grabs.

## Decisions (2026-10-10)

- **One binary for both sessions.** The window system is detected at run time
  (`opusdm_ipc::session`): `GDK_BACKEND` forcing x11 wins, then
  `WAYLAND_DISPLAY`, then `DISPLAY`, the same order GDK uses. X11 must keep
  working exactly as before: every Wayland path is chosen only when the
  session is Wayland, never as a replacement.
- Under a Wayland session `DISPLAY` is usually set too (XWayland). The X11
  code must **not** run there: it would talk to XWayland and see only X
  clients. Every X11 entry point is gated on the detected window system, not
  on whether an X connection happens to open.
- **Target: full parity** on wlroots compositors, **labwc** as the reference
  (and for `run-wayland.sh`). Panel, dock and desktop use `gtk4-layer-shell`;
  where layer-shell is missing they stay plain windows.
- System packages are installed by hand:
  `sudo apt install libgtk4-layer-shell-dev labwc wlr-randr`.

## Code inventory update (2026-10-10)

New X11 code since the analysis:

| File | Role | Wayland plan |
|---|---|---|
| `crates/x11` | Shared connection and EWMH helpers (no GTK) | X11 only, gated |
| `crates/hub/src/launch_focus.rs` | Startup notification + `_NET_ACTIVE_WINDOW` | xdg-activation token (`XDG_ACTIVATION_TOKEN`) |
| `crates/hub/src/tray_start.rs` | Minimize + close the first window of a tray app | foreign-toplevel `close` |
| `crates/hub/src/fullscreen.rs` | Is the desktop covered? | foreign-toplevel `fullscreen` state |
| `crates/hub/src/screen_hotplug.rs` | RandR events -> reapply `screens.json` | GDK monitor list changes |
| `crates/screen` | `xrandr` backend | `wlr-randr` backend |
| `crates/input-config` | `xinput`/`xset` backend | not portable (compositor config), message |
| `crates/shot/src/capture.rs` | `GetImage` on the root | `grim` (wlr-screencopy), portal fallback |
| `crates/snap` | Edge snapping of other windows | impossible for a client; labwc snaps natively. Off on Wayland |
| `crates/runner/src/ui.rs` | Run window placement | layer-shell OVERLAY, keyboard on demand |
| `crates/panel/src/device_popup.rs` | Popup in the corner | layer-shell OVERLAY with margins |
| `crates/hub/src/toolbar.rs` | Desktop count | `ext-workspace` when the compositor offers it |

## Implementation plan

Each step is one or more commits; X11 is tested after each one.

1. **Session detection and gating.** `opusdm_ipc::session`; every X11 entry
   point (helpers, observer threads, grabs, hot plug, launch focus, tray
   start, snap, shot) does nothing on Wayland. `run-wayland.sh` with nested
   labwc. Result: clean start on Wayland with reduced features.
2. **Layer shell.** `gtk4-layer-shell` for desktop (BACKGROUND), panel (TOP,
   exclusive zone), dock (BOTTOM), Run window and device popup (OVERLAY).
   Button banks: TOP layer, position kept as margins.
3. **Foreign toplevel.** A Wayland observer thread
   (`zwlr_foreign_toplevel_manager_v1`) feeding the same `EwmhEvent`s to the
   tasklist; activate, minimize, close. Used also by fullscreen detection and
   tray start. Pager and desktop count through `ext-workspace-v1` when present,
   otherwise hidden.
4. **Global hotkeys.** `org.freedesktop.portal.GlobalShortcuts` where the
   portal offers it; otherwise a `opusdm-hub --function <id>` command line and
   an export of the bindings as labwc `rc.xml` keybinds.
5. **Screens and screenshots.** `wlr-randr` backend for `opusdm-screen` and the
   hot plug; `grim` for `opusdm-shot`.
6. **Launch focus** through xdg-activation tokens.

## What works where (2026-10-10)

One binary, chosen at run time (`opusdm_ipc::session`). Build and run time
need `libgtk4-layer-shell` (on X11 too); `wlr-randr` is needed on Wayland
for the screen settings.

| Feature | X11 | labwc / sway (wlroots) | KDE Plasma | GNOME |
|---|---|---|---|---|
| Desktop, panel, dock, Run window, device popup | EWMH | layer shell | layer shell | plain windows |
| Borderless button banks (saved position) | EWMH | layer shell, Alt+drag | layer shell, Alt+drag | plain windows |
| Tasklist, pager, desktop switch | EWMH | foreign-toplevel, ext-workspace | no (KDE protocols) | no |
| Full screen pause, Start in Tray | EWMH | foreign-toplevel | no | no |
| Global hotkeys | `XGrabKey` | labwc keybinds (`rc.xml` block); sway: bind `opusdm-hub --run` by hand | GlobalShortcuts portal | GlobalShortcuts portal |
| Screen settings, hot plug | `xrandr` | `wlr-randr` | no | no |
| Input settings | `xinput`, `setxkbmap`, `xset` | labwc (`rc.xml`, `environment`) and Wayfire (`wayfire.ini`) | no | no |
| Screenshots | `GetImage` | wlr-screencopy | no | no |
| XEmbed tray | yes | no (SNI only) | no (SNI only) | no (SNI only) |
| Window snapping (`opusdm-snap`) | yes | compositor's own | compositor's own | compositor's own |
| Launch focus | startup notification | not needed (labwc focuses new windows) | - | - |
| Number of virtual desktops | EWMH | compositor config (labwc `<desktops>`) | - | - |
| Window opacity (picom) | yes | no | no | no |

Known gaps on Wayland: the tasklist shows no window icons from the
window itself (the desktop file of the app id instead), no Force kill
(no pid) and no Move to desktop; the pager draws a cascaded placeholder box per window (no real position); a full
screen window on another workspace also pauses the background.

Testing: `./run-wayland.sh` (nested labwc), `./run-wayland.sh --headless`
for scripted runs (always on a private D-Bus session bus: the script runs
itself again under `dbus-run-session`, needed on `PATH`); `crates/wayland/examples/watch.rs` prints the windows and
workspaces, `examples/screenshot.rs` saves every output.

## Native compositor (opusdm-compositor)

OpusDM's own Wayland compositor, `crates/compositor` (Smithay 0.7), built in
five stages (design: `docs/superpowers/specs/2026-11-25-opusdm-compositor-design.md`).
Stages 1 and 2 are done: it runs nested, as a window of the current
desktop (winit backend, GLES rendering), and is not a session yet.

What stage 1 covers:

- xdg-shell windows: cascade placement, client side decorations, move by
  title bar, maximize / restore, fullscreen (above the TOP layer); `Alt` +
  left drag moves, `Alt` + right drag resizes.
- xdg popups (menus) kept inside the output, with grabs: a click outside
  closes them.
- wlr-layer-shell: desktop (BACKGROUND), dock (BOTTOM), panel (TOP, its
  exclusive zone respected by maximized windows), Run window and
  notifications (OVERLAY, exclusive keyboard for the Run window).
- Click to focus, keyboard and pointer to clients; a client that dies
  (even `kill -9`) does not take the compositor down.

What stage 2 adds:

- Window ids, 4 virtual desktops (`desktop.json` `virtual_screens`, 1..16,
  changed live by the hub through `desktops/set-count`), minimize. A desktop
  switch focuses the top window of the new desktop; a layer surface (panel,
  Run window) never takes the active window.
- A private IPC (`$XDG_RUNTIME_DIR/opusdm-compositor-<WAYLAND_DISPLAY>.sock`,
  exported to children as `OPUSDM_COMPOSITOR_SOCKET`): window list and
  actions, desktops, hotkeys, a `watch` event stream. The panel and the hub
  use it through `opusdm_wayland::native` when they run under the compositor.
- wlr-foreign-toplevel and ext-workspace: the panel tasklist (minimize,
  restore, Move to desktop, Force kill) and pager (one box per desktop,
  click to switch) work as on X11.
- Global hotkeys: the hub sends its `hotkeys.json` global table
  (`hotkeys/set`, sent again after a hub restart; a hotkey pressed with no
  hub logs `hotkey <combo>: no hub connected`). Built in: Quit
  (`Ctrl+Alt+BackSpace`) and the window menu (`Alt+space`), configurable in
  `compositor.json` (`window_menu`, `quit`, `quit_enabled`; another file
  with `OPUSDM_COMPOSITOR_CONFIG`).
- The window menu, drawn by the compositor with the OpusDMTheme colours:
  Maximize / Restore, Minimize, Move to desktop 1..N, Close, Kill; keyboard
  and pointer.
- An exclusive layer (the Run window) closes open menus; xdg-activation
  (a notification click brings its application's window forward).

Not there yet (stage 3 and later): wlr-screencopy (opusdm-shot starts on
Ctrl+Print but cannot capture: "the compositor offers no screen copy"),
wlr-output-management (the hub logs that `wlr-randr` failed), XWayland,
the DRM / libinput session and the VT-switch bindings. Known in stage 2:
the tasklist shows the windows in stacking order, so the buttons move when
the active window changes (X11 keeps the map order); the first panel click
after a tasklist menu closed can be lost. The keymap is the xkb default
(`us`) unless `XKB_DEFAULT_LAYOUT` and the other `XKB_DEFAULT_*` variables
are set. On a software renderer (Xephyr, llvmpipe) `linux-dmabuf` is not
offered and one log line says so; clients use shared memory.

Running it: `./run-native.sh [--debug] [resolution]` builds the workspace,
opens the compositor window (default 1920x1080) and starts `opusdm-hub`
inside it with its own runtime dir, `target/release` first on `PATH` and
no Startup Apps (`--no-startup-apps`, as `run-x11.sh`). `--debug` sets
`OPUSDM_DEBUG=1` and writes the hub output to `/ramdisk/opusdm.log`, or
to `$NATIVE_LOG` when set; `NATIVE_RES` sets the default resolution.
The sandbox never writes the live configuration: `XDG_CONFIG_HOME` (for
the compositor, which only reads it, and the hub with every tool it starts)
is a temp folder whose `opusdm` is a copy of `~/.config/opusdm` and whose
other entries are symlinks to `~/.config`, removed at exit. In the copy
every global hotkey gets `Super+` before its key name, so the host desktop
keeps its own combos (the table used is printed at start; an entry that
already has `Super` is kept and listed on stderr as "may be taken by the
host"; `jq` is needed):

    Alt+Super+1             builtin-desktop-prev
    Alt+Super+2             builtin-desktop-next
    Ctrl+Super+backslash    exec:opusdm-terminal
    Ctrl+Super+Print        exec:opusdm-shot
    Ctrl+Shift+Super+space  exec:opusdm-run

and the window menu is `Super+Alt+space` (a temp `compositor.json` through
`OPUSDM_COMPOSITOR_CONFIG`: the host Openbox takes `Alt+space`). The real
combos are checked on a spare VT in stage 4. The script always runs on a
private D-Bus session bus (it runs itself again under `dbus-run-session`,
needed on `PATH`), so the sandbox hub never takes the live session's
`org.freedesktop.FileManager1`, notification or SNI names; `kill <script pid>`
or Ctrl+C still ends the compositor, the hub and the tools it started. E.g.
in a Xephyr:

    Xephyr :7 -ac -screen 1600x1000 -no-host-grab &
    env -u WAYLAND_DISPLAY DISPLAY=:7 ./run-native.sh --debug 1440x900

Without a window manager in Xephyr, give the compositor window the
keyboard focus (`xdotool windowfocus`) or keys do not arrive. On an Xvfb /
Xephyr with the default keymap, `xdotool key F2` presses `Alt_L` with the
F-key (the F-keys have the `CTRL+ALT` key type): the Lister sees Alt+F2 and
does nothing, and under a window manager `xdotool key F4` is Alt+F4, which
closes the window. Load a keymap whose F-keys are `ONE_LEVEL` first
(`xkbcomp` a dump of the server's keymap with `key <FK02> { type=
"ONE_LEVEL", symbols[Group1]= [ F2 ] };` and so on), or check with `xev`.
