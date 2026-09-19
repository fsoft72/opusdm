# OpusDM Panel StatusNotifierItem (SNI) tray - design

Status: approved 2026-09-19. Adds StatusNotifierItem/DBus tray support to
`opusdm-panel`, alongside the existing XEmbed tray
(`docs/superpowers/specs/2026-09-19-systray-design.md`). Many current apps
(including `nm-applet` on modern distros) register only via SNI, not
XEmbed - this closes that gap.

## Goals

- opusdm-panel implements `org.kde.StatusNotifierWatcher` itself (the DBus
  registry apps register with) - no other watcher runs on this WM.
- Apps register a `org.kde.StatusNotifierItem` and get rendered as an icon
  next to the existing XEmbed tray icons.
- Full `com.canonical.dbusmenu` client rendering: most SNI apps set
  `ItemIsMenu=true` and are unusable without their actual menu (e.g.
  nm-applet's network list), so clicking must show the real menu, not just
  call `Activate()`.
- An item vanishing from the bus (app exit/crash) removes its icon cleanly.

## Non-goals

- Rendering DBusMenu toggle/radio state (checkmarks) - items still work,
  just show as plain clickable rows. Matches the XEmbed spec's own v1 cuts.
- Tooltips (`ToolTip` property), `Status` (Active/Passive/NeedsAttention)
  visual distinction, icon theme "overlay"/"attention" icon variants -
  render every item the same way regardless of status.
- Acting as a `StatusNotifierHost` for some *other* watcher process - if
  the watcher name is already taken (another host running), opusdm-panel
  connects as a host to it instead of standing up its own registry; it
  never runs two watchers.
- Merging XEmbed and SNI icons into one interleaved, arrival-ordered strip
  - they render as two adjacent groups (XEmbed icons, then SNI icons).

## Architecture

Three new files in `crates/panel/src`, following the established
protocol-thread / GTK-widget split:

| file | responsibility |
|---|---|
| `sni.rs` | DBus thread (`zbus::blocking`): watcher registry, per-item property proxies, DBus-name-loss cleanup |
| `dbusmenu.rs` | `com.canonical.dbusmenu` client: fetch layout, build a GTK popover, dispatch clicks |
| `widgets/sni.rs` | one clickable icon per hosted item; owns the popover shown on click |

**New dependency**: `zbus` (blocking feature only - no async runtime, so the
thread reads like `ewmh.rs`/`systray.rs`: a loop making blocking DBus calls).

`lib.rs`: right box gains `widgets::sni::build()` next to the existing
`widgets::tray::build()` (XEmbed), both primary-bar-only.

### Channels (mirrors `systray.rs`'s `TrayCmd`/`TrayEvent`)

```rust
enum SniEvent {
    ItemAdded { service: String, item: SniItem },
    ItemChanged { service: String, item: SniItem },  // icon/title/menu-path updated
    ItemRemoved { service: String },
}

struct SniItem {
    icon: Option<IconRgba>,   // reuses ewmh.rs's IconRgba (same ARGB8 shape as IconPixmap)
    icon_name: Option<String>,
    title: String,
    item_is_menu: bool,
    menu_path: Option<zbus::zvariant::OwnedObjectPath>,
}

enum SniCmd {
    Activate { service: String, x: i32, y: i32 },
    ContextMenu { service: String, x: i32, y: i32 },
}
```

`sni::spawn() -> (Sender<SniCmd>, Receiver<SniEvent>)`, same shape as
`systray::spawn`.

## DBus thread (`sni.rs`)

**Startup**: connect to the session bus (`zbus::blocking::Connection::session()`).
Try to own the well-known name `org.kde.StatusNotifierWatcher`. If taken
(another host's watcher is already running): connect as a plain client
instead - call that watcher's `RegisteredStatusNotifierItems` once to seed
the current list, and subscribe to its `StatusNotifierItemRegistered`/
`Unregistered` signals to stay in sync, skipping the "run our own registry"
steps below entirely.

**If we own the name**: register the `org.kde.StatusNotifierWatcher`
interface (via zbus's `#[interface]`) exposing:
- `RegisterStatusNotifierItem(service: &str)` - resolves `service` to a bus
  name + object path (per spec: either a bus name, when the item is at the
  well-known root path, or `bus_name/object/path`), stores it, fires
  `StatusNotifierItemRegistered(service)`, and spawns a property-fetch +
  `PropertiesChanged` subscription for that item's `org.kde.StatusNotifierItem`
  interface.
- `RegisterStatusNotifierHost(service: &str)` - records a host (accepted,
  but with only one process ever calling this in practice, this exists for
  spec completeness so items that gate on `IsStatusNotifierHostRegistered`
  behave correctly).
- Properties: `RegisteredStatusNotifierItems -> Vec<String>`,
  `IsStatusNotifierHostRegistered -> bool` (true once we've registered
  ourselves as a host, which happens unconditionally right after startup).

**Per item**: build an `org.kde.StatusNotifierItem` proxy on `service`, read
`IconPixmap` (reuse `ewmh::pick_icon`'s unpack logic - same wire shape:
repeated `(width, height, ARGB32 pixels)`), `IconName`, `Title`,
`ItemIsMenu`, `Menu` (object path). Subscribe to that item's
`org.freedesktop.DBus.Properties.PropertiesChanged` to re-fetch on icon/
title changes, sending `SniEvent::ItemChanged`. Subscribe to
`org.freedesktop.DBus.NameOwnerChanged` (matching `service`'s bus name)
to detect the app exiting and send `SniEvent::ItemRemoved`.

**Commands**: `SniCmd::Activate`/`ContextMenu` call the matching method on
the item's proxy. Showing the DBusMenu popover is *not* a command on this
channel - the widget calls into `dbusmenu.rs` directly on the GTK thread
(short blocking DBus round-trip, same style as `ewmh.rs`'s click actions),
since building the popover needs GTK, not this thread. `SniCmd` therefore
only has the two variants shown above.

## DBusMenu client (`dbusmenu.rs`)

```rust
pub struct MenuNode {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub is_separator: bool,
    pub children: Vec<MenuNode>,
}

pub fn fetch_layout(conn: &zbus::blocking::Connection, service: &str, path: &ObjectPath) -> Result<MenuNode, zbus::Error>;
pub fn send_event_clicked(conn: &zbus::blocking::Connection, service: &str, path: &ObjectPath, id: i32);
pub fn build_popover(root: &MenuNode, on_click: impl Fn(i32) + Clone + 'static) -> gtk::PopoverMenu;
```

`fetch_layout` calls `AboutToShow(0)` then `GetLayout(0, -1, [])`, parses the
zvariant tuple `(revision, (id, properties: HashMap<String, Value>, children: Vec<Variant>))`
recursively into `MenuNode` (`type=="separator"` → `is_separator`,
`label`/`enabled`/`visible` pulled from `properties` with sane defaults
when absent, matching the spec's "assume true/empty" fallback rules).

`build_popover` builds a `gtk::PopoverMenu` from `gio::Menu` +
`gio::SimpleActionGroup`, one action per non-separator leaf/branch id;
activating an action calls `on_click(id)`, which the widget wires to
`send_event_clicked` followed by closing the popover.

## Widget (`widgets/sni.rs`)

```rust
pub fn build(cmd_tx: Sender<SniCmd>) -> gtk::Widget
```

Holds `Rc<RefCell<HashMap<String, gtk::Button>>>` keyed by DBus service
name. On `SniEvent::ItemAdded`/`Changed`: build/update a small flat button
containing a `gtk::Image` (from `IconRgba` via `gdk::gdk_pixbuf::Pixbuf::from_bytes`,
falling back to `gtk::IconTheme::default().lookup_icon(icon_name, ...)` when
there's no pixmap), append if new. On `SniEvent::ItemRemoved`: remove the
button.

Click handling (`gtk::GestureClick`, both buttons 1 and 3): if the item has
a menu path, call `dbusmenu::fetch_layout` synchronously (short-lived,
same throwaway-blocking-call style as `ewmh.rs`'s click actions) and show
`dbusmenu::build_popover`'s result as a popover anchored to the button;
otherwise send `SniCmd::Activate` (button 1) or `SniCmd::ContextMenu`
(button 3) over `cmd_tx`.

## Error handling

- No session bus available: `sni::spawn` logs once and returns (channels
  silent), matching `systray.rs`'s no-X11 fallback.
- Watcher name already owned: connect as host-only client, logged once at
  info level (not an error - expected when another tray host runs).
- Any single item's DBus calls failing (crashed mid-property-fetch, menu
  fetch timeout, etc.): log via `eprintln!` and skip that item/click;
  never let one bad item stop the thread, matching `systray.rs`'s
  per-icon error isolation.

## Testing

Pure helpers get unit tests, matching the rest of the panel:
- `dbusmenu`'s zvariant-tuple → `MenuNode` parser (feed it hand-built
  `Value`/`Variant` trees, assert the resulting tree shape - this is the
  one piece with real parsing logic and no live DBus needed).
- Icon pixmap unpacking reuses `ewmh::pick_icon`, already tested there.

The live DBus protocol (watcher registration, property fetch, signal
subscriptions, menu round-trips) is exercised manually against a running
session bus - no automated coverage, same rationale as the XEmbed tray's
live X11 protocol.
