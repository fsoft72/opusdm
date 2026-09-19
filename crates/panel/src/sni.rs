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

fn run(
    _cmd_rx: mpsc::Receiver<SniCmd>,
    _tx: async_channel::Sender<SniEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _conn = zbus::blocking::Connection::session()?;
    // Watcher registration and the event loop land in Task 2-3.
    Ok(())
}
