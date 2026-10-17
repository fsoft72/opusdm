//! Where the Run window's suggestions come from: the OpusDM application menu
//! and the executables on `PATH`.

use crate::search::{Entry, Kind};
use gtk4::gio;
use gtk4::prelude::*;
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

/// The applications of the custom menu (`menu.json`) when it lists any, else
/// every installed application a menu would show.
pub fn app_entries() -> Vec<Entry> {
    let ids = opusdm_ipc::menu::load_menu_config().map(|cfg| flatten_menu(&cfg.items)).unwrap_or_default();
    if !ids.is_empty() {
        let infos = opusdm_apps::apps::resolve(&ids).into_iter().map(|e| e.info);
        return infos.filter_map(|info| app_entry(&info)).collect();
    }
    gio::AppInfo::all()
        .into_iter()
        .filter(|info| info.should_show())
        .filter_map(|info| info.downcast::<gio::DesktopAppInfo>().ok())
        .filter_map(|info| app_entry(&info))
        .collect()
}

/// The entry for one desktop application; `None` when it has no id.
fn app_entry(info: &gio::DesktopAppInfo) -> Option<Entry> {
    let id = info.id()?.to_string();
    let generic = info.generic_name().map(|s| s.to_string()).unwrap_or_default();
    let description = info.description().map(|s| s.to_string()).unwrap_or_default();
    let mut extra = vec![generic.clone()];
    extra.extend(info.keywords().iter().map(|k| k.to_string()));
    if let Some(program) = info.executable().file_name() {
        extra.push(program.to_string_lossy().to_string());
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
