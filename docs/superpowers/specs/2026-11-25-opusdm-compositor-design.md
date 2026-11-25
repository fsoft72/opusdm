# OpusDM native Wayland compositor - design

Date: 2026-11-25. Status: draft for the owner's review; approach approved in chat by the owner
(approach A: Smithay, in stages).

## Intent

OpusDM runs today on X11 (Openbox) and on Wayland under labwc or Wayfire
(see `docs/WAYLAND-PORT.md`). On Wayland the most distinctive features depend
on what the third party compositor chooses to offer, and some are missing or
work only through workarounds:

- global hotkeys exist only as an exported `rc.xml` keybind block (labwc) or
  by hand (sway);
- full window control (pid for Force kill, Move to desktop, real window
  positions in the pager) exists only under Wayfire, through its IPC;
- snapping, opacity, input settings, the number of desktops and the screen
  layout live in each compositor's own config files or in external tools
  (`wlr-randr`, picom).

The owner wants their **own compositor**, written in Rust inside this
workspace, so that:

1. global hotkeys work natively;
2. OpusDM has complete control of every window;
3. labwc / Wayfire / `wlr-randr` / picom are no longer packages to install on
   each distro;
4. every feature OpusDM needs is guaranteed to be there, because OpusDM owns
   the compositor.

## Goals

- A new crate `crates/compositor`, binary `opusdm-compositor`, built on
  Smithay 0.7.
- A new session entry "OpusDM (native)" next to the existing X11, labwc and
  Wayfire ones.
- Window management with the same model as Openbox / labwc today: floating
  stacking windows, cascade placement, 4 virtual desktops in a row,
  maximize, minimize, fullscreen, Alt+drag move, Alt+right-drag resize, edge
  snapping, multi-monitor.
- Everything the existing OpusDM Wayland client code already uses, served by
  the compositor itself, so panel, dock, desktop, shot and runner work
  **unchanged**: `wlr-layer-shell`, `wlr-foreign-toplevel-management`,
  `ext-workspace`, `wlr-screencopy`, `xdg-activation`, `xdg-decoration`.
- A private IPC (Unix socket, JSON) for what no standard protocol covers:
  global hotkeys, window geometry / pid / desktop, Move to desktop, opacity,
  output layout, input settings, number of desktops, quit.
- XWayland, so X11 apps keep working.

## Non-goals

- Tiling, scrolling or any layout other than floating stacking.
- Animations and visual effects beyond per-window opacity.
- Supporting the private IPC from other desktops: it is OpusDM's channel only.
- Replacing the X11 session or dropping labwc / Wayfire support. They stay.
- Running on GPUs without GBM/EGL support (software rendering on real
  hardware). The nested sandbox may use whatever the host offers.

## Anti-goals (failure even if it "works")

- **Any regression of the X11, labwc or Wayfire sessions.** The native
  compositor is an addition. Every client-side change is chosen only when
  running under `opusdm-compositor`, the same way `under_wayfire()` gates
  the Wayfire path today.
- **Losing the session to a compositor crash with no way back.** The other
  session entries stay installed and selectable at the login screen until
  the owner decides otherwise.
- **Silent failure** (project CLAUDE.md rule 3): a hotkey that does nothing,
  a lost hub connection, a protocol request that is refused, must all leave
  a log line.
- **A compositor that can freeze on a client.** No blocking I/O on the
  compositor's event loop: the private IPC and the hub connection are
  non-blocking calloop sources with bounded buffers; a client that does not
  read is dropped with a log line.

## Constraints

- Rust, in this Cargo workspace (`rust-version = "1.92"`, edition 2021 for
  the workspace crates; Smithay itself is edition 2024, which is fine as a
  dependency).
- System libraries needed at build and run time: libinput, libseat (or
  logind through libseat), libudev, libgbm, libEGL / libGLESv2,
  libxkbcommon, Xwayland (run time). They replace labwc, wayfire,
  `wlr-randr` and picom in `Recommends`.
- Target hardware (checked 2026-11-25): one AMD iGPU (Krackan, Radeon
  840M/860M, `amdgpu`), laptop panel `eDP-1` 1920x1080, scale 1, external
  monitors through hot plug. Single GPU: no multi-GPU or NVIDIA work in
  stage 4. Fractional scaling stays in stage 5.
- The daily session today is **X11** (Openbox). The native session competes
  with it, not with labwc.
- The owner uses OpusDM every day as their desktop (project `CLAUDE.md`).
  The compositor must be usable in a nested sandbox for a long time before it
  touches the real session.
- Project rules apply: one risky change per commit, `CHANGES.md` updated
  for every step including what was not verified, no `--no-verify`, a
  regression test for every wiring change.

## Approaches considered

| | Approach | Trade-off | Outcome |
|---|---|---|---|
| A | Own compositor in Rust with Smithay, new crate | Same language and IPC as the rest of OpusDM, full control; everything must be written (window management, input, outputs, protocols, XWayland). Months of work. | **Chosen** by the owner |
| B | Fork of labwc (C, wlroots) with hotkeys and an IPC added | Working base on day one, weeks of work; C code and wlroots upgrades to maintain forever; not really "owned". | Rejected |
| - | wlroots through Rust bindings | No maintained bindings. | Dropped |

Reason for A: it is the only option that makes the compositor part of OpusDM
(same language, same `opusdm_ipc`, same tests and release pipeline), which is
the point of goals 2 and 4.

## Architecture

### Process model

```
login manager
  -> opusdm-session-native            (shell script, like opusdm-session-wayland)
       -> exec opusdm-compositor      (owns the Wayland socket for the whole session)
            -> spawns opusdm-hub      (once the socket is ready, env set)
                 -> panel, dock, desktop, listers... (unchanged)
            -> spawns Xwayland        (lazily, on first X11 client)
```

The compositor owns the session: when it exits, the session ends
(`opusdm_wayland::compositor::exit()` already sends it `SIGTERM`, which must
shut it down cleanly).

### Ownership and lifetimes (project CLAUDE.md rule 2)

- **Who owns the state for the whole run:** a single `State` struct owned by
  the calloop `EventLoop` in `main`. It lives until the loop exits. Every
  protocol handler, the IPC server and the backend reach it through the
  loop's `&mut State`, never through `Rc`/`Weak`.
- **Windows** are owned by the `Space` (Smithay `desktop::Space`) plus a
  `HashMap<WindowId, WindowInfo>` for OpusDM data (desktop index, opacity,
  saved geometry before maximize). A window is removed from both when its
  surface is destroyed; any request for an unknown id is answered with an
  error and logged.
- **The hub** is a client like any other. If it disappears, the compositor
  keeps running, logs `hub disconnected`, and global hotkeys that need the
  hub log `hotkey <combo>: no hub connected` when pressed. The compositor's
  own safety bindings (below) keep working.
- **IPC connections** are owned by the IPC server source. A connection that
  errors or overflows its buffer is closed and logged; the server keeps
  accepting.

### Crate layout (builder may refine)

```
crates/compositor/
  src/main.rs          args (--nested / --drm, --debug), logging, event loop
  src/state.rs         State, the delegate_* registrations
  src/backend/winit.rs nested backend (sandbox)
  src/backend/drm.rs   udev + libseat + DRM/GBM (real session)
  src/shell/xdg.rs     xdg-shell, xdg-decoration, placement
  src/shell/layer.rs   wlr-layer-shell, exclusive zones
  src/wm.rs            stacking, focus, desktops, maximize, snap, move/resize grabs
  src/input.rs         libinput / winit input, keyboard bindings, xkb
  src/decor.rs         server side title bars for clients that ask for them
  src/protocols/       wlr-foreign-toplevel, ext-workspace, wlr-screencopy (server side)
  src/ipc.rs           private IPC server
  src/xwayland.rs      XWayland and its window manager
```

Pure logic (placement, snap zones, desktop switching, hotkey matching,
IPC message parsing) lives in modules with no Smithay types so it can be unit
tested, like `crates/wayland/src/model.rs` does today.

### Protocols served

Standard, so the existing client code works unchanged:

| Protocol | Smithay helper | Used today by |
|---|---|---|
| xdg-shell, xdg-decoration | yes | every app |
| wlr-layer-shell | yes | desktop, panel, dock, runner, device popup, button banks |
| wlr-foreign-toplevel-management | **no**, server side with `wayland-protocols-wlr` | panel tasklist, fullscreen detection, tray start |
| ext-workspace | **no**, server side with `wayland-protocols` | pager, desktop count |
| wlr-screencopy | **no**, server side with `wayland-protocols-wlr` | `opusdm-shot`, `crates/wayland/src/screencopy.rs` |
| ext-image-copy-capture | yes | later: screen sharing through a portal |
| xdg-activation | yes | launch focus |
| session-lock, idle-notify, idle-inhibit | yes | screen locker, power (later phase) |
| data-device, primary selection | yes | clipboard, drag and drop |
| keyboard-shortcuts-inhibit | yes | virtual machines, remote desktop (later) |
| XWayland | yes (`smithay::xwayland`) | X11 apps |

### Private IPC

- Socket `$XDG_RUNTIME_DIR/opusdm-compositor-<WAYLAND_DISPLAY>.sock`, path
  exported to children as `OPUSDM_COMPOSITOR_SOCKET`.
- Wire format: the same framing as the Wayfire IPC OpusDM already speaks
  (4 byte little-endian length + JSON), so `crates/wayland` reuses its
  reader. Request `{"method": ..., "data": {...}}`, reply object with
  `"error"` on failure, `watch` turns the connection into an event stream.
- Client side: a new module `opusdm_wayland::native` with `under_native()`,
  next to `labwc` and `wayfire`. The panel's `wl_windows.rs`, the hub's
  `global_keys_wl.rs`, `input-config` and `screen` gain a native branch,
  chosen only under `under_native()`.

Methods (first version):

| Method | Purpose |
|---|---|
| `windows/list`, `watch` | every window: id, app id, title, pid, geometry, desktop, states |
| `window/activate`, `minimize`, `close`, `kill` | window control (kill by pid on the compositor side) |
| `window/move-to-desktop`, `window/set-geometry`, `window/set-opacity` | full window control |
| `desktops/set-count`, `desktops/switch` | virtual desktops |
| `hotkeys/set` | the hub sends the full global hotkey table; replaces the previous one |
| `outputs/list`, `outputs/apply` | screen layout (replaces `wlr-randr`) |
| `input/apply` | keyboard layout, repeat, pointer and touchpad settings |
| `session/quit` | end the session |

Events on a `watch` connection: window added / changed / removed, desktop
changed, output changed, and `hotkey` with the function id.

### Global hotkeys

- The hub keeps owning `hotkeys.json` and the Hotkey Editor. Under the
  native compositor it sends the global entries with `hotkeys/set` at
  startup and whenever they change, and listens for `hotkey` events, then
  runs the function the same way the X11 path does.
- The compositor matches key presses against the table **before** the
  focused client, using xkb keysyms (independent of keyboard layout and of
  lock keys). A matched key is not forwarded to the client.
- Safety bindings built into the compositor, working with no hub at all:
  Ctrl+Alt+F1..F12 (switch VT), Ctrl+Alt+Backspace (quit, configurable to
  off). The hub cannot override them.
- If a hotkey matches and no hub `watch` connection exists: log line, key
  swallowed. If the hub reconnects it must resend the table.

### Built-in window manager bindings

Decided by the owner: every window manager binding of today's
`openbox-rc.xml` is built into the compositor, with these defaults,
configurable in `compositor.json`:

| Keys | Action |
|---|---|
| Alt+Tab, Alt+Shift+Tab | window switcher (popup), next / previous |
| Ctrl+Alt+Tab | the same switcher as in `openbox-rc.xml` |
| Alt+F4 | close the window |
| Ctrl+Alt+Left/Right, Super+F1..F4 | switch desktop |
| Shift+Alt+Left/Right | move the window to the previous / next desktop and follow it |
| Super+D | show the desktop (toggle) |
| Alt+Escape | lower the window |
| Alt+Space, right click on the title bar | window menu |

The exact actions are copied from `openbox-rc.xml` (the builder reads it for
the details, for example wrap-around). Matching order: safety bindings, then
built-in bindings, then the hub's global hotkeys. A combo used both by a
built-in binding and by `hotkeys.json` is reported as a conflict by the
Hotkey Editor validation (a new error next to `HotkeyGlobalNeedsModifier`,
active under the native compositor), and at run time the built-in wins with
a log line naming both.

The window switcher popup and the window menu are drawn by the compositor
itself, with the title bar colours and a text renderer of the builder's
choice. Both are required before the native session entry is offered.

### Window management behaviour

Mirrors `labwc-rc.xml` and `openbox-rc.xml` as used today:

- placement: cascade; focus follows click; new windows get focus
  (xdg-activation token honoured when present);
- 4 desktops in a row by default, count settable via IPC;
- Alt+left drag move, Alt+right drag resize, edge snapping (left/right
  half, top maximize), threshold configurable;
- maximize respects layer-shell exclusive zones (panel, dock);
- fullscreen covers the whole output, above the TOP layer;
- server side title bar only for clients that request it through
  xdg-decoration (GTK4 apps draw their own): close, iconify, maximize on the
  left, then the title, colours taken from the active OpusDM theme.

### Configuration

The files OpusDM already has stay the **single source of truth**; no
setting is duplicated:

- desktop count (`virtual_screens`), snapping (`snap_enabled`) and the
  active / inactive window opacity: `DesktopSettings` (`desktop.json`). The
  compositor applies the same active / inactive opacity rule picom applies
  on X11 (`crates/hub/src/compositor.rs`), plus per-window overrides from
  `window/set-opacity`;
- screen layout: `screens.json`, one layout per set of connected monitors;
- input settings: the `crates/input-config` store.

The compositor reads these files itself (through `opusdm_ipc`, read only)
at startup, so outputs and keyboard are right before the first client
connects, and on hot plug it picks the matching `screens.json` layout
itself (the hub's `screen_hotplug.rs` does nothing under the native
compositor). The settings apps keep writing their files and then call
`desktops/set-count`, `outputs/apply` or `input/apply` so the change takes
effect at once. A failed apply is reported back to the app, which shows it.

`~/.config/opusdm/compositor.json` holds only what exists nowhere else:
the built-in key bindings and safety bindings, the snap threshold, the
title bar layout. It is read at startup and on IPC `reload`.

### Title bar colours

Taken from the Openbox theme the X11 session already uses (`OpusDMTheme`,
`themerc`): a small parser for the frame keys (active / inactive title
colour, text colour, button colours, border). Same look on X11 and native,
no change to the OpusDM theme format.

## Stages

Each stage ends with the sandbox exercised by hand and `CHANGES.md` updated.
Only after stage 4 is the native session offered at the login screen.

1. **Nested skeleton.** `opusdm-compositor --nested` on the winit backend:
   xdg-shell, layer-shell, seat, keyboard and pointer, cascade placement,
   move / resize, focus. `run-native.sh` sibling of `run-wayland.sh` starts
   it and the hub inside it. Exit criterion: desktop, panel, dock and a
   Lister appear and work in the nested window.
2. **OpusDM integration.** wlr-foreign-toplevel, ext-workspace, desktops,
   private IPC, global hotkeys, `opusdm_wayland::native` and the client
   branches. Exit criterion: the project CLAUDE.md checklist (Lister,
   global hotkeys, hub) passes in the sandbox.
3. **Full window control.** Force kill, Move to desktop, real pager boxes,
   snapping, opacity, server side decorations, wlr-screencopy (`opusdm-shot`),
   clipboard and drag and drop between apps, the built-in window manager
   bindings with the window switcher and the window menu.
4. **Real session.** DRM backend (udev, libseat, GBM), multi-monitor and hot
   plug, `outputs/*` and `input/*`, XWayland, idle-notify and idle-inhibit,
   `opusdm-session-native` and the `opusdm-native.desktop` session entry,
   packaging. Before the session entry is offered, the full project
   CLAUDE.md checklist runs on a spare VT with the DRM backend.

### Packaging

`opusdm-compositor` ships as its **own package** (`opusdm-compositor`), so
libinput, libseat and GBM do not become hard dependencies of every OpusDM
install. The main package `Recommends` it; labwc, wayfire, wlr-randr and
picom stay in `Recommends` as they are, since the X11, labwc and Wayfire
sessions remain supported.
5. **After daily use.** Screen lock (session-lock protocol plus a locker
   client, to be chosen then; decided by the owner: the native session may
   go into daily use without it), screen sharing (ext-image-copy-capture
   with a portal backend), keyboard-shortcuts-inhibit, fractional scale,
   tuning.

## Testing

- Unit tests for every pure module (placement, snap, desktop index, hotkey
  matching, IPC framing and parsing).
- An integration test that starts the compositor headless or nested,
  connects a test client, and checks: a toplevel appears in `windows/list`;
  a layer surface gets its exclusive zone; `hotkeys/set` followed by a
  synthetic key press produces a `hotkey` event. How to run it without a
  display (a headless backend with an offscreen renderer) is left to the
  builder.
- A `dlog!`-style trace for hub connect / disconnect and for every hotkey,
  so the session log shows the wiring.
- Manual checklist from the project CLAUDE.md, in `run-native.sh` for
  stages 1-3 and on a spare VT / real session for stage 4.
- Global hotkeys in the nested sandbox: the host X11 session grabs
  Ctrl+backslash, Ctrl+Shift+space, Ctrl+Print and Alt+1/2 before the nested
  window sees them. `run-native.sh` therefore runs the hub with a sandbox
  hotkey table (the same functions on combos the host does not grab, for
  example Super+Ctrl+backslash). The real combos are checked on the spare VT
  in stage 4. `CHANGES.md` says so at each stage.

## Decided by the owner

- Approach A: own compositor in Rust with Smithay, built in stages.
- Goals: native global hotkeys, complete window control, fewer distro
  packages, guaranteed features.
- All window manager bindings of `openbox-rc.xml` built in, window switcher
  and window menu included, before the session entry is offered.
- Screen lock can wait (stage 5).
- Work happens on the `opus-wayland` branch.

## Calls made by Claude (owner may correct)

- Crate `crates/compositor`, binary `opusdm-compositor`, session entry
  "OpusDM (native)".
- Serve the same standard protocols the existing client code uses, so panel,
  dock, desktop and shot work without changes; add a private IPC only for
  what the protocols do not cover.
- Private IPC framed like Wayfire's IPC, to reuse the existing reader.
- The hub keeps owning hotkeys; the compositor only matches and forwards.
- Built-in safety bindings (VT switch, quit) that work without the hub.
- Window management copies today's labwc / Openbox setup; server side title
  bars only for clients that ask for them.
- labwc, Wayfire and X11 sessions stay supported.
- Existing settings files stay the source of truth; the compositor reads
  them and applies the layout on hot plug itself.
- Title bar colours parsed from the `OpusDMTheme` Openbox theme.
- Separate `opusdm-compositor` package; `Recommends` unchanged otherwise.
- Sandbox hotkeys on substitute combos; real combos verified on a spare VT.

## Left to the builder

- Exact module split inside the crate.
- Renderer choice per backend (GLES everywhere, Pixman fallback in the
  sandbox if useful).
- How to run integration tests without a display.
- XWayland window manager details (Smithay's `X11Wm`).
