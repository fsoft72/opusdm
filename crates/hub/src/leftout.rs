//! Merge the left-out icons (`leftout.json`) into the desktop folder listing.
//! Pure filesystem work, no GTK, so it can run on a worker thread and be
//! unit-tested. See `docs/superpowers/specs/2026-09-30-leaveout-design.md`.

use opusdm_iconview::scan;
use opusdm_iconview::{IconEntry, IconKind};
use opusdm_ipc::groups::group_path;
use opusdm_ipc::{Group, Groups, LeftOut};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Combine the Desktop folder `desktop` entries with the left-out items. A
/// left-out path already listed is flagged in place (it appears once); the
/// others are appended in store order, built with [`scan::entry_for_path`]
/// (so a missing target is an `IconKind::Broken` entry) and flagged.
pub fn merge_entries(mut desktop: Vec<IconEntry>, left: &LeftOut) -> Vec<IconEntry> {
    for item in &left.items {
        if let Some(existing) = desktop.iter_mut().find(|e| e.path == item.path) {
            existing.left_out = true;
            continue;
        }
        let mut entry = scan::entry_for_path(&item.path);
        entry.left_out = true;
        desktop.push(entry);
    }
    desktop
}

/// What the desktop shows besides the contents of the Desktop folder: the
/// left-out icons and the program groups.
#[derive(Clone, Default)]
pub struct DesktopExtras {
    pub left_out: LeftOut,
    pub groups: Groups,
}

impl From<LeftOut> for DesktopExtras {
    /// Left-out icons only, no groups.
    fn from(left_out: LeftOut) -> Self {
        DesktopExtras { left_out, groups: Groups::default() }
    }
}

/// The desktop icon of `group`: the folder icon, flagged `group`, keyed by its
/// virtual `group:<id>` path.
pub fn group_entry(group: &Group) -> IconEntry {
    IconEntry {
        name: group.name.clone(),
        path: group_path(&group.id),
        kind: IconKind::Dir,
        mode: 0o040_755,
        ext: None,
        left_out: false,
        group: true,
    }
}

impl DesktopExtras {
    /// `desktop` (the Desktop folder entries) merged with the left-out icons
    /// (see [`merge_entries`]), then one group icon per group in store order.
    pub fn merge(&self, desktop: Vec<IconEntry>) -> Vec<IconEntry> {
        let mut entries = merge_entries(desktop, &self.left_out);
        entries.extend(self.groups.groups.iter().map(group_entry));
        entries
    }

    /// The saved position of every left-out icon (by real path) and group
    /// (by virtual path) that has one.
    pub fn positions(&self) -> HashMap<PathBuf, (f64, f64)> {
        let mut all = self.left_out.positions();
        all.extend(self.groups.positions());
        all
    }
}

/// Read the Desktop folder `dir` (hidden files skipped) and merge in
/// `extras`.
pub fn read_desktop_entries(dir: &Path, extras: &DesktopExtras) -> io::Result<Vec<IconEntry>> {
    Ok(extras.merge(scan::read_entries(dir, false)?))
}

/// How a group member is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchAction {
    /// Open a Lister on it (a folder, or a symlink to one).
    Lister,
    /// Launch the application a `.desktop` file describes.
    DesktopEntry,
    /// Run the executable regular file.
    Program,
    /// Hand it to the default application.
    Default,
}

/// Decide how `entry` is started when double-clicked in a group.
pub fn launch_action(entry: &IconEntry) -> LaunchAction {
    if opens_in_lister(entry) {
        return LaunchAction::Lister;
    }
    if entry.kind == IconKind::File && entry.ext.as_deref() == Some("desktop") {
        return LaunchAction::DesktopEntry;
    }
    if entry.kind == IconKind::File && entry.mode & 0o111 != 0 {
        return LaunchAction::Program;
    }
    LaunchAction::Default
}

/// Latest-wins gate for asynchronous desktop reloads. Every reload takes a
/// ticket with [`ReloadGate::begin`] and, when its (off-thread) read finishes,
/// applies the result only if [`ReloadGate::is_latest`] still holds, so a slow
/// read can never overwrite the result of a newer one.
#[derive(Default)]
pub struct ReloadGate {
    latest: std::cell::Cell<u64>,
}

impl ReloadGate {
    /// Start a reload: returns its ticket and makes every older one stale.
    pub fn begin(&self) -> u64 {
        let ticket = self.latest.get() + 1;
        self.latest.set(ticket);
        ticket
    }

    /// Whether `ticket` is still the newest one handed out.
    pub fn is_latest(&self, ticket: u64) -> bool {
        self.latest.get() == ticket
    }
}

/// Whether activating `entry` should open a Lister on it: a folder, or a
/// symlink whose target is a folder. Everything else goes to the default
/// application.
pub fn opens_in_lister(entry: &IconEntry) -> bool {
    match entry.kind {
        IconKind::Dir => true,
        IconKind::Symlink => entry.path.is_dir(),
        IconKind::File | IconKind::Broken => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(paths: &[&Path]) -> LeftOut {
        let mut lo = LeftOut::default();
        lo.add(&paths.iter().map(|p| p.to_path_buf()).collect::<Vec<_>>());
        lo
    }

    #[test]
    fn a_path_in_both_the_desktop_folder_and_the_store_shows_once_flagged() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a.txt"), b"x").unwrap();
        std::fs::write(td.path().join("b.txt"), b"x").unwrap();
        let left = store(&[&td.path().join("a.txt")]);
        let got = read_desktop_entries(td.path(), &DesktopExtras::from(left.clone())).unwrap();
        assert_eq!(got.len(), 2);
        let a = got.iter().find(|e| e.name == "a.txt").unwrap();
        let b = got.iter().find(|e| e.name == "b.txt").unwrap();
        assert!(a.left_out);
        assert!(!b.left_out);
    }

    #[test]
    fn a_left_out_path_outside_the_desktop_is_appended_flagged() {
        let desk = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(desk.path().join("mine.txt"), b"x").unwrap();
        std::fs::create_dir(other.path().join("proj")).unwrap();
        let left = store(&[&other.path().join("proj")]);
        let got = read_desktop_entries(desk.path(), &DesktopExtras::from(left.clone())).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "mine.txt");
        assert!(!got[0].left_out);
        assert_eq!((got[1].name.as_str(), got[1].kind, got[1].left_out), ("proj", IconKind::Dir, true));
    }

    #[test]
    fn a_missing_target_becomes_a_flagged_broken_entry_and_stays_in_the_store() {
        let desk = tempfile::tempdir().unwrap();
        let left = store(&[Path::new("/definitely/not/here/movie.mkv")]);
        let got = read_desktop_entries(desk.path(), &DesktopExtras::from(left.clone())).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].name.as_str(), got[0].kind, got[0].left_out), ("movie.mkv", IconKind::Broken, true));
        assert_eq!(left.items.len(), 1, "merging never edits the store");
    }

    #[test]
    fn left_out_entries_follow_store_order_after_the_desktop_ones() {
        let desk = tempfile::tempdir().unwrap();
        std::fs::write(desk.path().join("z.txt"), b"x").unwrap();
        let left = store(&[Path::new("/nope/b"), Path::new("/nope/a")]);
        let got = read_desktop_entries(desk.path(), &DesktopExtras::from(left.clone())).unwrap();
        let names: Vec<_> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["z.txt", "b", "a"]);
    }

    #[test]
    fn an_empty_store_changes_nothing() {
        let desk = tempfile::tempdir().unwrap();
        std::fs::write(desk.path().join("a.txt"), b"x").unwrap();
        let got = read_desktop_entries(desk.path(), &DesktopExtras::default()).unwrap();
        assert_eq!(got.len(), 1);
        assert!(!got[0].left_out);
    }

    #[test]
    fn reload_gate_only_accepts_the_latest_ticket() {
        let gate = ReloadGate::default();
        let first = gate.begin();
        assert!(gate.is_latest(first));
        let second = gate.begin();
        assert!(!gate.is_latest(first), "an older reload must be dropped");
        assert!(gate.is_latest(second));
    }

    #[test]
    fn opens_in_lister_covers_dirs_and_symlinks_to_dirs_only() {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        std::fs::create_dir(base.join("d")).unwrap();
        std::fs::write(base.join("f.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(base.join("d"), base.join("link_to_dir")).unwrap();
        std::os::unix::fs::symlink(base.join("f.txt"), base.join("link_to_file")).unwrap();
        std::os::unix::fs::symlink(base.join("nope"), base.join("dangling")).unwrap();
        let entry = |name: &str| opusdm_iconview::scan::entry_for_path(&base.join(name));
        assert!(opens_in_lister(&entry("d")));
        assert!(opens_in_lister(&entry("link_to_dir")));
        assert!(!opens_in_lister(&entry("f.txt")));
        assert!(!opens_in_lister(&entry("link_to_file")));
        assert!(!opens_in_lister(&entry("dangling")));
    }

    fn groups_with(names: &[&str]) -> opusdm_ipc::Groups {
        let mut g = opusdm_ipc::Groups::default();
        for n in names {
            g.create(n).unwrap();
        }
        g
    }

    #[test]
    fn group_icons_come_after_the_desktop_and_left_out_entries_and_are_flagged() {
        let desk = tempfile::tempdir().unwrap();
        std::fs::write(desk.path().join("a.txt"), b"x").unwrap();
        let extras = DesktopExtras {
            left_out: store(&[Path::new("/nope/lo")]),
            groups: groups_with(&["Editors", "Games"]),
        };
        let got = read_desktop_entries(desk.path(), &extras).unwrap();
        let names: Vec<_> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "lo", "Editors", "Games"]);
        let g = &got[2];
        assert_eq!((g.kind, g.group, g.left_out), (IconKind::Dir, true, false));
        assert_eq!(g.path, PathBuf::from("group:editors"));
        assert!(!got[0].group && !got[1].group);
    }

    #[test]
    fn positions_merge_left_out_paths_and_group_paths() {
        let mut left = store(&[Path::new("/a")]);
        left.set_position(Path::new("/a"), 1.0, 2.0);
        let mut groups = groups_with(&["Tools"]);
        groups.set_position("tools", 3.0, 4.0);
        let pos = DesktopExtras { left_out: left, groups }.positions();
        assert_eq!(pos.len(), 2);
        assert_eq!(pos.get(Path::new("/a")), Some(&(1.0, 2.0)));
        assert_eq!(pos.get(Path::new("group:tools")), Some(&(3.0, 4.0)));
    }

    #[test]
    fn launch_action_classifies_group_members() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        std::fs::create_dir(base.join("dir")).unwrap();
        std::fs::write(base.join("app.desktop"), b"[Desktop Entry]").unwrap();
        std::fs::write(base.join("tool"), b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(base.join("tool"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(base.join("notes.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(base.join("dir"), base.join("link_to_dir")).unwrap();
        let action = |name: &str| launch_action(&opusdm_iconview::scan::entry_for_path(&base.join(name)));
        assert_eq!(action("dir"), LaunchAction::Lister);
        assert_eq!(action("link_to_dir"), LaunchAction::Lister);
        assert_eq!(action("app.desktop"), LaunchAction::DesktopEntry);
        assert_eq!(action("tool"), LaunchAction::Program);
        assert_eq!(action("notes.txt"), LaunchAction::Default);
        assert_eq!(action("missing"), LaunchAction::Default);
    }
}
