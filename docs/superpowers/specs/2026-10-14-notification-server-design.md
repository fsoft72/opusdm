# opusdm-notify: desktop notification server

## Problem

Notifications sent by applications (Brave / WhatsApp Web, `notify-send`,
mail clients) never appear in an OpusDM session. Nothing owns
`org.freedesktop.Notifications` on the session bus: the only activatable
provider is Plasma's `plasma_waitforname`, which waits for a `plasmashell`
that never runs, so every notification is silently dropped (the browser's
`show` event still fires).

## Solution

A new binary, `opusdm-notify`, in its own crate (`crates/notify`), shaped like
`opusdm-polkit`: a GTK4 application (`org.opusdm.Notify`, one instance) that
the hub starts at login with `children::spawn_tool`. It owns
`org.freedesktop.Notifications` and draws the popups. A private interface,
`org.opusdm.Notifications1`, serves the history, Do Not Disturb and the mute
list to a new panel widget (bell).

If another daemon already owns the name (dunst, Plasma), `opusdm-notify` logs
it and exits; it never replaces an owner.

### Parts

| Module | Role |
|---|---|
| `model.rs` | Plain data: `Notification` (id, app_name, app_icon, summary, body, actions, urgency, expire_timeout, image, desktop_entry, transient, resident, received time). Hint parsing (`urgency`, `image-data` / `image_data` / `icon_data`, `image-path` / `image_path`, `desktop-entry`, `transient`, `resident`). Markup whitelist. No GTK, no D-Bus types beyond hint values. |
| `store.rs` | State: live notifications, history (memory only, `HISTORY_MAX = 100`), unread count, DND, mute list. `decide` routes a notification. No GTK. |
| `config.rs` | `~/.config/opusdm/notifications.json` (`corner`, `timeout_ms`, `dnd`, `mute`), read with `opusdm_ipc::settings::load_json`, written only by `opusdm-notify`. |
| `server.rs` | zbus `org.freedesktop.Notifications` at `/org/freedesktop/Notifications` on the session bus. Replies with the id at once and forwards to the GTK loop through an `async_channel`. Emits `NotificationClosed`, `ActionInvoked`, `ActivationToken`. |
| `control.rs` | zbus `org.opusdm.Notifications1` at `/org/opusdm/Notifications1`, same connection (well-known name `org.opusdm.Notifications1`): `GetState` (DND, unread, history newest first, muted apps, in one call), `ClearHistory`, `Remove(id)`, `Activate(id)`, `MarkRead`, `SetDnd(bool)`, `Mute(app)`, `Unmute(app)`, signal `Changed` (no arguments: the panel calls `GetState` again). Names and tuple types in `opusdm_ipc::notify`. |
| `markup.rs` | Body markup whitelist and plain text for the history. |
| `placement.rs` | Pure geometry: corner, stack positions inside a work area, `MAX_VISIBLE = 5`. |
| `popup.rs` | One GTK window per popup: icon, app name, time, summary, body, action buttons, close button; timers, hover pause, stacking, queue. Wayland through `opusdm_ui::layer` (overlay layer, no keyboard, the compositor picks the monitor). X11: `_NET_WM_WINDOW_TYPE_NOTIFICATION` set at realize; above, sticky, skip taskbar / pager asked with EWMH messages after map (GDK rewrites `_NET_WM_STATE` when it maps); `pin_geometry` + `retry_after_map`; the area is the monitor under the pointer minus the struts (`opusdm_snap::x11::Display`). |
| `main.rs` | GTK application, connects the parts. |

Panel: `crates/panel/src/widgets/notifications.rs`, a bell with an unread badge
(`9+` above nine) and a struck bell when DND is on. Left click: popover with a
DND switch, the history (newest first) and "Clear all"; opening it marks all
read. Click on an entry runs `Activate`; right click: "Remove", "Mute <app>" /
"Unmute <app>". Middle click on the bell toggles DND. When the service is not
running the bell is dimmed and the popover says so; it follows the owner with
`NameOwnerChanged`.

### Lifecycle

1. `Notify` gets an id (counter from 1, never 0, wraps), or keeps
   `replaces_id` when that notification is live or in the history.
2. Routing (`store::decide`):
   - critical urgency: popup always (also with DND), no timeout;
   - DND on or app muted (case-insensitive `app_name`): history only;
   - `transient`: popup only, never history;
   - otherwise popup and history.
3. A replaced notification updates its popup in place and its history entry.
4. Timeout: `-1` means the configured default (`timeout_ms`, 6000) or 3000 for
   low urgency; `0` never expires; `> 0` as given. Hovering pauses it.
5. `NotificationClosed(id, reason)` exactly once per id, when it is neither
   on screen nor in the history: 1 expired (a popup that was not kept, or
   the oldest entry pushed out of a full history), 2 dismissed by the user
   (popup closed, action run, removed or cleared from the history),
   3 `CloseNotification`. An expired popup that went into the history stays
   open, so a click in the history can still reach the application.
6. Click on the popup body runs the `default` action. Other actions become
   buttons (at most three). Each invocation emits `ActivationToken` (a GDK
   launch context token, for Wayland focus) and then `ActionInvoked`; the
   notification then closes with reason 2 unless `resident`.
7. `Activate(id)` from the history runs the default action like a click on
   the popup (the signals are broadcast: an application that is gone simply
   does not hear them).

Capabilities: `actions`, `body`, `body-markup`, `body-hyperlinks`,
`icon-static`, `persistence`. Body markup keeps `b`, `i`, `u` and `a href`;
everything else is escaped before `set_markup`.

### Threads

zbus runs both interfaces on its own executor. The store sits behind a
`Mutex` shared by both sides, so a D-Bus call never waits on the GTK loop.
What the screen must change goes to the GTK loop over an `async_channel`
(`Show`, `Hide`, `Invoke`); signals caused by the user (expired, dismissed,
action) are sent by the GTK loop on the blocking connection.

### Tests

- Unit: hints and `image-data` (bad stride, short buffer), markup whitelist,
  routing table, history cap and replace, unread count, timeouts, placement for
  the four corners and the queue.
- Live, `#[ignore]`, inside `dbus-run-session`: `Notify` / `replaces_id`,
  `CloseNotification` emits `NotificationClosed(3)`, capabilities, server
  information, control interface.
- Manual: Brave test page, `notify-send -u critical`, actions through
  `gdbus call` (Ubuntu's AppArmor profile of `notify-send` allows only
  `GetServerInformation` and `Notify`, so `notify-send -A` never sees the
  `actions` capability), WhatsApp Web, on X11 and Wayland.

### Out of scope

Sound, a Settings page (corner / timeout are edited in the JSON file),
persistent history, per-app rules beyond mute, animations beyond a fade-in.
See `NEXT-PHASE.md`.
