//! StatusNotifierItem (SNI) tray: a background thread (`spawn`) either runs
//! `org.kde.StatusNotifierWatcher` itself or, if one is already running,
//! connects to it as a host - then tracks every registered item's icon,
//! title and menu path. Companion to `systray.rs` (XEmbed); together they
//! cover both tray protocols apps use in the wild.

use std::sync::mpsc;

use crate::ewmh::IconRgba;

/// The state the GTK side needs to render one hosted item's icon and know
/// whether clicking it should show a DBusMenu. `path` is the item's actual
/// `org.kde.StatusNotifierItem` object path on `service` - apps built on
/// libappindicator/ayatana register a custom path (e.g.
/// `/org/ayatana/NotificationItem/nm_applet`), not the spec's default
/// `/StatusNotifierItem`, so this can't be assumed and must be carried
/// alongside every item.
#[derive(Clone, Debug)]
pub struct SniItem {
    pub icon: Option<IconRgba>,
    pub icon_name: Option<String>,
    pub title: String,
    pub item_is_menu: bool,
    pub menu_path: Option<zbus::zvariant::OwnedObjectPath>,
    pub path: String,
}

/// Snapshots the SNI thread sends to the GTK side, keyed by the item's
/// resolved DBus bus name (unique per registered item - see
/// `resolve_service`).
#[derive(Clone, Debug)]
pub enum SniEvent {
    ItemAdded { service: String, item: SniItem },
    ItemChanged { service: String, item: SniItem },
    ItemRemoved { service: String },
}

/// Commands the GTK side sends to the SNI thread. `path` is the item's
/// `SniItem::path`, since `Activate`/`ContextMenu` are called on the
/// item's actual object, not a spec-default one.
#[derive(Clone, Debug)]
pub enum SniCmd {
    Activate { service: String, path: String, x: i32, y: i32 },
    ContextMenu { service: String, path: String, x: i32, y: i32 },
}

/// Start the SNI thread. Returns a command sender and event receiver; if no
/// session bus is available, the thread logs once and exits - the channels
/// stay open but nothing is ever sent or received. Mirrors `systray::spawn`.
pub fn spawn() -> (mpsc::Sender<SniCmd>, async_channel::Receiver<SniEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<SniCmd>();
    let (evt_tx, evt_rx) = async_channel::unbounded::<SniEvent>();
    std::thread::Builder::new()
        .name("opusdm-panel-sni".into())
        .spawn(move || {
            if let Err(e) = run(cmd_rx, evt_tx) {
                opusdm_ipc::err_log!("opusdm-panel", "SNI tray stopped: {e}");
            }
        })
        .expect("spawn sni thread");
    (cmd_tx, evt_rx)
}

use std::sync::{Arc, Mutex};

/// Shared registry the watcher interface mutates and the event loop reads.
/// `Mutex` (not `RefCell`) because zbus's interface methods run on its own
/// internal executor thread, separate from this module's own loop below.
#[derive(Default)]
struct WatcherState {
    items: Vec<String>, // registered service names
}

struct Watcher {
    state: Arc<Mutex<WatcherState>>,
    tx: async_channel::Sender<SniEvent>,
}

/// Resolve a `RegisterStatusNotifierItem` argument into `(bus_name,
/// object_path)`, per the freedesktop spec: an argument starting with `/`
/// is an object path *on the calling connection* (`sender`'s unique name);
/// anything else is a bus name, and the item is assumed to live at the
/// spec-default `/StatusNotifierItem`.
fn resolve_service(service: &str, sender: &str) -> (String, String) {
    if let Some(path) = service.strip_prefix('/') {
        (sender.to_string(), format!("/{path}"))
    } else {
        (service.to_string(), "/StatusNotifierItem".to_string())
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        let sender = header.sender().map(|n| n.to_string()).unwrap_or_default();
        let (service, path) = resolve_service(service, &sender);

        let already = {
            let mut s = self.state.lock().unwrap();
            if s.items.iter().any(|i| i == &service) {
                true
            } else {
                s.items.push(service.clone());
                false
            }
        };
        if already {
            return;
        }
        match fetch_item(&service, &path) {
            Ok(item) => {
                let _ = self.tx.send_blocking(SniEvent::ItemAdded { service: service.clone(), item });
                watch_item(service, path, self.tx.clone());
            }
            Err(e) => opusdm_ipc::err_log!("opusdm-panel", "cannot fetch SNI item {service} at {path}: {e}"),
        }
    }

    async fn register_status_notifier_host(&self, _service: &str) {
        // Accepted for spec completeness; see the design doc's note on
        // `IsStatusNotifierHostRegistered`.
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.state.lock().unwrap().items.clone()
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }
}

/// Build a blocking proxy to `service`'s `org.kde.StatusNotifierItem`
/// interface and read the properties the widget needs. A fresh short-lived
/// connection per call, same throwaway style as `ewmh.rs`'s click actions -
/// this only runs once per (un)registration, not per frame.
fn fetch_item(service: &str, path: &str) -> Result<SniItem, Box<dyn std::error::Error>> {
    let conn = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(&conn, service, path, "org.kde.StatusNotifierItem")?;

    let title: String = proxy.get_property("Title").unwrap_or_default();
    let item_is_menu: bool = proxy.get_property("ItemIsMenu").unwrap_or(false);
    let icon_name: Option<String> = proxy.get_property::<String>("IconName").ok().filter(|s| !s.is_empty());
    let menu_path: Option<zbus::zvariant::OwnedObjectPath> = proxy.get_property("Menu").ok();

    // IconPixmap: array of (i32 width, i32 height, Vec<u8> ARGB32 rows),
    // same wire shape `ewmh::pick_icon` already unpacks for _NET_WM_ICON,
    // but SNI hands it over pre-split per-icon rather than as one flat
    // word stream - convert the closest (to 24px) entry directly here.
    let icon = proxy
        .get_property::<Vec<(i32, i32, Vec<u8>)>>("IconPixmap")
        .ok()
        .and_then(|pixmaps| pick_sni_icon(&pixmaps, 24));

    Ok(SniItem { icon, icon_name, title, item_is_menu, menu_path, path: path.to_string() })
}

/// Pick the pixmap closest to `target` px and unpack its big-endian ARGB32
/// bytes into tightly packed RGBA8 - mirrors `ewmh::pick_icon`'s pixel
/// unpacking, adapted to SNI's already-split-by-icon wire shape.
fn pick_sni_icon(pixmaps: &[(i32, i32, Vec<u8>)], target: i32) -> Option<IconRgba> {
    let (w, h, bytes) = pixmaps.iter().min_by_key(|(w, _, _)| (*w - target).abs())?;
    let mut rgba = Vec::with_capacity(bytes.len());
    for px in bytes.chunks_exact(4) {
        let (a, r, g, b) = (px[0], px[1], px[2], px[3]);
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Some(IconRgba { width: *w, height: *h, rgba })
}

/// Watch one item's DBus name for loss (app exit) and its properties for
/// changes, for as long as the item stays registered. Runs on its own
/// thread since each needs an independent blocking signal stream; sends
/// `ItemChanged`/`ItemRemoved` back through `tx`.
fn watch_item(service: String, path: String, tx: async_channel::Sender<SniEvent>) {
    std::thread::spawn(move || {
        let Ok(conn) = zbus::blocking::Connection::session() else { return };
        let Ok(dbus_proxy) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
        let Ok(mut owner_changes) = dbus_proxy.receive_name_owner_changed() else { return };

        let Ok(props_proxy) = zbus::blocking::Proxy::new(
            &conn,
            service.as_str(),
            path.as_str(),
            "org.freedesktop.DBus.Properties",
        ) else {
            return;
        };
        let Ok(mut changes) = props_proxy.receive_signal("PropertiesChanged") else { return };

        loop {
            // Either signal stream waking up means "re-check this item":
            // a real removal will fail `fetch_item` on the next iteration
            // once the name is fully gone, which is simpler than parsing
            // the NameOwnerChanged/PropertiesChanged payloads themselves.
            if owner_changes.next().is_none() && changes.next().is_none() {
                break;
            }
            match fetch_item(&service, &path) {
                Ok(item) => {
                    let _ = tx.send_blocking(SniEvent::ItemChanged { service: service.clone(), item });
                }
                Err(_) => {
                    let _ = tx.send_blocking(SniEvent::ItemRemoved { service: service.clone() });
                    break;
                }
            }
        }
    });
}

fn run(
    cmd_rx: mpsc::Receiver<SniCmd>,
    tx: async_channel::Sender<SniEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(Mutex::new(WatcherState::default()));
    let watcher = Watcher { state: state.clone(), tx: tx.clone() };

    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name("org.kde.StatusNotifierWatcher"))
        .and_then(|b| b.serve_at("/StatusNotifierWatcher", watcher))
        .and_then(|b| b.build());

    let conn = match conn {
        Ok(conn) => conn,
        Err(_) => {
            opusdm_ipc::err_log!("opusdm-panel", "SNI watcher already running, connecting as host only");
            return run_as_host_client(cmd_rx, tx);
        }
    };

    event_loop(&conn, &state, &cmd_rx, &tx)
}

/// Split one of an *existing* watcher's `RegisteredStatusNotifierItems`
/// entries into `(bus_name, object_path)`. Established watchers store
/// items pre-joined as `"<bus_name>/<path-without-leading-slash>"` (e.g.
/// `":1.81/StatusNotifierItem"` or
/// `"org.kde.StatusNotifierItem-4449-1/StatusNotifierItem"`) - this is the
/// read side of that convention; `resolve_service` (above) is the write
/// side for when *we* are the watcher being registered with.
fn split_registered_item(entry: &str) -> (String, String) {
    match entry.split_once('/') {
        Some((bus, rest)) => (bus.to_string(), format!("/{rest}")),
        None => (entry.to_string(), "/StatusNotifierItem".to_string()),
    }
}

/// Fallback when another process already owns the watcher name: connect to
/// it as a plain `StatusNotifierHost` - seed from its current
/// `RegisteredStatusNotifierItems`, then stay in sync via its
/// `StatusNotifierItemRegistered`/`Unregistered` signals. We never run a
/// second registry (the design's "never runs two watchers" non-goal), but
/// we still need to actually show what the real one is hosting - a silent
/// fallback here is why an app's icon can vanish into someone else's tray
/// (e.g. the host desktop's own) with opusdm-panel showing nothing.
fn run_as_host_client(
    cmd_rx: mpsc::Receiver<SniCmd>,
    tx: async_channel::Sender<SniEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = zbus::blocking::Connection::session()?;
    let watcher = zbus::blocking::Proxy::new(
        &conn,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )?;

    if let Ok(items) = watcher.get_property::<Vec<String>>("RegisteredStatusNotifierItems") {
        for entry in items {
            let (service, path) = split_registered_item(&entry);
            match fetch_item(&service, &path) {
                Ok(item) => {
                    let _ = tx.send_blocking(SniEvent::ItemAdded { service: service.clone(), item });
                    watch_item(service, path, tx.clone());
                }
                Err(e) => opusdm_ipc::err_log!("opusdm-panel", "cannot fetch existing SNI item {service} at {path}: {e}"),
            }
        }
    }

    watch_host_registrations(watcher, tx.clone());

    let state = Arc::new(Mutex::new(WatcherState::default()));
    event_loop(&conn, &state, &cmd_rx, &tx)
}

/// Subscribe to the real watcher's registration signals for as long as the
/// session bus connection lives, so items docked/withdrawn after our
/// initial seed still show up. Runs on its own thread - a blocking signal
/// stream, same pattern as `watch_item`.
fn watch_host_registrations(watcher: zbus::blocking::Proxy<'static>, tx: async_channel::Sender<SniEvent>) {
    std::thread::spawn(move || {
        let Ok(mut registered) = watcher.receive_signal("StatusNotifierItemRegistered") else { return };
        let Ok(mut unregistered) = watcher.receive_signal("StatusNotifierItemUnregistered") else { return };

        loop {
            if let Some(msg) = registered.next() {
                if let Ok(entry) = msg.body().deserialize::<String>() {
                    let (service, path) = split_registered_item(&entry);
                    match fetch_item(&service, &path) {
                        Ok(item) => {
                            let _ = tx.send_blocking(SniEvent::ItemAdded { service: service.clone(), item });
                            watch_item(service, path, tx.clone());
                        }
                        Err(e) => {
                            opusdm_ipc::err_log!("opusdm-panel", "cannot fetch newly-registered SNI item {service} at {path}: {e}")
                        }
                    }
                }
                continue;
            }
            if let Some(msg) = unregistered.next() {
                if let Ok(entry) = msg.body().deserialize::<String>() {
                    let (service, _path) = split_registered_item(&entry);
                    let _ = tx.send_blocking(SniEvent::ItemRemoved { service });
                }
                continue;
            }
            break;
        }
    });
}

/// Poll interval for draining `SniCmd`s; the watcher interface itself is
/// served by zbus's own internal executor, so this loop only needs to
/// forward GTK-side commands to DBus method calls.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

fn event_loop(
    conn: &zbus::blocking::Connection,
    _state: &Arc<Mutex<WatcherState>>,
    cmd_rx: &mpsc::Receiver<SniCmd>,
    _tx: &async_channel::Sender<SniEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        match cmd_rx.recv_timeout(POLL_INTERVAL) {
            Ok(SniCmd::Activate { service, path, x, y }) => send_activate(conn, &service, &path, x, y),
            Ok(SniCmd::ContextMenu { service, path, x, y }) => {
                send_context_menu(conn, &service, &path, x, y)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn send_activate(conn: &zbus::blocking::Connection, service: &str, path: &str, x: i32, y: i32) {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let proxy = zbus::blocking::Proxy::new(conn, service, path, "org.kde.StatusNotifierItem")?;
        proxy.call_method("Activate", &(x, y))?;
        Ok(())
    };
    if let Err(e) = run() {
        opusdm_ipc::err_log!("opusdm-panel", "SNI Activate failed for {service}: {e}");
    }
}

fn send_context_menu(conn: &zbus::blocking::Connection, service: &str, path: &str, x: i32, y: i32) {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let proxy = zbus::blocking::Proxy::new(conn, service, path, "org.kde.StatusNotifierItem")?;
        proxy.call_method("ContextMenu", &(x, y))?;
        Ok(())
    };
    if let Err(e) = run() {
        opusdm_ipc::err_log!("opusdm-panel", "SNI ContextMenu failed for {service}: {e}");
    }
}
