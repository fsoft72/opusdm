//! StatusNotifierItem (SNI) tray: a background thread (`spawn`) either runs
//! `org.kde.StatusNotifierWatcher` itself or, if one is already running,
//! connects to it as a host - then tracks every registered item's icon,
//! title and menu path. Companion to `systray.rs` (XEmbed); together they
//! cover both tray protocols apps use in the wild.
//!
//! Threads, all long lived except the per-item workers:
//! - the watcher interface runs on zbus's own executor and never blocks: a
//!   registration only records the item and hands the slow work to a worker;
//! - one dispatcher (`dispatch`) reads every signal the panel subscribed to
//!   (item changes, owners leaving, the external watcher's registrations) on
//!   a dedicated connection and turns them into wake-ups or add/remove jobs;
//! - one worker per item (`item_worker`) reads the item's properties, retries
//!   while the app is too busy to answer, and re-reads on every change
//!   signal, at most once per `REFRESH_INTERVAL`;
//! - the command loop (`command_loop`) forwards clicks to the apps.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::ewmh::IconRgba;

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
const DBUS_NAME: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// Upper bound for one method call to an app. An app that is busy (MEGAsync
/// takes about 25 s to answer while it starts) or hung must not keep a
/// worker or the command loop waiting forever; dbus-broker, unlike
/// dbus-daemon, enforces no reply timeout of its own.
const METHOD_TIMEOUT: Duration = Duration::from_secs(30);

/// Minimum time between two property reads of one item. Some apps animate
/// their icon (MEGAsync sends `NewIcon` every 60 ms while it syncs), so the
/// change signals are coalesced instead of each one costing a round trip.
const REFRESH_INTERVAL: Duration = Duration::from_millis(150);

/// Longest wait between two attempts to read an item that has not answered
/// yet; see `retry_delay`.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(15);

/// Icon size (px) the closest `IconPixmap` entry is picked for.
const ICON_TARGET_SIZE: i32 = 24;

/// The state the GTK side needs to render one hosted item's icon and know
/// whether clicking it should show a DBusMenu. `service` is the bus name the
/// item lives on and `path` its actual `org.kde.StatusNotifierItem` object
/// path there - apps built on libappindicator/ayatana register a custom path
/// (e.g. `/org/ayatana/NotificationItem/nm_applet`), not the spec's default
/// `/StatusNotifierItem`, so this can't be assumed and must be carried
/// alongside every item.
#[derive(Clone, Debug)]
pub struct SniItem {
    pub icon: Option<IconRgba>,
    pub icon_name: Option<String>,
    pub title: String,
    pub item_is_menu: bool,
    pub menu_path: Option<OwnedObjectPath>,
    pub service: String,
    pub path: String,
}

/// Snapshots the SNI thread sends to the GTK side, keyed by the item's id
/// (`item_id`: bus name plus object path, unique per registered item).
#[derive(Clone, Debug)]
pub enum SniEvent {
    Added { id: String, item: SniItem },
    Changed { id: String, item: SniItem },
    Removed { id: String },
}

/// Commands the GTK side sends to the SNI thread. `service`/`path` are the
/// item's `SniItem::service`/`SniItem::path`, since `Activate`/`ContextMenu`
/// are called on the item's actual object, not a spec-default one.
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

// --------------------------------------------------------------------------
// Pure helpers
// --------------------------------------------------------------------------

/// Resolve a `RegisterStatusNotifierItem` argument into `(bus_name,
/// object_path)`, per the freedesktop spec: an argument starting with `/`
/// is an object path *on the calling connection* (`sender`'s unique name);
/// anything else is a bus name, and the item is assumed to live at the
/// spec-default `/StatusNotifierItem`.
fn resolve_service(service: &str, sender: &str) -> (String, String) {
    if service.starts_with('/') {
        (sender.to_string(), service.to_string())
    } else {
        (service.to_string(), DEFAULT_ITEM_PATH.to_string())
    }
}

/// Split one of a watcher's `RegisteredStatusNotifierItems` entries into
/// `(bus_name, object_path)`. Watchers store items pre-joined as
/// `"<bus_name>/<path-without-leading-slash>"` (e.g.
/// `":1.81/StatusNotifierItem"` or
/// `"org.kde.StatusNotifierItem-4449-1/StatusNotifierItem"`); a bare bus
/// name means the spec-default path. `item_id` is the inverse.
fn split_registered_item(entry: &str) -> (String, String) {
    match entry.split_once('/') {
        Some((bus, rest)) => (bus.to_string(), format!("/{rest}")),
        None => (entry.to_string(), DEFAULT_ITEM_PATH.to_string()),
    }
}

/// The id of one item: `"<bus_name><object_path>"`, the same joined form
/// watchers publish in `RegisteredStatusNotifierItems`. Two items of one
/// app on different paths get different ids.
fn item_id(service: &str, path: &str) -> String {
    format!("{service}{path}")
}

/// True for the signals that mean an item's displayed state changed: the
/// SNI spec's own `New*` signals (what Qt, Electron and libappindicator
/// send) and the generic `PropertiesChanged` some other toolkits send.
fn is_item_change_signal(interface: &str, member: &str) -> bool {
    match interface {
        ITEM_INTERFACE => matches!(
            member,
            "NewIcon"
                | "NewTitle"
                | "NewStatus"
                | "NewToolTip"
                | "NewAttentionIcon"
                | "NewOverlayIcon"
                | "NewMenu"
        ),
        "org.freedesktop.DBus.Properties" => member == "PropertiesChanged",
        _ => false,
    }
}

/// How long to wait before attempt `attempt + 1` to read an item that has
/// not answered yet: 1, 2, 4, 8 s, then `MAX_RETRY_DELAY` forever (the
/// worker stops when the app leaves the bus, not after a fixed count).
fn retry_delay(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.min(4);
    Duration::from_secs(secs).min(MAX_RETRY_DELAY)
}

/// Match rule for every signal an item's object sends (`New*` and
/// `PropertiesChanged`). Signals carry the sender's unique name, so `owner`
/// must be that, not a well-known name.
fn item_signals_rule(owner: &str, path: &str) -> String {
    format!("type='signal',sender='{owner}',path='{path}'")
}

/// Match rule for `name` losing its owner (the app quit or crashed).
fn owner_lost_rule(name: &str) -> String {
    format!(
        "type='signal',sender='{DBUS_NAME}',interface='{DBUS_NAME}',member='NameOwnerChanged',arg0='{name}',arg2=''"
    )
}

/// Match rule for the registration signals of an external watcher.
fn watcher_signals_rule() -> String {
    format!("type='signal',sender='{WATCHER_NAME}',interface='{WATCHER_NAME}',path='{WATCHER_PATH}'")
}

/// Build an `SniItem` from an `org.kde.StatusNotifierItem` `GetAll` reply.
/// Missing or mistyped properties fall back to empty values, as a host must
/// tolerate partial items.
fn item_from_props(props: &HashMap<String, OwnedValue>, service: &str, path: &str) -> SniItem {
    let title = prop::<String>(props, "Title").unwrap_or_default();
    let item_is_menu = prop::<bool>(props, "ItemIsMenu").unwrap_or(false);
    let icon_name = prop::<String>(props, "IconName").filter(|s| !s.is_empty());
    let menu_path = prop::<OwnedObjectPath>(props, "Menu").filter(|p| p.as_str() != "/");
    // IconPixmap: array of (i32 width, i32 height, Vec<u8> ARGB32 rows).
    let icon = prop::<Vec<(i32, i32, Vec<u8>)>>(props, "IconPixmap")
        .and_then(|pixmaps| pick_sni_icon(&pixmaps, ICON_TARGET_SIZE));
    SniItem {
        icon,
        icon_name,
        title,
        item_is_menu,
        menu_path,
        service: service.to_string(),
        path: path.to_string(),
    }
}

/// Read one property of a `GetAll` reply as `T`, or `None` when it is
/// missing or has another type.
fn prop<T>(props: &HashMap<String, OwnedValue>, name: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    let value = props.get(name)?.try_clone().ok()?;
    T::try_from(value).ok()
}

/// Pick the pixmap closest to `target` px and unpack its big-endian ARGB32
/// bytes into tightly packed RGBA8 - mirrors `ewmh::pick_icon`'s pixel
/// unpacking, adapted to SNI's already-split-by-icon wire shape. Entries
/// whose byte count does not match their size are skipped.
fn pick_sni_icon(pixmaps: &[(i32, i32, Vec<u8>)], target: i32) -> Option<IconRgba> {
    let (w, h, bytes) = pixmaps
        .iter()
        .filter(|(w, h, bytes)| *w > 0 && *h > 0 && bytes.len() == (*w as usize) * (*h as usize) * 4)
        .min_by_key(|(w, _, _)| (*w - target).abs())?;
    let mut rgba = Vec::with_capacity(bytes.len());
    for px in bytes.as_chunks::<4>().0 {
        let (a, r, g, b) = (px[0], px[1], px[2], px[3]);
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Some(IconRgba { width: *w, height: *h, rgba })
}

// --------------------------------------------------------------------------
// Item registry
// --------------------------------------------------------------------------

/// One registered item. Dropping `wake` stops its worker.
struct Tracked {
    service: String,
    path: String,
    /// Unique bus name of the app, the sender of the item's signals.
    owner: String,
    wake: mpsc::Sender<()>,
    rules: Vec<String>,
}

/// Items currently hosted, by `item_id`. Pure bookkeeping, no DBus, so
/// the add/remove rules can be unit tested.
#[derive(Default)]
struct Registry {
    items: HashMap<String, Tracked>,
}

impl Registry {
    /// Ids of the items that live on `name`, either as their registered bus
    /// name or as their owner's unique name.
    fn ids_owned_by(&self, name: &str) -> Vec<String> {
        self.items
            .iter()
            .filter(|(_, t)| t.service == name || t.owner == name)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Wake the worker of every item whose object at `path` belongs to the
    /// unique name `sender`.
    fn wake(&self, sender: &str, path: &str) {
        for t in self.items.values().filter(|t| t.owner == sender && t.path == path) {
            let _ = t.wake.send(());
        }
    }
}

/// Everything the watcher, dispatcher and workers share.
struct Tracker {
    registry: Mutex<Registry>,
    /// Method calls to apps (property reads, clicks) and to the bus.
    calls: zbus::blocking::Connection,
    /// Only receives the signals subscribed with `add_match`; read by
    /// `dispatch` alone, which never makes calls on it (a call waiting for
    /// its reply while nobody drains the stream could stall the connection).
    signals: zbus::blocking::Connection,
    tx: async_channel::Sender<SniEvent>,
    /// Our watcher connection, set only when this process owns the watcher
    /// name; used to emit the `StatusNotifierItem(Un)Registered` signals.
    watcher: OnceLock<zbus::blocking::Connection>,
}

impl Tracker {
    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The ids of every hosted item, for `RegisteredStatusNotifierItems`.
    fn ids(&self) -> Vec<String> {
        self.lock().items.keys().cloned().collect()
    }

    /// Start hosting the item `service`/`path` unless it is already hosted:
    /// subscribe to its signals and owner, then start its worker. Makes bus
    /// calls, so it must run off zbus's executor and off `dispatch`. Returns
    /// the new item's id.
    fn add(self: &Arc<Self>, service: &str, path: &str) -> Option<String> {
        let id = item_id(service, path);
        if self.lock().items.contains_key(&id) {
            return None;
        }
        let owner = match self.name_owner(service) {
            Ok(owner) => owner,
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-panel", "SNI item {id} has no owner, ignored: {e}");
                return None;
            }
        };

        let rules = vec![item_signals_rule(&owner, path), owner_lost_rule(service)];
        for rule in &rules {
            if let Err(e) = self.bus_call_on(&self.signals, "AddMatch", rule) {
                opusdm_ipc::err_log!("opusdm-panel", "cannot watch SNI item {id}: {e}");
            }
        }

        let (wake_tx, wake_rx) = mpsc::channel();
        {
            let mut reg = self.lock();
            if reg.items.contains_key(&id) {
                drop(reg);
                self.remove_rules(&rules);
                return None;
            }
            reg.items.insert(
                id.clone(),
                Tracked { service: service.into(), path: path.into(), owner, wake: wake_tx, rules },
            );
        }

        // The app may have quit between registering and our subscription;
        // its owner-lost signal would then never come.
        if !self.has_owner(service) {
            self.remove(&id);
            return None;
        }

        let (id_w, service, path) = (id.clone(), service.to_string(), path.to_string());
        let (calls, tx) = (self.calls.clone(), self.tx.clone());
        let spawned = std::thread::Builder::new()
            .name("opusdm-panel-sni-item".into())
            .spawn(move || item_worker(&id_w, &service, &path, &calls, &tx, &wake_rx));
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("opusdm-panel", "cannot start SNI worker for {id}: {e}");
            self.remove(&id);
            return None;
        }
        Some(id)
    }

    /// Stop hosting `id`: unsubscribe, stop its worker, tell the GTK side
    /// and, when we are the watcher, every other host.
    fn remove(&self, id: &str) {
        let Some(tracked) = self.lock().items.remove(id) else { return };
        self.remove_rules(&tracked.rules);
        drop(tracked.wake);
        let _ = self.tx.send_blocking(SniEvent::Removed { id: id.to_string() });
        if let Some(conn) = self.watcher.get() {
            let _ = conn.emit_signal(
                None::<&str>,
                WATCHER_PATH,
                WATCHER_NAME,
                "StatusNotifierItemUnregistered",
                &(id,),
            );
        }
    }

    /// Remove every item that lives on `name` (it left the bus).
    fn remove_owned_by(&self, name: &str) {
        let ids = self.lock().ids_owned_by(name);
        for id in ids {
            self.remove(&id);
        }
    }

    fn remove_rules(&self, rules: &[String]) {
        for rule in rules {
            let _ = self.bus_call_on(&self.signals, "RemoveMatch", rule);
        }
    }

    /// The unique name owning `name` (itself when it already is one).
    fn name_owner(&self, name: &str) -> Result<String, zbus::Error> {
        if name.starts_with(':') {
            return Ok(name.to_string());
        }
        let reply = self.calls.call_method(Some(DBUS_NAME), DBUS_PATH, Some(DBUS_NAME), "GetNameOwner", &(name,))?;
        reply.body().deserialize::<String>()
    }

    /// Whether `name` currently has an owner on the bus. Asked on the
    /// signals connection so the bus has processed our `AddMatch` calls
    /// (sent there just before) when it answers.
    fn has_owner(&self, name: &str) -> bool {
        self.signals
            .call_method(Some(DBUS_NAME), DBUS_PATH, Some(DBUS_NAME), "NameHasOwner", &(name,))
            .and_then(|reply| reply.body().deserialize::<bool>())
            .unwrap_or(false)
    }

    fn bus_call_on(&self, conn: &zbus::blocking::Connection, method: &str, rule: &str) -> Result<(), zbus::Error> {
        conn.call_method(Some(DBUS_NAME), DBUS_PATH, Some(DBUS_NAME), method, &(rule,))?;
        Ok(())
    }
}

/// Run `job` on a short-lived thread, so the caller (zbus's executor or
/// `dispatch`) never waits on bus calls.
fn run_detached(job: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name("opusdm-panel-sni-job".into()).spawn(job) {
        opusdm_ipc::err_log!("opusdm-panel", "cannot start SNI job: {e}");
    }
}

// --------------------------------------------------------------------------
// Watcher interface
// --------------------------------------------------------------------------

struct Watcher {
    tracker: Arc<Tracker>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// Record the item and answer at once; reading it happens on its worker,
    /// so an app that is slow to answer (or two apps registering together)
    /// can't stall the watcher.
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        let sender = header.sender().map(|n| n.to_string()).unwrap_or_default();
        let (service, path) = resolve_service(service, &sender);
        let tracker = self.tracker.clone();
        run_detached(move || {
            let Some(id) = tracker.add(&service, &path) else { return };
            if let Some(conn) = tracker.watcher.get() {
                let _ = conn.emit_signal(
                    None::<&str>,
                    WATCHER_PATH,
                    WATCHER_NAME,
                    "StatusNotifierItemRegistered",
                    &(id.as_str(),),
                );
            }
        });
    }

    async fn register_status_notifier_host(&self, _service: &str) {
        // Accepted for spec completeness; see the design doc's note on
        // `IsStatusNotifierHostRegistered`.
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.tracker.ids()
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

// --------------------------------------------------------------------------
// Threads
// --------------------------------------------------------------------------

fn run(
    cmd_rx: mpsc::Receiver<SniCmd>,
    tx: async_channel::Sender<SniEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let calls = zbus::blocking::connection::Builder::session()?.method_timeout(METHOD_TIMEOUT).build()?;
    let signals = zbus::blocking::Connection::session()?;
    let tracker = Arc::new(Tracker {
        registry: Mutex::new(Registry::default()),
        calls,
        signals,
        tx,
        watcher: OnceLock::new(),
    });

    let watcher_conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name(WATCHER_NAME))
        .and_then(|b| b.serve_at(WATCHER_PATH, Watcher { tracker: tracker.clone() }))
        .and_then(|b| b.build());

    let is_host_only = match watcher_conn {
        Ok(conn) => {
            let _ = tracker.watcher.set(conn);
            false
        }
        Err(_) => {
            opusdm_ipc::err_log!("opusdm-panel", "SNI watcher already running, connecting as host only");
            true
        }
    };

    let dispatcher = tracker.clone();
    std::thread::Builder::new()
        .name("opusdm-panel-sni-signals".into())
        .spawn(move || dispatch(&dispatcher, is_host_only))?;

    if is_host_only {
        join_external_watcher(&tracker);
    }

    command_loop(&tracker.calls, &cmd_rx);
    Ok(())
}

/// Fallback when another process already owns the watcher name: register
/// as a host with it, subscribe to its registration signals (handled by
/// `dispatch`), then host every item it already has. Subscribing first
/// means an item registered meanwhile is not missed; `Tracker::add`
/// ignores the duplicate.
fn join_external_watcher(tracker: &Arc<Tracker>) {
    if let Err(e) = tracker.bus_call_on(&tracker.signals, "AddMatch", &watcher_signals_rule()) {
        opusdm_ipc::err_log!("opusdm-panel", "cannot watch the SNI watcher's registrations: {e}");
    }

    let host_name = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    let registered = tracker
        .calls
        .request_name(host_name.as_str())
        .and_then(|_| {
            tracker.calls.call_method(
                Some(WATCHER_NAME),
                WATCHER_PATH,
                Some(WATCHER_NAME),
                "RegisterStatusNotifierHost",
                &(host_name.as_str(),),
            )
        });
    if let Err(e) = registered {
        opusdm_ipc::err_log!("opusdm-panel", "cannot register as SNI host: {e}");
    }

    let items = zbus::blocking::Proxy::new(&tracker.calls, WATCHER_NAME, WATCHER_PATH, WATCHER_NAME)
        .and_then(|p| p.get_property::<Vec<String>>("RegisteredStatusNotifierItems"));
    match items {
        Ok(items) => {
            for entry in items {
                let (service, path) = split_registered_item(&entry);
                let tracker = tracker.clone();
                run_detached(move || {
                    tracker.add(&service, &path);
                });
            }
        }
        Err(e) => opusdm_ipc::err_log!("opusdm-panel", "cannot list the SNI watcher's items: {e}"),
    }
}

/// Read every signal subscribed on `tracker.signals` for the life of the
/// panel and route it: item changes wake that item's worker, an owner
/// leaving removes its items, and (`is_host_only`) the external watcher's
/// registrations add or remove items. Never makes a bus call itself.
fn dispatch(tracker: &Arc<Tracker>, is_host_only: bool) {
    for msg in zbus::blocking::MessageIterator::from(&tracker.signals) {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        if header.message_type() != zbus::message::Type::Signal {
            continue;
        }
        let (Some(interface), Some(member)) = (header.interface(), header.member()) else { continue };
        let (interface, member) = (interface.as_str(), member.as_str());

        if interface == DBUS_NAME && member == "NameOwnerChanged" {
            let Ok((name, _old, new)) = msg.body().deserialize::<(String, String, String)>() else { continue };
            if new.is_empty() {
                let tracker = tracker.clone();
                run_detached(move || tracker.remove_owned_by(&name));
            }
        } else if is_host_only && interface == WATCHER_NAME {
            let Ok(entry) = msg.body().deserialize::<String>() else { continue };
            let (service, path) = split_registered_item(&entry);
            let tracker = tracker.clone();
            match member {
                "StatusNotifierItemRegistered" => run_detached(move || {
                    tracker.add(&service, &path);
                }),
                "StatusNotifierItemUnregistered" => {
                    run_detached(move || tracker.remove(&item_id(&service, &path)))
                }
                _ => {}
            }
        } else if is_item_change_signal(interface, member) {
            let (Some(sender), Some(path)) = (header.sender(), header.path()) else { continue };
            tracker.lock().wake(sender.as_str(), path.as_str());
        }
    }
}

/// Keep one item's icon current until its wake channel is dropped (the item
/// was removed). The first read is retried with `retry_delay` while the app
/// doesn't answer; after that, each wake-up re-reads the item, with bursts
/// of wake-ups collapsed into one read per `REFRESH_INTERVAL`.
fn item_worker(
    id: &str,
    service: &str,
    path: &str,
    calls: &zbus::blocking::Connection,
    tx: &async_channel::Sender<SniEvent>,
    wake: &mpsc::Receiver<()>,
) {
    let mut shown = false;
    let mut attempt = 0u32;
    loop {
        match fetch_item(calls, service, path) {
            Ok(item) => {
                let ev = if shown {
                    SniEvent::Changed { id: id.to_string(), item }
                } else {
                    SniEvent::Added { id: id.to_string(), item }
                };
                shown = true;
                if tx.send_blocking(ev).is_err() {
                    return;
                }
            }
            Err(e) if !shown => {
                opusdm_ipc::err_log!("opusdm-panel", "cannot read SNI item {id} (attempt {}): {e}", attempt + 1);
                let delay = retry_delay(attempt);
                attempt += 1;
                match wake.recv_timeout(delay) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            // A shown item that fails one read keeps its last icon; it is
            // removed only when its owner leaves the bus.
            Err(_) => {}
        }

        if wake.recv().is_err() {
            return;
        }
        std::thread::sleep(REFRESH_INTERVAL);
        loop {
            match wake.try_recv() {
                Ok(()) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
    }
}

/// Read every `org.kde.StatusNotifierItem` property of `service`/`path` in
/// one `GetAll` call (one round trip, no property-cache proxy and its own
/// match rules).
fn fetch_item(
    calls: &zbus::blocking::Connection,
    service: &str,
    path: &str,
) -> Result<SniItem, zbus::Error> {
    let reply = calls.call_method(
        Some(service),
        path,
        Some("org.freedesktop.DBus.Properties"),
        "GetAll",
        &(ITEM_INTERFACE,),
    )?;
    let props: HashMap<String, OwnedValue> = reply.body().deserialize()?;
    Ok(item_from_props(&props, service, path))
}

/// Forward GTK-side clicks to the apps until the GTK side drops its sender.
/// Blocks in `recv`, so an idle tray costs no wake-ups.
fn command_loop(calls: &zbus::blocking::Connection, cmd_rx: &mpsc::Receiver<SniCmd>) {
    while let Ok(cmd) = cmd_rx.recv() {
        let (method, service, path, x, y) = match cmd {
            SniCmd::Activate { service, path, x, y } => ("Activate", service, path, x, y),
            SniCmd::ContextMenu { service, path, x, y } => ("ContextMenu", service, path, x, y),
        };
        if let Err(e) = calls.call_method(Some(service.as_str()), path.as_str(), Some(ITEM_INTERFACE), method, &(x, y)) {
            opusdm_ipc::err_log!("opusdm-panel", "SNI {method} failed for {service}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn owned(v: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(v).unwrap()
    }

    fn tracked(service: &str, path: &str, owner: &str) -> (Tracked, mpsc::Receiver<()>) {
        let (wake, rx) = mpsc::channel();
        let t = Tracked { service: service.into(), path: path.into(), owner: owner.into(), wake, rules: Vec::new() };
        (t, rx)
    }

    #[test]
    fn resolve_service_bus_name_uses_default_path() {
        let got = resolve_service("org.kde.StatusNotifierItem-4911-1", ":1.59");
        assert_eq!(got, ("org.kde.StatusNotifierItem-4911-1".into(), "/StatusNotifierItem".into()));
    }

    #[test]
    fn resolve_service_object_path_lives_on_the_sender() {
        let got = resolve_service("/org/ayatana/NotificationItem/nm_applet", ":1.20");
        assert_eq!(got, (":1.20".into(), "/org/ayatana/NotificationItem/nm_applet".into()));
    }

    #[test]
    fn item_id_and_split_registered_item_round_trip() {
        let id = item_id(":1.20", "/org/ayatana/NotificationItem/nm_applet");
        assert_eq!(id, ":1.20/org/ayatana/NotificationItem/nm_applet");
        assert_eq!(split_registered_item(&id), (":1.20".into(), "/org/ayatana/NotificationItem/nm_applet".into()));
    }

    #[test]
    fn split_registered_item_bare_name_means_default_path() {
        assert_eq!(split_registered_item(":1.34"), (":1.34".into(), "/StatusNotifierItem".into()));
    }

    #[test]
    fn spec_new_signals_and_properties_changed_are_item_changes() {
        assert!(is_item_change_signal(ITEM_INTERFACE, "NewIcon"));
        assert!(is_item_change_signal(ITEM_INTERFACE, "NewToolTip"));
        assert!(is_item_change_signal("org.freedesktop.DBus.Properties", "PropertiesChanged"));
        assert!(!is_item_change_signal(ITEM_INTERFACE, "Activate"));
        assert!(!is_item_change_signal("com.canonical.dbusmenu", "LayoutUpdated"));
    }

    #[test]
    fn retry_delay_doubles_then_caps() {
        let secs: Vec<u64> = (0..7).map(|a| retry_delay(a).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 15, 15, 15]);
    }

    #[test]
    fn match_rules_filter_on_owner_and_path() {
        assert_eq!(item_signals_rule(":1.59", "/StatusNotifierItem"), "type='signal',sender=':1.59',path='/StatusNotifierItem'");
        assert!(owner_lost_rule("org.x").contains("arg0='org.x',arg2=''"));
    }

    #[test]
    fn item_from_props_reads_title_menu_and_pixmap() {
        // One 1x1 opaque red pixel in ARGB32, plus a 2x2 entry farther from the target.
        let pixmaps = vec![(1i32, 1i32, vec![0xff, 0xff, 0x00, 0x00]), (2i32, 2i32, vec![0u8; 16])];
        let mut props = HashMap::new();
        props.insert("Title".to_string(), owned(Value::from("MEGAsync")));
        props.insert("ItemIsMenu".to_string(), owned(Value::from(true)));
        props.insert("IconName".to_string(), owned(Value::from("")));
        props.insert(
            "Menu".to_string(),
            owned(Value::from(zbus::zvariant::ObjectPath::try_from("/MenuBar").unwrap())),
        );
        props.insert("IconPixmap".to_string(), owned(Value::from(pixmaps)));

        let item = item_from_props(&props, "org.kde.StatusNotifierItem-1-1", "/StatusNotifierItem");
        assert_eq!(item.title, "MEGAsync");
        assert!(item.item_is_menu);
        assert_eq!(item.icon_name, None, "an empty icon name is no icon name");
        assert_eq!(item.menu_path.as_ref().map(|p| p.as_str()), Some("/MenuBar"));
        let icon = item.icon.expect("pixmap decoded");
        assert_eq!((icon.width, icon.height), (2, 2), "closest to 24 px wins");
        assert_eq!(item.service, "org.kde.StatusNotifierItem-1-1");
    }

    #[test]
    fn item_from_props_tolerates_missing_properties() {
        let item = item_from_props(&HashMap::new(), ":1.5", "/StatusNotifierItem");
        assert_eq!(item.title, "");
        assert!(!item.item_is_menu && item.icon.is_none() && item.menu_path.is_none());
    }

    #[test]
    fn pick_sni_icon_converts_argb_to_rgba() {
        let icon = pick_sni_icon(&[(1, 1, vec![0x80, 0x11, 0x22, 0x33])], 24).unwrap();
        assert_eq!(icon.rgba, vec![0x11, 0x22, 0x33, 0x80]);
    }

    #[test]
    fn pick_sni_icon_skips_truncated_pixmaps() {
        assert!(pick_sni_icon(&[(22, 22, vec![0u8; 10])], 24).is_none());
    }

    #[test]
    fn registry_finds_items_by_service_or_owner() {
        let mut reg = Registry::default();
        let (a, _ra) = tracked("org.kde.StatusNotifierItem-9-1", "/StatusNotifierItem", ":1.59");
        let (b, _rb) = tracked(":1.20", "/org/ayatana/NotificationItem/nm", ":1.20");
        reg.items.insert("a".into(), a);
        reg.items.insert("b".into(), b);
        assert_eq!(reg.ids_owned_by("org.kde.StatusNotifierItem-9-1"), vec!["a".to_string()]);
        assert_eq!(reg.ids_owned_by(":1.59"), vec!["a".to_string()]);
        assert_eq!(reg.ids_owned_by(":1.20"), vec!["b".to_string()]);
        assert!(reg.ids_owned_by(":1.99").is_empty());
    }

    #[test]
    fn registry_wakes_only_the_matching_item() {
        let mut reg = Registry::default();
        let (a, ra) = tracked("org.kde.StatusNotifierItem-9-1", "/StatusNotifierItem", ":1.59");
        let (b, rb) = tracked(":1.20", "/StatusNotifierItem", ":1.20");
        reg.items.insert("a".into(), a);
        reg.items.insert("b".into(), b);
        reg.wake(":1.59", "/StatusNotifierItem");
        assert!(ra.try_recv().is_ok());
        assert!(rb.try_recv().is_err());
        reg.wake(":1.59", "/Other");
        assert!(ra.try_recv().is_err());
    }
}
