//! Quick apps: `~/.config/opusdm/quickapps.json`, the icon launchers the
//! panel shows next to the pager. Written by the panel itself (the application
//! menu adds entries, the quick apps bar reorders and removes them).

use crate::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// On-disk shape of `quickapps.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickApps {
    /// Desktop file ids (`firefox.desktop`) or absolute `.desktop` paths, in
    /// display order.
    pub apps: Vec<String>,
}

impl QuickApps {
    /// Append `id`. Returns `false` (and changes nothing) when it is already
    /// present.
    pub fn add(&mut self, id: &str) -> bool {
        if self.contains(id) {
            return false;
        }
        self.apps.push(id.to_string());
        true
    }

    /// Whether `id` is in the list.
    pub fn contains(&self, id: &str) -> bool {
        self.apps.iter().any(|a| a == id)
    }

    /// Remove `id`. A no-op when it is not in the list.
    pub fn remove(&mut self, id: &str) {
        self.apps.retain(|a| a != id);
    }

    /// Move `id` so it sits at the position `target` currently has (the item
    /// takes its place, the others shift). A no-op when either is missing or
    /// they are the same.
    pub fn move_to(&mut self, id: &str, target: &str) {
        if id == target {
            return;
        }
        let (Some(from), Some(to)) = (
            self.apps.iter().position(|a| a == id),
            self.apps.iter().position(|a| a == target),
        ) else {
            return;
        };
        let item = self.apps.remove(from);
        self.apps.insert(to, item);
    }
}

/// Path to `quickapps.json` under [`config_dir`].
pub fn quickapps_path() -> PathBuf {
    config_dir().join("quickapps.json")
}

/// Load the quick apps from the standard path (empty when missing; a
/// malformed file is backed up to `.bak`).
pub fn load_quickapps() -> QuickApps {
    load_quickapps_from(&quickapps_path())
}

/// Testable core of [`load_quickapps`].
pub fn load_quickapps_from(path: &Path) -> QuickApps {
    load_json(path, "ipc", "quick apps")
}

/// Save the quick apps to the standard path, creating the directory.
pub fn save_quickapps(q: &QuickApps) -> io::Result<()> {
    save_quickapps_to(&quickapps_path(), q)
}

/// Testable core of [`save_quickapps`].
pub fn save_quickapps_to(path: &Path, q: &QuickApps) -> io::Result<()> {
    save_json(path, q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qa(ids: &[&str]) -> QuickApps {
        QuickApps { apps: ids.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn add_appends_and_refuses_duplicates() {
        let mut q = qa(&["a"]);
        assert!(q.add("b"));
        assert!(!q.add("a"));
        assert_eq!(q, qa(&["a", "b"]));
    }

    #[test]
    fn remove_drops_only_that_id() {
        let mut q = qa(&["a", "b", "c"]);
        q.remove("b");
        q.remove("zzz");
        assert_eq!(q, qa(&["a", "c"]));
    }

    #[test]
    fn move_to_takes_the_target_position_in_both_directions() {
        let mut q = qa(&["a", "b", "c", "d"]);
        q.move_to("a", "c");
        assert_eq!(q, qa(&["b", "c", "a", "d"]));
        q.move_to("d", "b");
        assert_eq!(q, qa(&["d", "b", "c", "a"]));
    }

    #[test]
    fn move_to_ignores_missing_or_same() {
        let mut q = qa(&["a", "b"]);
        q.move_to("a", "a");
        q.move_to("x", "a");
        q.move_to("a", "x");
        assert_eq!(q, qa(&["a", "b"]));
    }

    #[test]
    fn missing_file_is_empty_and_roundtrip_works() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("sub").join("quickapps.json");
        assert_eq!(load_quickapps_from(&path), QuickApps::default());
        save_quickapps_to(&path, &qa(&["a.desktop", "/x/b.desktop"])).unwrap();
        assert_eq!(load_quickapps_from(&path), qa(&["a.desktop", "/x/b.desktop"]));
    }
}
