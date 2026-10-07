//! Custom application menu: `~/.config/opusdm/menu.json`.
//!
//! Owned by `opusdm-settings` (the Menu tab, which edits it) and read by
//! `opusdm-panel` (the application menu popover). A folder nests apps and
//! further folders; an empty config (no file, or `{}`) means "no custom
//! menu yet", so the panel falls back to its auto-generated, category-based
//! menu until the user builds one.

use crate::settings::{config_dir, save_json};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// One entry in the menu tree: a launchable app, or a folder of further
/// entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MenuItem {
    /// A desktop file id (`firefox.desktop`) or, for an entry outside the
    /// applications directories, its absolute path - the same shape
    /// `opusdm-dock` pins apps by.
    App { id: String },
    /// A named group, nesting apps and/or further folders.
    Folder { name: String, items: Vec<MenuItem> },
}

/// On-disk shape of `menu.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuConfig {
    /// Top-level entries, in menu order.
    pub items: Vec<MenuItem>,
}

/// Path to `menu.json` under [`config_dir`].
pub fn menu_config_path() -> PathBuf {
    config_dir().join("menu.json")
}

/// Load the menu config from the standard path. `None` when the file is
/// missing or malformed, so the caller can fall back to its defaults (the
/// auto-generated category menu).
pub fn load_menu_config() -> Option<MenuConfig> {
    load_menu_config_from(&menu_config_path())
}

/// Testable core of [`load_menu_config`].
pub fn load_menu_config_from(path: &Path) -> Option<MenuConfig> {
    crate::settings::read_json(path, "ipc", "menu config")
}

/// Save the menu config to the standard path, creating the directory.
pub fn save_menu_config(cfg: &MenuConfig) -> io::Result<()> {
    save_menu_config_to(&menu_config_path(), cfg)
}

/// Testable core of [`save_menu_config`].
pub fn save_menu_config_to(path: &Path, cfg: &MenuConfig) -> io::Result<()> {
    save_json(path, cfg)
}

/// Walk `path` (an index chain into nested folders) down to its last
/// ancestor, returning the list it lives in and its index there. `None`
/// when any ancestor is missing or not a folder, or `path` is empty.
fn locate<'a>(items: &'a mut Vec<MenuItem>, path: &[usize]) -> Option<(&'a mut Vec<MenuItem>, usize)> {
    let (&last, ancestors) = path.split_last()?;
    let mut cur = items;
    for &i in ancestors {
        match cur.get_mut(i) {
            Some(MenuItem::Folder { items, .. }) => cur = items,
            _ => return None,
        }
    }
    if last < cur.len() {
        Some((cur, last))
    } else {
        None
    }
}

/// The item list a folder holds: `path` empty means the root list itself.
/// `None` when `path` does not name a folder.
fn container_at<'a>(items: &'a mut Vec<MenuItem>, path: &[usize]) -> Option<&'a mut Vec<MenuItem>> {
    let mut cur = items;
    for &i in path {
        match cur.get_mut(i) {
            Some(MenuItem::Folder { items, .. }) => cur = items,
            _ => return None,
        }
    }
    Some(cur)
}

/// Append an app entry inside the folder at `folder_path` (empty = root).
/// A no-op when `folder_path` does not name a folder.
pub fn add_app(items: &mut Vec<MenuItem>, folder_path: &[usize], id: &str) {
    if let Some(list) = container_at(items, folder_path) {
        list.push(MenuItem::App { id: id.to_string() });
    }
}

/// Append an empty folder named `name` inside the folder at `folder_path`
/// (empty = root). A no-op when `folder_path` does not name a folder.
pub fn add_folder(items: &mut Vec<MenuItem>, folder_path: &[usize], name: &str) {
    if let Some(list) = container_at(items, folder_path) {
        list.push(MenuItem::Folder { name: name.to_string(), items: Vec::new() });
    }
}

/// Rename the folder at `path`. A no-op when `path` does not name a folder.
pub fn rename_folder(items: &mut Vec<MenuItem>, path: &[usize], name: &str) {
    if let Some((parent, idx)) = locate(items, path) {
        if let Some(MenuItem::Folder { name: n, .. }) = parent.get_mut(idx) {
            *n = name.to_string();
        }
    }
}

/// Remove the entry at `path`. A no-op when `path` is invalid.
pub fn remove_at(items: &mut Vec<MenuItem>, path: &[usize]) {
    if let Some((parent, idx)) = locate(items, path) {
        parent.remove(idx);
    }
}

/// Swap the entry at `path` with its previous sibling. A no-op at the top
/// of its list.
pub fn move_up(items: &mut Vec<MenuItem>, path: &[usize]) {
    if let Some((parent, idx)) = locate(items, path) {
        if idx > 0 {
            parent.swap(idx, idx - 1);
        }
    }
}

/// Swap the entry at `path` with its next sibling. A no-op at the bottom of
/// its list.
pub fn move_down(items: &mut Vec<MenuItem>, path: &[usize]) {
    if let Some((parent, idx)) = locate(items, path) {
        if idx + 1 < parent.len() {
            parent.swap(idx, idx + 1);
        }
    }
}

/// How many entries the folder at `folder_path` (empty = root) holds. `None`
/// when `folder_path` does not name a folder.
pub fn children_len(items: &mut Vec<MenuItem>, folder_path: &[usize]) -> Option<usize> {
    container_at(items, folder_path).map(|l| l.len())
}

/// Move the entry at `from` next to or into `target`: onto a folder it is
/// appended to the folder, onto an app it is inserted before that app.
/// Returns `false` (and changes nothing) when a path is invalid, `from` is
/// `target`, or a folder would land inside itself.
pub fn move_to(items: &mut Vec<MenuItem>, from: &[usize], target: &[usize]) -> bool {
    if from.is_empty() || target.is_empty() || from == target || target.starts_with(from) {
        return false;
    }
    let target_is_folder = {
        let Some((list, idx)) = locate(items, target) else { return false };
        matches!(list.get(idx), Some(MenuItem::Folder { .. }))
    };
    if locate(items, from).is_none() {
        return false;
    }
    // Removing `from` shifts the index of later siblings along the target path.
    let depth = from.len() - 1;
    let mut target = target.to_vec();
    if target.len() > depth && target[..depth] == from[..depth] && from[depth] < target[depth] {
        target[depth] -= 1;
    }
    let Some((list, idx)) = locate(items, from) else { return false };
    let item = list.remove(idx);
    if target_is_folder {
        match container_at(items, &target) {
            Some(dest) => dest.push(item),
            None => return false,
        }
    } else {
        match locate(items, &target) {
            Some((dest, at)) => dest.insert(at, item),
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> MenuItem {
        MenuItem::App { id: id.to_string() }
    }

    fn folder(name: &str, items: Vec<MenuItem>) -> MenuItem {
        MenuItem::Folder { name: name.to_string(), items }
    }

    #[test]
    fn missing_file_is_none() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(load_menu_config_from(&td.path().join("menu.json")), None);
    }

    #[test]
    fn malformed_file_is_none() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("menu.json");
        std::fs::write(&path, b"nope").unwrap();
        assert_eq!(load_menu_config_from(&path), None);
        assert_eq!(std::fs::read_to_string(td.path().join("menu.json.bak")).unwrap(), "nope", "the broken file is kept");
    }

    #[test]
    fn save_then_load_roundtrips_nested_folders() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("nested").join("menu.json");
        let cfg = MenuConfig {
            items: vec![app("a.desktop"), folder("Dev", vec![app("code.desktop")])],
        };
        save_menu_config_to(&path, &cfg).unwrap();
        assert_eq!(load_menu_config_from(&path), Some(cfg));
    }

    #[test]
    fn empty_object_is_an_empty_list_not_defaults() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("menu.json");
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(load_menu_config_from(&path), Some(MenuConfig::default()));
    }

    #[test]
    fn add_app_and_folder_go_into_the_named_folder() {
        let mut items = vec![folder("Dev", vec![])];
        add_folder(&mut items, &[0], "Editors");
        add_app(&mut items, &[0, 0], "code.desktop");
        assert_eq!(items, vec![folder("Dev", vec![folder("Editors", vec![app("code.desktop")])])]);
    }

    #[test]
    fn add_into_root_when_path_is_empty() {
        let mut items = vec![];
        add_app(&mut items, &[], "a.desktop");
        assert_eq!(items, vec![app("a.desktop")]);
    }

    #[test]
    fn add_into_a_non_folder_is_a_no_op() {
        let mut items = vec![app("a.desktop")];
        add_app(&mut items, &[0], "b.desktop");
        assert_eq!(items, vec![app("a.desktop")]);
    }

    #[test]
    fn rename_only_touches_folders() {
        let mut items = vec![folder("Dev", vec![]), app("a.desktop")];
        rename_folder(&mut items, &[0], "Development");
        rename_folder(&mut items, &[1], "ignored");
        assert_eq!(items, vec![folder("Development", vec![]), app("a.desktop")]);
    }

    #[test]
    fn remove_drops_a_nested_entry() {
        let mut items = vec![folder("Dev", vec![app("a.desktop"), app("b.desktop")])];
        remove_at(&mut items, &[0, 0]);
        assert_eq!(items, vec![folder("Dev", vec![app("b.desktop")])]);
    }

    #[test]
    fn move_up_and_down_swap_siblings_and_stop_at_the_ends() {
        let mut items = vec![app("a.desktop"), app("b.desktop"), app("c.desktop")];
        move_up(&mut items, &[0]); // already at the top: no-op
        assert_eq!(items, vec![app("a.desktop"), app("b.desktop"), app("c.desktop")]);
        move_up(&mut items, &[1]);
        assert_eq!(items, vec![app("b.desktop"), app("a.desktop"), app("c.desktop")]);
        move_down(&mut items, &[2]); // already at the bottom: no-op
        assert_eq!(items, vec![app("b.desktop"), app("a.desktop"), app("c.desktop")]);
        move_down(&mut items, &[0]);
        assert_eq!(items, vec![app("a.desktop"), app("b.desktop"), app("c.desktop")]);
    }

    #[test]
    fn invalid_path_is_a_no_op() {
        let mut items = vec![app("a.desktop")];
        remove_at(&mut items, &[5]);
        remove_at(&mut items, &[]);
        move_up(&mut items, &[0, 0]);
        assert_eq!(items, vec![app("a.desktop")]);
    }

    #[test]
    fn move_to_inserts_before_an_app_and_appends_into_a_folder() {
        let mut items = vec![app("a"), app("b"), folder("F", vec![app("c")]), app("d")];
        assert!(move_to(&mut items, &[0], &[3]));
        assert_eq!(items, vec![app("b"), folder("F", vec![app("c")]), app("a"), app("d")]);
        assert!(move_to(&mut items, &[3], &[1]));
        assert_eq!(items, vec![app("b"), folder("F", vec![app("c"), app("d")]), app("a")]);
        assert!(move_to(&mut items, &[1, 1], &[0]));
        assert_eq!(items, vec![app("d"), app("b"), folder("F", vec![app("c")]), app("a")]);
    }

    #[test]
    fn move_to_refuses_a_folder_into_itself_and_bad_paths() {
        let mut items = vec![folder("F", vec![folder("G", vec![])]), app("a")];
        let before = items.clone();
        assert!(!move_to(&mut items, &[0], &[0, 0]));
        assert!(!move_to(&mut items, &[0], &[0]));
        assert!(!move_to(&mut items, &[5], &[0]));
        assert!(!move_to(&mut items, &[1], &[9]));
        assert_eq!(items, before);
    }

    #[test]
    fn children_len_counts_a_folder() {
        let mut items = vec![folder("F", vec![app("a"), app("b")]), app("c")];
        assert_eq!(children_len(&mut items, &[]), Some(2));
        assert_eq!(children_len(&mut items, &[0]), Some(2));
        assert_eq!(children_len(&mut items, &[1]), None);
    }
}
