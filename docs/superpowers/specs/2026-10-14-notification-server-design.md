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
| `control.rs` | zbus `org.opusdm.Notifications1` at `/org/opusdm/Notifications1`, same connection: `GetHistory`, `ClearHistory`, `Remove(id)`, `Activate(id)`, `MarkRead`, `SetDnd(bool)`, `Mute(app)`, `Unmute(app)`, properties `Dnd`, `Unread`, `Muted`, signal `Changed(unread)`. |
| `placement.rs` | Pure geometry: corner, stack positions inside a work area, `MAX_VISIBLE = 5`. |
| `popup.rs` | One GTK window per popup: icon, app name, time, summary, body, action buttons, close button; timers, hover pause, stacking, queue. Wayland through `opusdm_ui::layer` (overlay layer, no keyboard), X11 through `_NET_WM_WINDOW_TYPE_NOTIFICATION`, above, skip taskbar / pager, no input focus, `configure_window` + `retry_after_map`. |
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
5. `NotificationClosed(id, reason)` exactly once per id: 1 expired,
   2 dismissed by the user, 3 `CloseNotification`. An expired popup stays in
   the history.
6. Click on the popup body runs the `default` action. Other actions become
   buttons (at most three). Each invocation emits `ActivationToken` (a GDK
   launch context token, for Wayland focus) and then `ActionInvoked`; the
   notification then closes with reason 2 unless `resident`.
7. `Activate(id)` from the history emits the default action when the sender
   still owns its unique name on the bus; otherwise nothing happens.

Capabilities: `actions`, `body`, `body-markup`, `body-hyperlinks`,
`icon-static`, `persistence`. Body markup keeps `b`, `i`, `u` and `a href`;
everything else is escaped before `set_markup`.

### Threads

zbus runs both interfaces on its own executor. Messages go to the GTK main loop
over an `async_channel`; replies that need GTK state (history, DND) are read
from a `Mutex`-guarded shared store snapshot so a D-Bus call never waits on the
GTK loop.

### Tests

- Unit: hints and `image-data` (bad stride, short buffer), markup whitelist,
  routing table, history cap and replace, unread count, timeouts, placement for
  the four corners and the queue.
- Live, `#[ignore]`, inside `dbus-run-session`: `Notify` / `replaces_id`,
  `CloseNotification` emits `NotificationClosed(3)`, capabilities, server
  information, control interface.
- Manual: Brave test page, `notify-send -u critical`, `notify-send -A ok=OK`,
  WhatsApp Web, on X11 and Wayland.

### Out of scope

Sound, a Settings page (corner / timeout are edited in the JSON file),
persistent history, per-app rules beyond mute, animations beyond a fade.
