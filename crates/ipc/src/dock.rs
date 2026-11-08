//! The dock: its settings file `~/.config/opusdm/dock.json` and the wire
//! protocol of its socket.
//!
//! `dock.json` holds everything about the dock: the pinned apps, the edge,
//! the icon size, whether it is shown and its look. Nobody keeps a long-lived
//! copy of it: every writer (the dock, the "Show the dock" check box of the
//! Settings tool, the hub desktop menu) goes through [`update_dock_config`],
//! which reads the file fresh, changes only its own fields and saves it
//! atomically. The dock watches the file and follows every change.
//!
//! The layout fields used to live in `desktop.json` (`dock_position`,
//! `dock_icon_size`, `dock_visible`, `dock_style`). The first load copies
//! them over once ([`DockConfig::layout_migrated`]).

use crate::desktop::{DockPosition, DockStyle, DOCK_ICON_SIZE_RANGE};
use crate::frame::{recv, send};
use crate::settings::{config_dir, desktop_settings_path, save_json};
use serde::{Deserialize, Serialize};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default launcher icon size, in pixels.
pub const DEFAULT_DOCK_ICON_SIZE: i32 = 48;

/// Environment variable that overrides the dock socket path.
pub const DOCK_ENV_SOCK: &str = "OPUSDM_DOCK_SOCK";

/// File name of the dock socket, next to the hub socket.
pub const DOCK_SOCK_FILE_NAME: &str = "dock.sock";

/// Connect, read and write timeout of a client of the dock socket. The dock
/// answers from its GTK main loop; a dock that does not answer in time is
/// treated as stuck, so the caller never hangs on it.
pub const DOCK_CLIENT_TIMEOUT: Duration = Duration::from_secs(1);

/// Log tag of this module.
const TAG: &str = "dock-config";

/// On-disk shape of `dock.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockConfig {
    /// Pinned apps as desktop file ids (`firefox.desktop`), in dock order.
    /// `None` when no list was ever chosen: the dock fills in its default set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<String>>,
    /// Requested edge; the dock keeps clear of the panel on its own
    /// (see [`DockPosition::avoiding`]).
    pub position: DockPosition,
    /// Launcher icon size in pixels.
    pub icon_size: i32,
    /// Whether the dock runs at all. Off: no dock process.
    pub visible: bool,
    /// Background and zoom.
    pub style: DockStyle,
    /// Whether the layout fields were already copied from `desktop.json`.
    pub layout_migrated: bool,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            apps: None,
            position: DockPosition::default(),
            icon_size: DEFAULT_DOCK_ICON_SIZE,
            visible: true,
            style: DockStyle::default(),
            layout_migrated: false,
        }
    }
}

impl DockConfig {
    /// Clamp the icon size and the style into their accepted ranges.
    pub fn sanitized(mut self) -> DockConfig {
        let (lo, hi) = DOCK_ICON_SIZE_RANGE;
        self.icon_size = self.icon_size.clamp(lo, hi);
        self.style = self.style.sanitized();
        self
    }

    /// The pinned apps, or an empty list when none were chosen yet.
    pub fn app_ids(&self) -> Vec<String> {
        self.apps.clone().unwrap_or_default()
    }
}

/// Path to `dock.json` under [`config_dir`].
pub fn dock_config_path() -> PathBuf {
    config_dir().join("dock.json")
}

/// Read `dock.json` at `path` as it is. `Ok(None)` when the file is missing;
/// an unreadable or malformed file is an error that names the file.
pub fn read_dock_config_from(path: &Path) -> io::Result<Option<DockConfig>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io::Error::new(e.kind(), format!("cannot read {}: {e}", path.display()))),
    };
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("malformed {}: {e}", path.display())))
}

/// `cfg` with the layout fields copied from the `dock_*` keys of the
/// `desktop.json` at `desktop_path`, when they were not copied yet. A missing
/// key, or a missing `desktop.json`, keeps the default. The second value
/// tells whether anything changed (the caller then saves).
pub fn migrated(cfg: Option<DockConfig>, desktop_path: &Path) -> (DockConfig, bool) {
    let mut cfg = cfg.unwrap_or_default();
    if cfg.layout_migrated {
        return (cfg, false);
    }
    let desktop = _read_desktop_json(desktop_path);
    let key = |name: &str| desktop.as_ref().and_then(|d| d.get(name)).cloned();
    if let Some(v) = key("dock_position").and_then(|v| serde_json::from_value(v).ok()) {
        cfg.position = v;
    }
    if let Some(v) = key("dock_icon_size").and_then(|v| serde_json::from_value(v).ok()) {
        cfg.icon_size = v;
    }
    if let Some(v) = key("dock_visible").and_then(|v| serde_json::from_value(v).ok()) {
        cfg.visible = v;
    }
    if let Some(v) = key("dock_style").and_then(|v| serde_json::from_value(v).ok()) {
        cfg.style = v;
    }
    cfg.layout_migrated = true;
    (cfg.sanitized(), true)
}

/// The raw JSON object of `desktop.json`, `None` when it is missing or
/// cannot be used (logged: the dock then starts from the default layout).
fn _read_desktop_json(path: &Path) -> Option<serde_json::Value> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            crate::err_log!(TAG, "cannot read {} to move the dock settings: {e}; default dock layout", path.display());
            return None;
        }
    };
    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(v) if v.is_object() => Some(v),
        Ok(_) => {
            crate::err_log!(TAG, "{} is not a JSON object: default dock layout", path.display());
            None
        }
        Err(e) => {
            crate::err_log!(TAG, "malformed {} ({e}): default dock layout", path.display());
            None
        }
    }
}

/// Load the dock config from the standard paths, see [`load_dock_config_at`].
pub fn load_dock_config() -> DockConfig {
    load_dock_config_at(&dock_config_path(), &desktop_settings_path())
}

/// Load `dock.json` at `path`, moving the layout over from the
/// `desktop.json` at `desktop_path` first when needed (and saving the
/// result). An unreadable or malformed file is logged and left untouched;
/// the defaults (with the layout of `desktop.json`) are returned instead.
pub fn load_dock_config_at(path: &Path, desktop_path: &Path) -> DockConfig {
    let current = match read_dock_config_from(path) {
        Ok(c) => c,
        Err(e) => {
            crate::err_log!(TAG, "{e}; the dock uses its defaults and the file is not overwritten");
            return migrated(None, desktop_path).0;
        }
    };
    let (cfg, changed) = migrated(current, desktop_path);
    if changed {
        match save_json(path, &cfg) {
            Ok(()) => crate::debug_log!(TAG, "dock layout moved from {} to {}", desktop_path.display(), path.display()),
            Err(e) => crate::err_log!(TAG, "cannot write {} after moving the dock layout: {e}", path.display()),
        }
    }
    cfg.sanitized()
}

/// Change `dock.json` with `change`, see [`update_dock_config_at`].
pub fn update_dock_config(change: impl FnOnce(&mut DockConfig)) -> io::Result<DockConfig> {
    update_dock_config_at(&dock_config_path(), &desktop_settings_path(), change)
}

/// Read `dock.json` at `path` fresh (moving the layout over from
/// `desktop_path` when needed), apply `change`, and save it atomically.
/// Returns the saved config. An unreadable or malformed file is an error
/// and is not overwritten.
pub fn update_dock_config_at(path: &Path, desktop_path: &Path, change: impl FnOnce(&mut DockConfig)) -> io::Result<DockConfig> {
    let (mut cfg, _) = migrated(read_dock_config_from(path)?, desktop_path);
    change(&mut cfg);
    let cfg = cfg.sanitized();
    save_json(path, &cfg).map_err(|e| io::Error::new(e.kind(), format!("cannot write {}: {e}", path.display())))?;
    Ok(cfg)
}

/// Save `cfg` to `path` as it is, creating the directory. Prefer
/// [`update_dock_config_at`], which keeps the fields of the other writers.
pub fn save_dock_config_to(path: &Path, cfg: &DockConfig) -> io::Result<()> {
    save_json(path, cfg)
}

/// A request sent to the dock process.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockRequest {
    /// Is the dock alive? Also the single-instance probe.
    Ping,
    /// The hub hid (`true`) or revealed the desktop chrome ("Hide All").
    SetHidden(bool),
}

/// The answer to one [`DockRequest`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum DockReply {
    /// The request was done; whether the dock is hidden by the hub now.
    Hidden(bool),
    /// The request failed; the text says why.
    Error(String),
}

/// Path of the dock socket: `$OPUSDM_DOCK_SOCK`, else `dock.sock` in the
/// folder of the hub socket (the temp dir if that has no parent).
pub fn dock_socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os(DOCK_ENV_SOCK).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let hub = crate::sockpath::hub_sock_path();
    let dir = hub.parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
    dir.join(DOCK_SOCK_FILE_NAME)
}

/// Send `req` to the dock socket at `path` and return the one reply, within
/// [`DOCK_CLIENT_TIMEOUT`]. A missing socket, a timeout or an EOF before the
/// reply is an `io::Error`.
pub fn dock_request_at(path: &Path, req: &DockRequest) -> io::Result<DockReply> {
    let mut s = UnixStream::connect(path)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot connect to {}: {e}", path.display())))?;
    s.set_read_timeout(Some(DOCK_CLIENT_TIMEOUT))?;
    s.set_write_timeout(Some(DOCK_CLIENT_TIMEOUT))?;
    send(&mut s, req)?;
    match recv::<_, DockReply>(&mut s) {
        Ok(Some(reply)) => Ok(reply),
        Ok(None) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "dock socket closed before a reply")),
        Err(e) => Err(io::Error::new(e.kind(), format!("no reply from the dock socket: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::DockImageMode;
    use std::os::unix::net::UnixListener;

    /// The two file paths in a fresh temp dir.
    fn paths(td: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        (td.path().join("dock.json"), td.path().join("desktop.json"))
    }

    #[test]
    fn missing_file_is_none() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(read_dock_config_from(&td.path().join("dock.json")).unwrap(), None);
    }

    #[test]
    fn malformed_file_is_an_error_and_is_not_overwritten() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        std::fs::write(&dock, b"nope").unwrap();
        assert!(read_dock_config_from(&dock).is_err());
        let loaded = load_dock_config_at(&dock, &desktop);
        assert_eq!(loaded.apps, None);
        assert!(update_dock_config_at(&dock, &desktop, |c| c.visible = false).is_err());
        assert_eq!(std::fs::read_to_string(&dock).unwrap(), "nope", "a broken file is kept as it is");
    }

    #[test]
    fn an_old_file_with_only_apps_keeps_them_and_takes_the_desktop_layout() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        std::fs::write(&dock, br#"{"apps":["b.desktop","a.desktop"]}"#).unwrap();
        std::fs::write(
            &desktop,
            br#"{"icon_size":64,"dock_position":"left","dock_icon_size":32,"dock_visible":false,"dock_style":{"opacity":40,"image_mode":"tile"}}"#,
        )
        .unwrap();
        let cfg = load_dock_config_at(&dock, &desktop);
        assert_eq!(cfg.apps, Some(vec!["b.desktop".to_string(), "a.desktop".to_string()]));
        assert_eq!((cfg.position, cfg.icon_size, cfg.visible), (DockPosition::Left, 32, false));
        assert_eq!((cfg.style.opacity, cfg.style.image_mode), (40, DockImageMode::Tile));
        assert!(cfg.layout_migrated);
        assert_eq!(read_dock_config_from(&dock).unwrap(), Some(cfg), "the migrated file is saved");
    }

    #[test]
    fn the_migration_runs_once() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        std::fs::write(&desktop, br#"{"dock_icon_size":32}"#).unwrap();
        assert_eq!(load_dock_config_at(&dock, &desktop).icon_size, 32);
        update_dock_config_at(&dock, &desktop, |c| c.icon_size = 64).unwrap();
        // desktop.json still has the old key: it must not win again.
        assert_eq!(load_dock_config_at(&dock, &desktop).icon_size, 64);
    }

    #[test]
    fn without_any_file_the_defaults_are_used_and_the_apps_stay_unset() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        let cfg = load_dock_config_at(&dock, &desktop);
        assert_eq!(cfg, DockConfig { layout_migrated: true, ..DockConfig::default() });
        assert_eq!(cfg.apps, None, "the dock fills in its default set");
    }

    #[test]
    fn a_malformed_desktop_json_gives_the_default_layout_and_is_not_touched() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        std::fs::write(&desktop, b"{broken").unwrap();
        let cfg = load_dock_config_at(&dock, &desktop);
        assert_eq!((cfg.position, cfg.icon_size, cfg.visible), (DockPosition::Bottom, DEFAULT_DOCK_ICON_SIZE, true));
        assert_eq!(std::fs::read_to_string(&desktop).unwrap(), "{broken");
    }

    #[test]
    fn an_update_keeps_the_fields_it_does_not_touch() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        // Writer 1 (the dock) sets the apps and the edge.
        update_dock_config_at(&dock, &desktop, |c| {
            c.apps = Some(vec!["a.desktop".into()]);
            c.position = DockPosition::Right;
        })
        .unwrap();
        // Writer 2 (the Settings check box) only turns the dock off.
        update_dock_config_at(&dock, &desktop, |c| c.visible = false).unwrap();
        let cfg = read_dock_config_from(&dock).unwrap().unwrap();
        assert_eq!(cfg.apps, Some(vec!["a.desktop".to_string()]));
        assert_eq!((cfg.position, cfg.visible), (DockPosition::Right, false));
    }

    #[test]
    fn values_out_of_range_are_clamped() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        std::fs::write(&dock, br#"{"icon_size":9000,"layout_migrated":true,"style":{"opacity":500}}"#).unwrap();
        let cfg = load_dock_config_at(&dock, &desktop);
        assert_eq!(cfg.icon_size, DOCK_ICON_SIZE_RANGE.1);
        assert_eq!(cfg.style.opacity, 100);
    }

    #[test]
    fn an_empty_list_is_kept_apart_from_no_list() {
        let td = tempfile::tempdir().unwrap();
        let (dock, desktop) = paths(&td);
        std::fs::write(&dock, br#"{"apps":[]}"#).unwrap();
        assert_eq!(load_dock_config_at(&dock, &desktop).apps, Some(Vec::new()));
    }

    #[test]
    fn the_protocol_roundtrips_over_a_socket() {
        let td = tempfile::Builder::new().prefix("od1").tempdir_in("/tmp").unwrap();
        let path = td.path().join("d.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let req: DockRequest = recv(&mut s).unwrap().unwrap();
                let hidden = matches!(req, DockRequest::SetHidden(true));
                send(&mut s, &DockReply::Hidden(hidden)).unwrap();
            }
        });
        assert_eq!(dock_request_at(&path, &DockRequest::SetHidden(true)).unwrap(), DockReply::Hidden(true));
        assert_eq!(dock_request_at(&path, &DockRequest::Ping).unwrap(), DockReply::Hidden(false));
        server.join().unwrap();
    }

    #[test]
    fn a_silent_dock_times_out_instead_of_hanging() {
        let td = tempfile::Builder::new().prefix("od2").tempdir_in("/tmp").unwrap();
        let path = td.path().join("d.sock");
        let _silent = UnixListener::bind(&path).unwrap();
        let started = std::time::Instant::now();
        assert!(dock_request_at(&path, &DockRequest::Ping).is_err());
        assert!(started.elapsed() < DOCK_CLIENT_TIMEOUT * 3);
    }

    #[test]
    fn a_missing_socket_is_an_error() {
        let td = tempfile::Builder::new().prefix("od3").tempdir_in("/tmp").unwrap();
        assert!(dock_request_at(&td.path().join("none.sock"), &DockRequest::Ping).is_err());
    }
}
