//! Desktop applications as Run-style search entries, shared by the Run
//! window and the Lister's "Open with..." dialog.

use crate::search::{Entry, Kind};
use gtk4::gio;
use gtk4::prelude::*;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;

/// The file name of the program `info` runs; `None` when the entry has no
/// `Exec=`: GIO returns NULL there and the binding would panic.
pub fn program_name(info: &gio::DesktopAppInfo) -> Option<String> {
    if !info.has_key("Exec") {
        return None;
    }
    info.executable().file_name().map(|name| name.to_string_lossy().to_string())
}

/// The entry for one desktop application; `None` when it has no id.
pub fn app_entry(info: &gio::DesktopAppInfo) -> Option<Entry> {
    let id = info.id()?.to_string();
    let generic = info.generic_name().map(|s| s.to_string()).unwrap_or_default();
    let description = info.description().map(|s| s.to_string()).unwrap_or_default();
    let mut extra = vec![generic.clone()];
    extra.extend(info.keywords().iter().map(|k| k.to_string()));
    match program_name(info) {
        Some(program) => extra.push(program),
        None => opusdm_ipc::err_log!("apps", "{id}: desktop entry without Exec="),
    }
    extra.retain(|text| !text.is_empty());
    Some(Entry {
        kind: Kind::App,
        id,
        name: info.name().to_string(),
        comment: if generic.is_empty() { description } else { generic },
        extra,
        icon: info.icon().and_then(|icon| IconExt::to_string(&icon)).map(|s| s.to_string()),
    })
}

/// Every installed application a menu would show, as search entries.
pub fn installed_entries() -> Vec<Entry> {
    gio::AppInfo::all()
        .into_iter()
        .filter(|info| info.should_show())
        .filter_map(|info| info.downcast::<gio::DesktopAppInfo>().ok())
        .filter_map(|info| app_entry(&info))
        .collect()
}

/// The executables on this process's `PATH`. Stats every file of every
/// directory, so it belongs on a worker thread, not the GTK one.
pub fn path_commands() -> Vec<Entry> {
    scan_path(&std::env::var_os("PATH").unwrap_or_default())
}

/// The executables in the directories of `path_var` (a `PATH` value), each
/// name once, the first directory winning.
pub fn scan_path(path_var: &OsStr) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for dir in std::env::split_paths(path_var) {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            let runnable = std::fs::metadata(item.path()).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
            if !runnable || !seen.insert(name.clone()) {
                continue;
            }
            entries.push(Entry { kind: Kind::Command, id: name.clone(), name, comment: String::new(), extra: Vec::new(), icon: None });
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A desktop entry without `Exec=` (like a hand-made `IsaacSim.desktop`)
    /// must not panic the entry builder: it used to kill the Lister when
    /// "Open with..." listed the installed applications.
    #[test]
    fn an_entry_without_exec_has_no_program_and_does_not_panic() {
        let keyfile = gtk4::glib::KeyFile::new();
        keyfile.set_string("Desktop Entry", "Type", "Application");
        keyfile.set_string("Desktop Entry", "Name", "No Exec");
        let info = gio::DesktopAppInfo::from_keyfile(&keyfile).expect("keyfile app info");
        assert_eq!(program_name(&info), None);
    }

    #[test]
    fn an_entry_with_exec_reports_its_program() {
        let keyfile = gtk4::glib::KeyFile::new();
        keyfile.set_string("Desktop Entry", "Type", "Application");
        keyfile.set_string("Desktop Entry", "Name", "With Exec");
        keyfile.set_string("Desktop Entry", "Exec", "/usr/bin/true %U");
        let info = gio::DesktopAppInfo::from_keyfile(&keyfile).expect("keyfile app info");
        assert_eq!(program_name(&info).as_deref(), Some("true"));
    }
}
