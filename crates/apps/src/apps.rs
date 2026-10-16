//! Pinned applications: resolving desktop file ids to app infos, the default
//! set for a first run, and launching.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;

/// Terminal emulators tried, in order, for the default set.
const TERMINAL_IDS: [&str; 8] = [
    "org.gnome.Terminal.desktop",
    "xfce4-terminal.desktop",
    "org.kde.konsole.desktop",
    "konsole.desktop",
    "kitty.desktop",
    "Alacritty.desktop",
    "lxterminal.desktop",
    "xterm.desktop",
];

/// File managers tried, in order, when the system has no default one.
const FILE_MANAGER_IDS: [&str; 6] = [
    "org.gnome.Nautilus.desktop",
    "thunar.desktop",
    "pcmanfm.desktop",
    "org.kde.dolphin.desktop",
    "nemo.desktop",
    "caja.desktop",
];

/// One pinned app that exists on this system.
#[derive(Clone)]
pub struct AppEntry {
    pub id: String,
    pub info: gio::DesktopAppInfo,
}

/// Look up one pinned id: a desktop file id (`firefox.desktop`) or, for a
/// `.desktop` file outside the applications directories, its absolute path.
fn lookup(id: &str) -> Option<gio::DesktopAppInfo> {
    if id.starts_with('/') {
        gio::DesktopAppInfo::from_filename(id)
    } else {
        gio::DesktopAppInfo::new(id)
    }
}

/// Look up each of `ids`, dropping the ones that are not installed.
pub fn resolve(ids: &[String]) -> Vec<AppEntry> {
    ids.iter()
        .filter_map(|id| lookup(id).map(|info| AppEntry { id: id.clone(), info }))
        .collect()
}

/// The id to pin for the `.desktop` file at `path`: its desktop file id when
/// the system knows it, else the absolute path. `None` when `path` is not a
/// launchable desktop entry.
pub fn id_for_desktop_file(path: &std::path::Path) -> Option<String> {
    if path.extension()? != "desktop" {
        return None;
    }
    let info = gio::DesktopAppInfo::from_filename(path)?;
    let by_id = info.id().map(|id| id.to_string()).filter(|id| gio::DesktopAppInfo::new(id).is_some());
    by_id.or_else(|| path.to_str().map(str::to_string))
}

/// Ids of the entries in `entries`, in order.
pub fn ids(entries: &[AppEntry]) -> Vec<String> {
    entries.iter().map(|e| e.id.clone()).collect()
}

/// The desktop file id of the system's default handler for `content_type`,
/// if it is an installed desktop app.
fn default_handler(content_type: &str) -> Option<String> {
    let id = gio::AppInfo::default_for_type(content_type, false)?.id()?.to_string();
    gio::DesktopAppInfo::new(&id).map(|_| id)
}

/// The first of `candidates` that is installed.
fn first_installed(candidates: &[&str]) -> Option<String> {
    candidates.iter().find(|id| gio::DesktopAppInfo::new(id).is_some()).map(|id| id.to_string())
}

/// The set of apps a dock starts with when there is no `dock.json`: file
/// manager, terminal and web browser, keeping only those installed.
pub fn default_app_ids() -> Vec<String> {
    [
        default_handler("inode/directory").or_else(|| first_installed(&FILE_MANAGER_IDS)),
        first_installed(&TERMINAL_IDS),
        default_handler("x-scheme-handler/http"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Launch `info` through the display's launch context, so the window manager
/// gets startup notification. Errors are logged.
pub fn launch(info: &gio::DesktopAppInfo) {
    let ctx = gtk::gdk::Display::default().map(|d| d.app_launch_context());
    let ctx = ctx.as_ref().map(|c| c.upcast_ref::<gio::AppLaunchContext>());
    if let Err(e) = info.launch(&[], ctx) {
        opusdm_ipc::err_log!("dock", "cannot launch {}: {e}", info.name());
    }
}

/// Launch the desktop action `action` (`Actions=` entry) of `info`.
pub fn launch_action(info: &gio::DesktopAppInfo, action: &str) {
    let ctx = gtk::gdk::Display::default().map(|d| d.app_launch_context());
    let ctx = ctx.as_ref().map(|c| c.upcast_ref::<gio::AppLaunchContext>());
    info.launch_action(action, ctx);
}
