//! Merge the left-out icons (`leftout.json`) into the desktop folder listing.
//! Pure filesystem work, no GTK, so it can run on a worker thread and be
//! unit-tested. See `docs/superpowers/specs/2026-09-30-leaveout-design.md`.

use opusdm_iconview::scan;
use opusdm_iconview::{IconEntry, IconKind};
use opusdm_ipc::LeftOut;
use std::io;
use std::path::Path;

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

/// Read the Desktop folder `dir` (hidden files skipped) and merge in `left`.
pub fn read_desktop_entries(dir: &Path, left: &LeftOut) -> io::Result<Vec<IconEntry>> {
    Ok(merge_entries(scan::read_entries(dir, false)?, left))
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
        let got = read_desktop_entries(td.path(), &left).unwrap();
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
        let got = read_desktop_entries(desk.path(), &left).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "mine.txt");
        assert!(!got[0].left_out);
        assert_eq!((got[1].name.as_str(), got[1].kind, got[1].left_out), ("proj", IconKind::Dir, true));
    }

    #[test]
    fn a_missing_target_becomes_a_flagged_broken_entry_and_stays_in_the_store() {
        let desk = tempfile::tempdir().unwrap();
        let left = store(&[Path::new("/definitely/not/here/movie.mkv")]);
        let got = read_desktop_entries(desk.path(), &left).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].name.as_str(), got[0].kind, got[0].left_out), ("movie.mkv", IconKind::Broken, true));
        assert_eq!(left.items.len(), 1, "merging never edits the store");
    }

    #[test]
    fn left_out_entries_follow_store_order_after_the_desktop_ones() {
        let desk = tempfile::tempdir().unwrap();
        std::fs::write(desk.path().join("z.txt"), b"x").unwrap();
        let left = store(&[Path::new("/nope/b"), Path::new("/nope/a")]);
        let got = read_desktop_entries(desk.path(), &left).unwrap();
        let names: Vec<_> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["z.txt", "b", "a"]);
    }

    #[test]
    fn an_empty_store_changes_nothing() {
        let desk = tempfile::tempdir().unwrap();
        std::fs::write(desk.path().join("a.txt"), b"x").unwrap();
        let got = read_desktop_entries(desk.path(), &LeftOut::default()).unwrap();
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
}
