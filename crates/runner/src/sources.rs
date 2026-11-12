//! Where the Run window's suggestions come from: the OpusDM application menu,
//! the other installed applications and the executables on `PATH`.

use opusdm_apps::entries::app_entry;
use opusdm_apps::search::{Entry, Kind};
use opusdm_ipc::MenuItem;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;

/// The executables on this process's `PATH`. Stats every file of every
/// directory, so it belongs on a worker thread, not the GTK one.
pub fn path_commands() -> Vec<Entry> {
    scan_path(&std::env::var_os("PATH").unwrap_or_default())
}

/// The desktop file ids of `items`, folders flattened in menu order.
pub fn flatten_menu(items: &[MenuItem]) -> Vec<String> {
    let mut ids = Vec::new();
    for item in items {
        match item {
            MenuItem::App { id } => ids.push(id.clone()),
            MenuItem::Folder { items, .. } => ids.extend(flatten_menu(items)),
        }
    }
    ids
}

/// The applications of the custom menu (`menu.json`), in menu order, then
/// every other installed application a menu would show. `menu.json` is
/// seeded once, so an app installed later is only in the second list; without
/// it the Run window offered that app only as a `PATH` command.
pub fn app_entries() -> Vec<Entry> {
    let ids = opusdm_ipc::menu::load_menu_config().map(|cfg| flatten_menu(&cfg.items)).unwrap_or_default();
    let menu = opusdm_apps::apps::resolve(&ids).into_iter().filter_map(|e| app_entry(&e.info)).collect();
    merge_apps(menu, opusdm_apps::entries::installed_entries())
}

/// `menu` followed by the entries of `installed` it does not hold, each id
/// once, the first occurrence winning.
fn merge_apps(menu: Vec<Entry>, installed: Vec<Entry>) -> Vec<Entry> {
    let mut seen = HashSet::new();
    menu.into_iter().chain(installed).filter(|entry| seen.insert(entry.id.clone())).collect()
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
    use std::fs;

    fn app(id: &str) -> MenuItem {
        MenuItem::App { id: id.to_string() }
    }

    fn app_entry(id: &str) -> Entry {
        Entry { kind: Kind::App, id: id.to_string(), name: id.to_string(), comment: String::new(), extra: Vec::new(), icon: None }
    }

    fn ids_of(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    /// An app installed after `menu.json` was seeded must still be an
    /// application of the Run window (with its icon), not only a command.
    #[test]
    fn merge_apps_keeps_the_menu_order_then_adds_the_other_installed_apps() {
        let menu = vec![app_entry("b.desktop"), app_entry("a.desktop")];
        let installed = vec![app_entry("a.desktop"), app_entry("new.desktop"), app_entry("b.desktop"), app_entry("new.desktop")];
        assert_eq!(ids_of(&merge_apps(menu, installed)), ["b.desktop", "a.desktop", "new.desktop"]);
    }

    #[test]
    fn merge_apps_with_an_empty_menu_lists_every_installed_app() {
        let installed = vec![app_entry("a.desktop"), app_entry("b.desktop")];
        assert_eq!(ids_of(&merge_apps(Vec::new(), installed)), ["a.desktop", "b.desktop"]);
    }

    #[test]
    fn flatten_menu_walks_folders_in_order() {
        let items = vec![app("a.desktop"), MenuItem::Folder { name: "F".into(), items: vec![app("b.desktop"), MenuItem::Folder { name: "G".into(), items: vec![app("c.desktop")] }] }, app("d.desktop")];
        assert_eq!(flatten_menu(&items), ["a.desktop", "b.desktop", "c.desktop", "d.desktop"]);
    }

    #[test]
    fn scan_path_lists_executables_once_and_skips_the_rest() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for (dir, name, mode) in [(&first, "tool", 0o755), (&first, "notes.txt", 0o644), (&second, "tool", 0o755), (&second, "other", 0o755)] {
            let path = dir.path().join(name);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        }
        fs::create_dir(second.path().join("subdir")).unwrap();
        let path_var = std::env::join_paths([first.path(), second.path(), std::path::Path::new("/nonexistent-dir")]).unwrap();
        let mut names: Vec<String> = scan_path(&path_var).into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(names, ["other", "tool"]);
    }
}
