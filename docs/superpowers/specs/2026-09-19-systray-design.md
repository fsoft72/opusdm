# OpusDM Panel system tray - design

Status: approved 2026-09-19. Adds a legacy XEmbed system tray (freedesktop
`systemtray-spec`) to `opusdm-panel`, on the right side of the bar, before the
CPU/network widget.

## Goals

- Other apps (e.g. a VPN client, a chat app, a sync daemon) can dock a tray
  icon in the panel via the standard XEmbed system tray protocol.
- Icons appear left-to-right in dock-request order, fixed size, clickable
  (clicks reach the docked app normally - it is a real X window).
- Icons are removed cleanly when the owning app withdraws or exits.
- If another system tray is already running, opusdm-panel's tray stays
  inactive rather than fighting over ownership.

## Non-goals

- StatusNotifierItem/DBus tray protocol (SNI). Only XEmbed for v1.
- Icon reordering/drag, tooltips/balloons, XEMBED focus forwarding.
- `_NET_SYSTEM_TRAY_VISUAL` / compositing negotiation - default visual only.
- Wayland. X11-only, matching the rest of the panel; widget stays empty
  (no crash) when there's no X11 surface.
- Multiple tray managers (one per X screen, not one per monitor).

## Architecture

Two new files in `crates/panel/src`, following the existing split between a
protocol thread (`ewmh.rs`) and a GTK widget (`widgets/sysmon.rs`):

| file | responsibility |
|---|---|
| `systray.rs` | background thread, own x11rb connection: selection ownership, `MANAGER` announce, dock/undock handling, icon window reparenting + XEMBED handshake, placement commands |
| `widgets/tray.rs` | GTK box of fixed-size placeholder squares (one per docked icon, in dock order); computes each icon's root-screen rect and sends it to the protocol thread |

`lib.rs`: right box becomes
`tray::build()`, separator, `sysmon::build()`, separator, `clock::build()`.

### Channels (mirrors the hub's `cmd_tx`/`evt_rx` split)

```rust
enum TrayEvent {
    IconDocked(u32),          // xid, appended to the end of the order
    IconRemoved(u32),         // xid
}

enum TrayCmd {
    PlaceIcon { xid: u32, x: i32, y: i32, w: i32, h: i32 },
}
```

`systray::spawn() -> (Sender<TrayCmd>, Receiver<TrayEvent>)`, same shape as
`ewmh::spawn()` plus a command side, since (unlike the read-only EWMH
observer) the tray needs the GTK side to tell it where things go.

## Protocol thread (`systray.rs`)

**Startup**: connect to X11, compute `_NET_SYSTEM_TRAY_S<screen>` atom for the
default screen. `GetSelectionOwner`; if non-zero, log
`"system tray already running, opusdm-panel tray disabled"` and stop (the
`Sender`/`Receiver` pair still exists but nothing is ever sent - `widgets/tray.rs`
just shows an empty box). Otherwise create a small (1x1) `InputOutput`
manager window, `SetSelectionOwner` for it, then send a `MANAGER`
`ClientMessage` to root: `(timestamp, selection_atom, manager_xid, 0, 0)`.
Set `_NET_SYSTEM_TRAY_ORIENTATION=0` (horizontal) on the manager window.

**Dock requests**: the manager window receives `ClientMessage` with
`message_type = _NET_SYSTEM_TRAY_OPCODE`, `data[1] = SYSTEM_TRAY_REQUEST_DOCK
(0)`, `data[2] = <icon xid>`. For each:
1. Create an override-redirect `InputOutput` container window, child of
   root, initial size = icon square size (`PANEL_HEIGHT - 2 * margin`).
2. `ReparentWindow(icon_xid, container_xid, 0, 0)`.
3. Resize the icon window to fill the container.
4. Send `_XEMBED` `XEMBED_EMBEDDED_NOTIFY` (opcode 0) to the icon window:
   `(timestamp=0, opcode=0, 0, container_xid, xembed_version=0)`.
5. `MapWindow` on both; `ConfigureWindow` stack mode `ABOVE`.
6. Record `xid -> container_xid` in a `HashMap`, send `TrayEvent::IconDocked(xid)`.
7. Select `StructureNotify` on the icon window (for step below).

**Removal**: `DestroyNotify`, `UnmapNotify`, or `ReparentNotify` where the new
parent isn't our container -> `DestroyWindow` the container, drop the map
entry, send `TrayEvent::IconRemoved(xid)`.

**Placement**: on `TrayCmd::PlaceIcon`, look up the container xid and
`ConfigureWindow` it to `(x, y, w, h)` (root-relative, since it's a root
child) - no reparenting involved, just moving the container.

**Event loop**: single thread `select`-style over the X connection's fd and
the `TrayCmd` receiver (mirrors the `ewmh.rs` blocking `wait_for_event` loop,
but also drains commands - use a non-blocking check of the command channel
after each X event, plus a short poll timeout so a `PlaceIcon` sent with no
X activity in between still lands promptly).

## Widget (`widgets/tray.rs`)

```rust
pub fn build() -> gtk::Widget
```

Holds `Rc<RefCell<Vec<u32>>>` (dock order) and a `gtk::Box` of one
zero-content `gtk::Box` placeholder per xid, fixed
`content_width/content_height` = icon square size, in a horizontal
`gtk::Box` with the same spacing as the sparkline meters.

On `TrayEvent::IconDocked`: append xid, append a placeholder widget, schedule
a reposition pass. On `TrayEvent::IconRemoved`: drop the xid + its
placeholder, schedule a reposition pass.

**Reposition pass** (`glib::idle_add_local_once`, so it runs after GTK has
laid the new children out): for each placeholder, `compute_bounds` relative
to the bar's `ApplicationWindow`, add the bar's known root origin
`(mon_x, mon_y)` - the same value `build_bar_window` already has and that
`window.rs::pin_geometry` uses - to get the absolute screen rect, and send
`TrayCmd::PlaceIcon`. Also re-run this pass from the existing panel
reposition timers (the `[80, 400, 1000]` ms settle delays in
`build_bar_window`) so a monitor-hotplug rebuild re-places any icons docked
before it.

No rendering in the placeholder itself - the real pixels are the docked X
window, drawn by the X server directly on top, so no GTK draw/paint code is
needed here.

## Error handling

- No X11 surface (Wayland/test env): `systray::spawn` returns immediately
  after a failed `x11rb::connect`, channels stay silent, widget shows empty.
  Logged once via `opusdm_ipc::err_log!` (matching `window.rs`'s pattern).
- Selection already owned: logged once, no manager window created, no
  further action - never contend for the selection.
- Any single X request failing (icon vanished mid-handshake, etc.): log via
  `err_log!` and continue the loop; never let one bad icon crash the thread.

## Testing

Pure, X-free helpers get unit tests, matching `ewmh.rs`/`window.rs`:

- XEMBED `XEMBED_EMBEDDED_NOTIFY` message word layout.
- `MANAGER` announce message word layout.
- Icon square size / spacing math (if factored into a standalone function
  rather than inline GTK calls).

The live protocol (selection ownership, reparenting, XEMBED handshake) is
exercised manually against a real X session - no automated coverage, same as
the rest of the panel's X11-direct code.
