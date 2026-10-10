# Wayland port: exploration notes

Status: **in progress** (started 2026-10-10). See "Decisions" and
"Implementation plan" at the end; the analysis below is from 2026-10-05.

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
