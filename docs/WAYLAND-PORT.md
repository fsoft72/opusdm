# Wayland port: exploration notes

Status: **exploration only, nothing implemented.** These notes record how hard
a Wayland port of OpusDM would be, what blocks it and a possible strategy, so
the work can start later without redoing the analysis.

Date of the analysis: 2026-10-05.

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

## Open questions

- Which compositor is the primary Wayland target (labwc is the natural one)?
- Is it acceptable to ship panel, dock and desktop only where `layer-shell`
  exists, and degrade to plain windows elsewhere?
- Should X11 and Wayland builds share one binary (runtime detection) or use
  cargo features?
