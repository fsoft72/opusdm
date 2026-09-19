//! StatusNotifierItem (SNI) tray: a background thread (`spawn`) either runs
//! `org.kde.StatusNotifierWatcher` itself or, if one is already running,
//! connects to it as a host - then tracks every registered item's icon,
//! title and menu path. Companion to `systray.rs` (XEmbed); together they
//! cover both tray protocols apps use in the wild.

use std::sync::mpsc;

use crate::ewmh::IconRgba;

/// The state the GTK side needs to render one hosted item's icon and know
/// whether clicking it should show a DBusMenu.
#[derive(Clone, Debug)]
pub struct SniItem {
    pub icon: Option<IconRgba>,
    pub icon_name: Option<String>,
    pub title: String,
    pub item_is_menu: bool,
    pub menu_path: Option<zbus::zvariant::OwnedObjectPath>,
}

/// Snapshots the SNI thread sends to the GTK side, keyed by the item's
/// DBus service name (unique per registered item).
#[derive(Clone, Debug)]
pub enum SniEvent {
    ItemAdded { service: String, item: SniItem },
    ItemChanged { service: String, item: SniItem },
    ItemRemoved { service: String },
}

/// Commands the GTK side sends to the SNI thread.
#[derive(Clone, Debug)]
pub enum SniCmd {
    Activate { service: String, x: i32, y: i32 },
    ContextMenu { service: String, x: i32, y: i32 },
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
                eprintln!("opusdm-panel: SNI tray stopped: {e}");
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

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(&self, service: &str) {
        let already = {
            let mut s = self.state.lock().unwrap();
            if s.items.iter().any(|i| i == service) {
                true
            } else {
                s.items.push(service.to_string());
                false
            }
        };
        if already {
            return;
        }
        if let Ok(item) = fetch_item(service) {
            let _ = self.tx.send_blocking(SniEvent::ItemAdded { service: service.to_string(), item });
            watch_item(service.to_string(), self.tx.clone());
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
fn fetch_item(service: &str) -> Result<SniItem, Box<dyn std::error::Error>> {
    let conn = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(&conn, service, "/StatusNotifierItem", "org.kde.StatusNotifierItem")?;

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

    Ok(SniItem { icon, icon_name, title, item_is_menu, menu_path })
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
fn watch_item(service: String, tx: async_channel::Sender<SniEvent>) {
    std::thread::spawn(move || {
        let Ok(conn) = zbus::blocking::Connection::session() else { return };
        let Ok(dbus_proxy) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
        let Ok(mut owner_changes) = dbus_proxy.receive_name_owner_changed() else { return };

        let Ok(props_proxy) = zbus::blocking::Proxy::new(
            &conn,
            service.as_str(),
            "/StatusNotifierItem",
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
            match fetch_item(&service) {
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
            eprintln!("opusdm-panel: SNI watcher already running, connecting as host only");
            return run_as_host_client(cmd_rx, tx);
        }
    };

    event_loop(&conn, &state, &cmd_rx, &tx)
}

/// Fallback when another process already owns the watcher name: just
/// process commands (no items to track ourselves - the design's non-goal
/// "never runs two watchers" means we don't duplicate its registry here).
fn run_as_host_client(
    cmd_rx: mpsc::Receiver<SniCmd>,
    tx: async_channel::Sender<SniEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = zbus::blocking::Connection::session()?;
    let state = Arc::new(Mutex::new(WatcherState::default()));
    event_loop(&conn, &state, &cmd_rx, &tx)
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
            Ok(SniCmd::Activate { service, x, y }) => send_activate(conn, &service, x, y),
            Ok(SniCmd::ContextMenu { service, x, y }) => send_context_menu(conn, &service, x, y),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn send_activate(conn: &zbus::blocking::Connection, service: &str, x: i32, y: i32) {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let proxy = zbus::blocking::Proxy::new(conn, service, "/StatusNotifierItem", "org.kde.StatusNotifierItem")?;
        proxy.call_method("Activate", &(x, y))?;
        Ok(())
    };
    if let Err(e) = run() {
        eprintln!("opusdm-panel: SNI Activate failed for {service}: {e}");
    }
}

fn send_context_menu(conn: &zbus::blocking::Connection, service: &str, x: i32, y: i32) {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let proxy = zbus::blocking::Proxy::new(conn, service, "/StatusNotifierItem", "org.kde.StatusNotifierItem")?;
        proxy.call_method("ContextMenu", &(x, y))?;
        Ok(())
    };
    if let Err(e) = run() {
        eprintln!("opusdm-panel: SNI ContextMenu failed for {service}: {e}");
    }
}
