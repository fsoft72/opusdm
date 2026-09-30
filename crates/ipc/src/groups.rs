//! Program groups: named collections of programs, files or folders shown as
//! one icon on the desktop. One JSON file per group under
//! `~/.config/opusdm/groups/`. See
//! `docs/superpowers/specs/2026-09-30-program-groups-design.md`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

/// Prefix of the virtual path an icon field uses to key a group's icon.
pub const GROUP_PATH_PREFIX: &str = "group:";

/// Base directory of the group files (`~/.config/opusdm/groups`).
pub fn groups_dir() -> PathBuf {
    crate::settings::config_dir().join("groups")
}

/// The virtual path of the desktop icon of group `id`. Real paths are always
/// absolute, so this relative path can never collide with one.
pub fn group_path(id: &str) -> PathBuf {
    PathBuf::from(format!("{GROUP_PATH_PREFIX}{id}"))
}

/// The group id behind a virtual path made by [`group_path`], or `None` for
/// any other path.
pub fn group_id(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix(GROUP_PATH_PREFIX).filter(|id| !id.is_empty())
}

/// File-name-safe id for `name`: lowercase ASCII letters and digits, every run
/// of other characters becomes one `-`, no leading or trailing `-`. An empty
/// result is `"group"`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "group".to_string()
    } else {
        out
    }
}

/// One group: its file id, display name, members (in the order they were
/// added) and, once dragged, its desktop position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Group {
    /// File stem of the group's JSON file. Not stored inside the file.
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub items: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
}

impl Group {
    /// Drop duplicate and non-absolute members, and clear a position whose
    /// coordinates are not finite and non-negative.
    fn cleaned(mut self) -> Group {
        let mut seen = HashSet::new();
        self.items.retain(|p| p.is_absolute() && seen.insert(p.clone()));
        let ok = matches!((self.x, self.y), (Some(x), Some(y)) if x.is_finite() && x >= 0.0 && y.is_finite() && y >= 0.0);
        if !ok {
            self.x = None;
            self.y = None;
        }
        self
    }
}

/// Every group, in file-name order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Groups {
    pub groups: Vec<Group>,
}

impl Groups {
    /// The group with `id`.
    pub fn get(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Whether some group other than `except` already has `name`, ignoring
    /// case.
    fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        self.groups
            .iter()
            .any(|g| Some(g.id.as_str()) != except && g.name.to_lowercase() == name.to_lowercase())
    }

    /// Create an empty group called `name` (trimmed). Returns its id, unique
    /// among the groups. Empty or duplicate names are refused. Pure: it knows
    /// nothing about files, see [`Groups::create_in_dir`] for the real thing.
    pub fn create(&mut self, name: &str) -> Result<String, String> {
        self.create_with(name, |_| false)
    }

    /// Like [`Groups::create`], but the id also avoids every `<id>.json`
    /// already present in `dir`, including files that failed to load, so a new
    /// group can never overwrite a file it did not read.
    pub fn create_in_dir(&mut self, dir: &Path, name: &str) -> Result<String, String> {
        self.create_with(name, |id| dir.join(format!("{id}.json")).exists())
    }

    /// [`Groups::create`] with an extra `taken` test on candidate ids.
    fn create_with(&mut self, name: &str, taken: impl Fn(&str) -> bool) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Group name is empty".to_string());
        }
        if self.name_taken(name, None) {
            return Err(format!("A group named \"{name}\" already exists"));
        }
        let base = slugify(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() || taken(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.groups.push(Group { id: id.clone(), name: name.to_string(), ..Group::default() });
        Ok(id)
    }

    /// Rename group `id` to `name` (trimmed); the id does not change.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Group name is empty".to_string());
        }
        if self.get(id).is_none() {
            return Err(format!("No group \"{id}\""));
        }
        if self.name_taken(name, Some(id)) {
            return Err(format!("A group named \"{name}\" already exists"));
        }
        if let Some(g) = self.get_mut(id) {
            g.name = name.to_string();
        }
        Ok(())
    }

    /// Remove group `id`. Returns whether it existed. The caller removes the
    /// file with [`Groups::remove_group_file_from`].
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| g.id != id);
        self.groups.len() != before
    }

    /// Append the absolute paths not already in group `id`. Returns how many
    /// were added (0 for an unknown group).
    pub fn add_items(&mut self, id: &str, paths: &[PathBuf]) -> usize {
        let Some(group) = self.get_mut(id) else { return 0 };
        let mut added = 0;
        for path in paths {
            if !path.is_absolute() || group.items.contains(path) {
                continue;
            }
            group.items.push(path.clone());
            added += 1;
        }
        added
    }

    /// Drop `path` from group `id`. Unknown groups and paths are ignored.
    pub fn remove_item(&mut self, id: &str, path: &Path) {
        if let Some(group) = self.get_mut(id) {
            group.items.retain(|p| p != path);
        }
    }

    /// Record where the icon of group `id` sits. Unknown groups are ignored.
    pub fn set_position(&mut self, id: &str, x: f64, y: f64) {
        if let Some(group) = self.get_mut(id) {
            group.x = Some(x);
            group.y = Some(y);
        }
    }

    /// The saved position of every group that has both coordinates, keyed by
    /// [`group_path`].
    pub fn positions(&self) -> HashMap<PathBuf, (f64, f64)> {
        self.groups
            .iter()
            .filter_map(|g| Some((group_path(&g.id), (g.x?, g.y?))))
            .collect()
    }

    /// Load every `*.json` file of `dir` (sorted by name). A missing directory
    /// gives no groups; an unreadable or malformed file is skipped with a log
    /// line and left alone. Members and positions are cleaned; an empty name
    /// falls back to the file name.
    pub fn load_from_dir(dir: &Path) -> Groups {
        let Ok(read) = std::fs::read_dir(dir) else { return Groups::default() };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        let mut groups = Vec::new();
        for path in files {
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("opusdm: cannot read group {}: {e}", path.display());
                    continue;
                }
            };
            match serde_json::from_str::<Group>(&text) {
                Ok(mut group) => {
                    group.id = id.clone();
                    if group.name.trim().is_empty() {
                        group.name = id;
                    }
                    groups.push(group.cleaned());
                }
                Err(e) => eprintln!("opusdm: malformed group file {}: {e}", path.display()),
            }
        }
        Groups { groups }
    }

    /// [`Groups::load_from_dir`] the standard directory.
    pub fn load() -> Groups {
        Groups::load_from_dir(&groups_dir())
    }

    /// Write group `id` to `<dir>/<id>.json`, creating `dir`. An unknown id is
    /// an error.
    pub fn save_group_to(&self, dir: &Path, id: &str) -> io::Result<()> {
        let group = self
            .get(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no group \"{id}\"")))?;
        crate::settings::save_json(&dir.join(format!("{id}.json")), group)
    }

    /// [`Groups::save_group_to`] the standard directory.
    pub fn save_group(&self, id: &str) -> io::Result<()> {
        self.save_group_to(&groups_dir(), id)
    }

    /// Delete `<dir>/<id>.json`. A file that is already gone is not an error.
    pub fn remove_group_file_from(dir: &Path, id: &str) -> io::Result<()> {
        match std::fs::remove_file(dir.join(format!("{id}.json"))) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// [`Groups::remove_group_file_from`] the standard directory.
    pub fn remove_group_file(id: &str) -> io::Result<()> {
        Groups::remove_group_file_from(&groups_dir(), id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn slugify_keeps_lowercase_letters_and_digits_and_joins_the_rest_with_dashes() {
        assert_eq!(slugify("Editors"), "editors");
        assert_eq!(slugify("  My  Tools! v2 "), "my-tools-v2");
        assert_eq!(slugify("Caff\u{e8} & Co"), "caff-co");
        assert_eq!(slugify("!!!"), "group");
        assert_eq!(slugify(""), "group");
    }

    #[test]
    fn create_gives_unique_ids_and_refuses_empty_or_duplicate_names() {
        let mut g = Groups::default();
        let a = g.create("A b").unwrap();
        let b = g.create("a-b").unwrap();
        assert_eq!(a, "a-b");
        assert_eq!(b, "a-b-2");
        assert!(g.create("   ").unwrap_err().contains("empty"));
        assert!(g.create("  A B ").unwrap_err().contains("already exists"));
        assert_eq!(g.groups.len(), 2);
    }

    #[test]
    fn rename_keeps_the_id_and_refuses_a_name_taken_by_another_group() {
        let mut g = Groups::default();
        let a = g.create("Alpha").unwrap();
        g.create("Beta").unwrap();
        assert!(g.rename(&a, "beta").unwrap_err().contains("already exists"));
        g.rename(&a, "Alpha 2").unwrap();
        assert_eq!(g.get(&a).unwrap().name, "Alpha 2");
        g.rename(&a, "ALPHA 2").unwrap(); // same group, other case: allowed
        assert!(g.rename("nope", "X").is_err());
    }

    #[test]
    fn delete_removes_only_the_named_group() {
        let mut g = Groups::default();
        let a = g.create("A").unwrap();
        g.create("B").unwrap();
        assert!(g.delete(&a));
        assert!(!g.delete(&a));
        assert_eq!(g.groups.len(), 1);
    }

    #[test]
    fn add_items_skips_duplicates_and_relative_paths_and_counts_what_it_added() {
        let mut g = Groups::default();
        let id = g.create("Tools").unwrap();
        assert_eq!(g.add_items(&id, &[p("/usr/bin/a"), p("/usr/bin/b"), p("relative")]), 2);
        assert_eq!(g.add_items(&id, &[p("/usr/bin/a"), p("/usr/bin/c")]), 1);
        assert_eq!(g.add_items("nope", &[p("/x")]), 0);
        let items = &g.get(&id).unwrap().items;
        assert_eq!(items, &vec![p("/usr/bin/a"), p("/usr/bin/b"), p("/usr/bin/c")]);
    }

    #[test]
    fn remove_item_drops_one_member_and_ignores_unknown_ones() {
        let mut g = Groups::default();
        let id = g.create("Tools").unwrap();
        g.add_items(&id, &[p("/a"), p("/b")]);
        g.remove_item(&id, Path::new("/a"));
        g.remove_item(&id, Path::new("/never"));
        g.remove_item("nope", Path::new("/b"));
        assert_eq!(g.get(&id).unwrap().items, vec![p("/b")]);
    }

    #[test]
    fn positions_are_keyed_by_the_virtual_group_path() {
        let mut g = Groups::default();
        let id = g.create("Tools").unwrap();
        g.create("No position").unwrap();
        g.set_position(&id, 10.0, 20.0);
        g.set_position("nope", 1.0, 2.0);
        let pos = g.positions();
        assert_eq!(pos.len(), 1);
        assert_eq!(pos.get(&group_path(&id)), Some(&(10.0, 20.0)));
    }

    #[test]
    fn group_path_and_group_id_roundtrip_and_reject_real_paths() {
        let path = group_path("my-tools");
        assert_eq!(path, p("group:my-tools"));
        assert_eq!(group_id(&path), Some("my-tools"));
        assert_eq!(group_id(Path::new("/usr/bin/tool")), None);
        assert_eq!(group_id(Path::new("group:")), None);
    }

    #[test]
    fn save_and_load_roundtrip_one_file_per_group() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("groups");
        let mut g = Groups::default();
        let a = g.create("Editors").unwrap();
        let b = g.create("Games").unwrap();
        g.add_items(&a, &[p("/usr/bin/gedit"), p("/usr/bin/nano")]);
        g.set_position(&a, 120.0, 80.0);
        g.save_group_to(&dir, &a).unwrap();
        g.save_group_to(&dir, &b).unwrap();
        assert!(dir.join("editors.json").is_file());
        assert!(dir.join("games.json").is_file());
        assert_eq!(Groups::load_from_dir(&dir), g);
        g.delete(&b);
        Groups::remove_group_file_from(&dir, &b).unwrap();
        assert!(!dir.join("games.json").exists());
        assert_eq!(Groups::load_from_dir(&dir), g);
        Groups::remove_group_file_from(&dir, "never-existed").unwrap();
    }

    #[test]
    fn load_skips_a_malformed_file_and_never_deletes_it() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("bad.json"), b"{ not json").unwrap();
        std::fs::write(td.path().join("ok.json"), br#"{"name":"Fine","items":["/a","/a","rel"]}"#).unwrap();
        std::fs::write(td.path().join("notes.txt"), b"ignored").unwrap();
        let g = Groups::load_from_dir(td.path());
        assert_eq!(g.groups.len(), 1);
        assert_eq!(g.groups[0].id, "ok");
        assert_eq!(g.groups[0].items, vec![p("/a")], "duplicates and relative paths cleaned");
        assert!(td.path().join("bad.json").exists());
    }

    #[test]
    fn load_clears_bad_coordinates_and_falls_back_to_the_file_name_for_an_empty_name() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("neg.json"), br#"{"name":"","x":-3.0,"y":4.0}"#).unwrap();
        let g = Groups::load_from_dir(td.path());
        assert_eq!(g.groups.len(), 1);
        assert_eq!(g.groups[0].name, "neg");
        assert_eq!((g.groups[0].x, g.groups[0].y), (None, None));
    }

    #[test]
    fn load_from_a_missing_directory_is_empty() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(Groups::load_from_dir(&td.path().join("nope")), Groups::default());
    }

    #[test]
    fn create_in_dir_never_reuses_an_id_whose_file_exists_even_if_it_failed_to_load() {
        let td = tempfile::tempdir().unwrap();
        let bad = td.path().join("foo.json");
        std::fs::write(&bad, b"{ truncated").unwrap();
        let mut g = Groups::load_from_dir(td.path());
        assert!(g.groups.is_empty(), "the malformed file is skipped");
        let id = g.create_in_dir(td.path(), "Foo").unwrap();
        assert_eq!(id, "foo-2");
        g.save_group_to(td.path(), &id).unwrap();
        assert_eq!(std::fs::read(&bad).unwrap(), b"{ truncated", "the original file is untouched");
    }
}
