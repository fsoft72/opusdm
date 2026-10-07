//! Icons "left out" on the desktop: permanent shortcuts to any file or folder,
//! stored in `~/.config/opusdm/leftout.json`. See
//! `docs/superpowers/specs/2026-09-30-leaveout-design.md`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

/// File name of the store under the OpusDM config directory.
pub const LEFTOUT_FILE: &str = "leftout.json";

/// Path of `leftout.json` under the OpusDM config directory.
pub fn leftout_path() -> PathBuf {
    crate::settings::config_dir().join(LEFTOUT_FILE)
}

/// One left-out entry: the target path and, once the user dragged it, where
/// the icon sits on the desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeftOutItem {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
}

/// The whole set of left-out icons, in the order they were left out.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LeftOut {
    pub items: Vec<LeftOutItem>,
}

/// Whether `v` is a usable icon coordinate: finite and not negative.
fn coordinate_ok(v: f64) -> bool {
    v.is_finite() && v >= 0.0
}

impl LeftOut {
    /// Append every path not already present, keeping the existing order.
    pub fn add(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if self.items.iter().any(|i| &i.path == path) {
                continue;
            }
            self.items.push(LeftOutItem { path: path.clone(), x: None, y: None });
        }
    }

    /// Drop every item whose path is in `paths`. Unknown paths are ignored.
    pub fn remove(&mut self, paths: &[PathBuf]) {
        self.items.retain(|i| !paths.contains(&i.path));
    }

    /// Record where the icon of `path` sits. Unknown paths are ignored.
    pub fn set_position(&mut self, path: &Path, x: f64, y: f64) {
        let Some(item) = self.items.iter_mut().find(|i| i.path == path) else { return };
        item.x = Some(x);
        item.y = Some(y);
    }

    /// The saved position of every item that has both coordinates.
    pub fn positions(&self) -> HashMap<PathBuf, (f64, f64)> {
        self.items
            .iter()
            .filter_map(|i| Some((i.path.clone(), (i.x?, i.y?))))
            .collect()
    }

    /// Drop duplicate paths (first wins) and clear the position of any item
    /// whose coordinates are not finite and non-negative. Items are never
    /// dropped for a bad position.
    pub fn sanitized(mut self) -> LeftOut {
        let mut seen = HashSet::new();
        self.items.retain(|i| seen.insert(i.path.clone()));
        for item in &mut self.items {
            let ok = matches!((item.x, item.y), (Some(x), Some(y)) if coordinate_ok(x) && coordinate_ok(y));
            if !ok {
                item.x = None;
                item.y = None;
            }
        }
        self
    }

    /// Load from `path`: missing or malformed gives an empty store, then
    /// [`LeftOut::sanitized`].
    pub fn load_from(path: &Path) -> LeftOut {
        crate::settings::load_json::<LeftOut>(path, "opusdm", "left-out icons").sanitized()
    }

    /// Save to `path`, creating the parent directory.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        crate::settings::save_json(path, self)
    }

    /// [`LeftOut::load_from`] the standard path.
    pub fn load() -> LeftOut {
        LeftOut::load_from(&leftout_path())
    }

    /// [`LeftOut::save_to`] the standard path.
    pub fn save(&self) -> io::Result<()> {
        self.save_to(&leftout_path())
    }
}

/// Check that every path can be left out: absolute, and present on disk (a
/// dangling symlink counts as present). The error names the first offender.
pub fn check_leavable(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        if !path.is_absolute() {
            return Err(format!("Leave Out: {} is not an absolute path", path.display()));
        }
        if std::fs::symlink_metadata(path).is_err() {
            return Err(format!("Leave Out: {} does not exist", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn add_skips_duplicates_and_keeps_order() {
        let mut lo = LeftOut::default();
        lo.add(&[p("/b"), p("/a")]);
        lo.add(&[p("/a"), p("/c")]);
        let paths: Vec<_> = lo.items.iter().map(|i| i.path.clone()).collect();
        assert_eq!(paths, vec![p("/b"), p("/a"), p("/c")]);
    }

    #[test]
    fn remove_drops_only_the_named_paths_and_ignores_unknown_ones() {
        let mut lo = LeftOut::default();
        lo.add(&[p("/a"), p("/b")]);
        lo.remove(&[p("/a"), p("/never-left-out")]);
        assert_eq!(lo.items.len(), 1);
        assert_eq!(lo.items[0].path, p("/b"));
    }

    #[test]
    fn set_position_updates_known_paths_and_ignores_unknown_ones() {
        let mut lo = LeftOut::default();
        lo.add(&[p("/a")]);
        lo.set_position(Path::new("/a"), 10.0, 20.0);
        lo.set_position(Path::new("/zzz"), 1.0, 2.0);
        assert_eq!(lo.items.len(), 1);
        assert_eq!(lo.positions().get(&p("/a")), Some(&(10.0, 20.0)));
    }

    #[test]
    fn positions_skips_items_without_both_coordinates() {
        let mut lo = LeftOut::default();
        lo.add(&[p("/a")]);
        lo.items.push(LeftOutItem { path: p("/half"), x: Some(1.0), y: None });
        assert!(lo.positions().is_empty());
    }

    #[test]
    fn json_roundtrip_keeps_items_and_positions() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("nested").join("leftout.json");
        let mut lo = LeftOut::default();
        lo.add(&[p("/etc/fstab"), p("/home")]);
        lo.set_position(Path::new("/etc/fstab"), 120.0, 80.0);
        lo.save_to(&file).unwrap();
        assert_eq!(LeftOut::load_from(&file), lo);
    }

    #[test]
    fn load_from_a_missing_or_malformed_file_is_empty() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(LeftOut::load_from(&td.path().join("nope.json")), LeftOut::default());
        let bad = td.path().join("bad.json");
        std::fs::write(&bad, b"{ not json").unwrap();
        assert_eq!(LeftOut::load_from(&bad), LeftOut::default());
    }

    #[test]
    fn load_drops_bad_coordinates_but_keeps_the_item() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("leftout.json");
        std::fs::write(
            &file,
            br#"{"items":[{"path":"/neg","x":-5.0,"y":10.0},{"path":"/ok","x":1.0,"y":2.0}]}"#,
        )
        .unwrap();
        let lo = LeftOut::load_from(&file);
        assert_eq!(lo.items.len(), 2);
        assert_eq!((lo.items[0].x, lo.items[0].y), (None, None));
        assert_eq!(lo.positions().get(&p("/ok")), Some(&(1.0, 2.0)));
    }

    #[test]
    fn sanitized_drops_non_finite_coordinates_and_duplicate_paths() {
        let lo = LeftOut {
            items: vec![
                LeftOutItem { path: p("/nan"), x: Some(f64::NAN), y: Some(1.0) },
                LeftOutItem { path: p("/inf"), x: Some(1.0), y: Some(f64::INFINITY) },
                LeftOutItem { path: p("/nan"), x: Some(3.0), y: Some(4.0) },
            ],
        }
        .sanitized();
        assert_eq!(lo.items.len(), 2);
        assert!(lo.positions().is_empty());
    }

    #[test]
    fn check_leavable_requires_an_absolute_existing_path() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(check_leavable(std::slice::from_ref(&file)).is_ok());
        let err = check_leavable(&[p("relative.txt")]).unwrap_err();
        assert!(err.contains("not an absolute path") && err.contains("relative.txt"), "{err}");
        let err = check_leavable(&[file, td.path().join("gone")]).unwrap_err();
        assert!(err.contains("does not exist") && err.contains("gone"), "{err}");
    }

    #[test]
    fn check_leavable_accepts_a_dangling_symlink() {
        let td = tempfile::tempdir().unwrap();
        let link = td.path().join("dangling");
        std::os::unix::fs::symlink(td.path().join("gone"), &link).unwrap();
        assert!(check_leavable(&[link]).is_ok());
    }
}
