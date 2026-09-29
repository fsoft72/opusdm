//! Lister formats (F-22): which columns a Lister shows, how it sorts and
//! separates directories, and its show/hide name patterns. Formats are saved
//! per directory in `formats.json` and resolved again on every navigation.

use crate::settings::{config_dir, save_json};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// A column a Lister can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormatColumn {
    Name,
    Size,
    Date,
    User,
    Group,
    Access,
}

impl FormatColumn {
    /// Every column, in the built-in display order.
    pub const ALL: [FormatColumn; 6] = [
        FormatColumn::Name,
        FormatColumn::Size,
        FormatColumn::Date,
        FormatColumn::User,
        FormatColumn::Group,
        FormatColumn::Access,
    ];

    /// The column header text.
    pub fn title(self) -> &'static str {
        match self {
            FormatColumn::Name => "Name",
            FormatColumn::Size => "Size",
            FormatColumn::Date => "Date",
            FormatColumn::User => "User",
            FormatColumn::Group => "Group",
            FormatColumn::Access => "Access",
        }
    }
}

/// Where directories sit relative to files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Separate {
    /// Directories form a block above the files.
    #[default]
    DirsFirst,
    /// Directories form a block below the files.
    FilesFirst,
    /// Directories and files are sorted together.
    Mixed,
}

/// Everything a Lister needs to lay out one directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListerFormat {
    /// Shown columns, in display order.
    pub columns: Vec<FormatColumn>,
    /// Column the list is sorted on.
    pub sort: FormatColumn,
    /// Sort descending.
    pub reverse: bool,
    /// Directory / file separation.
    pub separate: Separate,
    /// Show pattern list; empty means "show everything".
    pub show_glob: String,
    /// Hide pattern list; empty means "hide nothing".
    pub hide_glob: String,
    /// Show the free-space gauge in the status bar.
    pub gauge: bool,
}

impl Default for ListerFormat {
    fn default() -> Self {
        Self::builtin()
    }
}

impl ListerFormat {
    /// The format used when nothing is saved: all columns, sorted by name.
    pub fn builtin() -> Self {
        Self {
            columns: FormatColumn::ALL.to_vec(),
            sort: FormatColumn::Name,
            reverse: false,
            separate: Separate::DirsFirst,
            show_glob: String::new(),
            hide_glob: String::new(),
            gauge: false,
        }
    }

    /// A marker for "whatever is on screen is unknown": no columns, so it never
    /// equals a sanitized format. A Lister stores it after the user changes
    /// the format so the next listing reapplies the format even when it equals
    /// the previous one (a header click may have changed the live sort).
    pub fn stale() -> Self {
        Self { columns: Vec::new(), ..Self::builtin() }
    }

    /// Repair a hand-edited format: no duplicate columns, `Name` always
    /// present (appended when missing), and a `sort` that is a shown column.
    pub fn sanitized(mut self) -> Self {
        let mut seen: Vec<FormatColumn> = Vec::new();
        self.columns.retain(|c| {
            if seen.contains(c) {
                return false;
            }
            seen.push(*c);
            true
        });
        if !self.columns.contains(&FormatColumn::Name) {
            self.columns.push(FormatColumn::Name);
        }
        if !self.columns.contains(&self.sort) {
            self.sort = FormatColumn::Name;
        }
        self
    }

    /// Columns that exist but are not shown, in `ALL` order.
    pub fn hidden_columns(&self) -> Vec<FormatColumn> {
        FormatColumn::ALL.iter().copied().filter(|c| !self.columns.contains(c)).collect()
    }
}

/// One saved format bound to a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatEntry {
    /// The directory this entry is for.
    pub path: PathBuf,
    /// Also apply to subdirectories that have no entry of their own.
    #[serde(default)]
    pub inherit: bool,
    /// The saved format.
    pub format: ListerFormat,
}

/// Where the format in effect for a directory came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatSource {
    /// A saved entry (possibly an ancestor's, when it has `inherit`).
    Entry { path: PathBuf, inherit: bool },
    /// The saved default.
    Default,
    /// Nothing saved: the built-in format.
    Builtin,
}

/// Everything saved in `formats.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FormatStore {
    /// Format for directories with no matching entry.
    pub default: Option<ListerFormat>,
    /// Per-directory entries.
    pub entries: Vec<FormatEntry>,
}

impl FormatStore {
    /// The format in effect for `dir`, and where it came from.
    pub fn resolve_with_source(&self, dir: &Path) -> (ListerFormat, FormatSource) {
        if let Some(e) = self.entries.iter().find(|e| e.path == dir) {
            return (e.format.clone(), FormatSource::Entry { path: e.path.clone(), inherit: e.inherit });
        }
        let nearest = self
            .entries
            .iter()
            .filter(|e| e.inherit && dir.starts_with(&e.path))
            .max_by_key(|e| e.path.components().count());
        if let Some(e) = nearest {
            return (e.format.clone(), FormatSource::Entry { path: e.path.clone(), inherit: true });
        }
        match &self.default {
            Some(f) => (f.clone(), FormatSource::Default),
            None => (ListerFormat::builtin(), FormatSource::Builtin),
        }
    }

    /// The format in effect for `dir`.
    pub fn resolve(&self, dir: &Path) -> ListerFormat {
        self.resolve_with_source(dir).0
    }

    /// The saved default, else the built-in format (used where no path
    /// applies, such as inside an archive).
    pub fn fallback(&self) -> ListerFormat {
        self.default.clone().unwrap_or_else(ListerFormat::builtin)
    }

    /// Add the entry for `path`, replacing an existing one.
    pub fn upsert(&mut self, path: PathBuf, inherit: bool, format: ListerFormat) {
        let entry = FormatEntry { path, inherit, format };
        match self.entries.iter_mut().find(|e| e.path == entry.path) {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
    }

    /// Remove the entry for exactly `path`. True when there was one.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.path != path);
        self.entries.len() != before
    }

    /// Set the default format.
    pub fn set_default(&mut self, format: ListerFormat) {
        self.default = Some(format);
    }

    /// Save `format` where the format in effect came from: the matching entry
    /// (keeping its `inherit`), the default, or a new entry for `dir` when the
    /// built-in format was in effect.
    pub fn save_from(&mut self, source: &FormatSource, dir: &Path, format: ListerFormat) {
        match source {
            FormatSource::Entry { path, inherit } => self.upsert(path.clone(), *inherit, format),
            FormatSource::Default => self.set_default(format),
            FormatSource::Builtin => self.upsert(dir.to_path_buf(), false, format),
        }
    }

    /// Repair every format and keep only the last entry for each path.
    pub fn sanitized(self) -> Self {
        let mut entries: Vec<FormatEntry> = Vec::new();
        for e in self.entries {
            let e = FormatEntry { format: e.format.sanitized(), ..e };
            entries.retain(|x| x.path != e.path);
            entries.push(e);
        }
        Self { default: self.default.map(ListerFormat::sanitized), entries }
    }
}

/// Path of `formats.json` under the standard config directory.
pub fn formats_path() -> PathBuf {
    config_dir().join("formats.json")
}

/// Load the store from the standard config directory.
pub fn load_formats() -> FormatStore {
    load_formats_from(&config_dir())
}

/// Testable core of [`load_formats`]. A missing file is an empty store. A file
/// that does not parse is copied to `formats.json.bak` first, because the next
/// save rewrites the file from memory and would destroy what the user wrote.
pub fn load_formats_from(dir: &Path) -> FormatStore {
    let path = dir.join("formats.json");
    let Ok(text) = std::fs::read_to_string(&path) else { return FormatStore::default() };
    match serde_json::from_str::<FormatStore>(&text) {
        Ok(store) => store.sanitized(),
        Err(e) => {
            let backup = dir.join("formats.json.bak");
            crate::err_log!("opusdm", "malformed formats.json ({e}); keeping a copy in {}", backup.display());
            let _ = std::fs::copy(&path, &backup);
            FormatStore::default()
        }
    }
}

/// Save the store under the standard config directory.
pub fn save_formats(store: &FormatStore) -> io::Result<()> {
    save_formats_to(&config_dir(), store)
}

/// Testable core of [`save_formats`].
pub fn save_formats_to(dir: &Path, store: &FormatStore) -> io::Result<()> {
    save_json(&dir.join("formats.json"), store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(hide: &str) -> ListerFormat {
        ListerFormat { hide_glob: hide.into(), ..ListerFormat::builtin() }
    }

    fn store_with(entries: &[(&str, bool, &str)]) -> FormatStore {
        let mut s = FormatStore::default();
        for (p, inherit, hide) in entries {
            s.upsert(PathBuf::from(p), *inherit, fmt(hide));
        }
        s
    }

    #[test]
    fn builtin_shows_every_column_sorted_by_name() {
        let b = ListerFormat::builtin();
        assert_eq!(b.columns, FormatColumn::ALL.to_vec());
        assert_eq!(b.sort, FormatColumn::Name);
        assert_eq!(b.separate, Separate::DirsFirst);
        assert!(!b.reverse && !b.gauge);
    }

    #[test]
    fn exact_entry_wins_over_an_inherited_one() {
        let s = store_with(&[("/a", true, "x"), ("/a/b", false, "y")]);
        assert_eq!(s.resolve(Path::new("/a/b")).hide_glob, "y");
    }

    #[test]
    fn the_deepest_inheriting_ancestor_wins() {
        let s = store_with(&[("/a", true, "x"), ("/a/b", true, "y")]);
        assert_eq!(s.resolve(Path::new("/a/b/c/d")).hide_glob, "y");
        assert_eq!(s.resolve(Path::new("/a/z")).hide_glob, "x");
    }

    #[test]
    fn an_ancestor_without_inherit_does_not_apply_to_children() {
        let s = store_with(&[("/a", false, "x")]);
        assert_eq!(s.resolve(Path::new("/a/b")), ListerFormat::builtin());
    }

    #[test]
    fn a_sibling_with_the_same_text_prefix_does_not_inherit() {
        let s = store_with(&[("/tmp", true, "x")]);
        assert_eq!(s.resolve(Path::new("/tmpx")), ListerFormat::builtin());
        assert_eq!(s.resolve(Path::new("/tmp/sub")).hide_glob, "x");
    }

    #[test]
    fn the_default_applies_when_no_entry_matches() {
        let mut s = store_with(&[("/tmp", false, "x")]);
        s.set_default(fmt("d"));
        assert_eq!(s.resolve(Path::new("/var")).hide_glob, "d");
        assert_eq!(s.resolve(Path::new("/tmp")).hide_glob, "x");
    }

    #[test]
    fn source_reports_where_the_format_came_from() {
        let mut s = store_with(&[("/a", true, "x")]);
        assert_eq!(
            s.resolve_with_source(Path::new("/a/b")).1,
            FormatSource::Entry { path: PathBuf::from("/a"), inherit: true }
        );
        assert_eq!(s.resolve_with_source(Path::new("/z")).1, FormatSource::Builtin);
        s.set_default(fmt("d"));
        assert_eq!(s.resolve_with_source(Path::new("/z")).1, FormatSource::Default);
    }

    #[test]
    fn save_from_updates_the_entry_the_format_came_from() {
        let mut s = store_with(&[("/a", true, "x")]);
        let (_, src) = s.resolve_with_source(Path::new("/a/b"));
        s.save_from(&src, Path::new("/a/b"), fmt("new"));
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].path, PathBuf::from("/a"));
        assert!(s.entries[0].inherit, "the entry keeps its inherit flag");
        assert_eq!(s.entries[0].format.hide_glob, "new");
    }

    #[test]
    fn save_from_builtin_creates_a_this_directory_entry() {
        let mut s = FormatStore::default();
        s.save_from(&FormatSource::Builtin, Path::new("/tmp"), fmt("n"));
        assert_eq!(s.entries.len(), 1);
        assert!(!s.entries[0].inherit);
        s.save_from(&FormatSource::Default, Path::new("/tmp"), fmt("d"));
        assert_eq!(s.default.as_ref().unwrap().hide_glob, "d");
        assert_eq!(s.entries.len(), 1);
    }

    #[test]
    fn upsert_replaces_and_remove_reports() {
        let mut s = store_with(&[("/a", false, "x")]);
        s.upsert(PathBuf::from("/a"), true, fmt("y"));
        assert_eq!(s.entries.len(), 1);
        assert!(s.entries[0].inherit);
        assert!(s.remove(Path::new("/a")));
        assert!(!s.remove(Path::new("/a")));
    }

    #[test]
    fn trailing_slash_and_dot_components_match_the_same_entry() {
        let s = store_with(&[("/tmp", false, "x")]);
        assert_eq!(s.resolve(Path::new("/tmp/")).hide_glob, "x");
    }

    #[test]
    fn sanitize_drops_duplicates_restores_name_and_fixes_sort() {
        let f = ListerFormat {
            columns: vec![FormatColumn::Size, FormatColumn::Size, FormatColumn::Date],
            sort: FormatColumn::User,
            ..ListerFormat::builtin()
        }
        .sanitized();
        assert_eq!(f.columns, vec![FormatColumn::Size, FormatColumn::Date, FormatColumn::Name]);
        assert_eq!(f.sort, FormatColumn::Name, "a sort on a hidden column falls back to Name");
    }

    #[test]
    fn hidden_columns_are_the_ones_not_shown() {
        let f = ListerFormat {
            columns: vec![FormatColumn::Name, FormatColumn::Size],
            ..ListerFormat::builtin()
        };
        assert_eq!(
            f.hidden_columns(),
            vec![FormatColumn::Date, FormatColumn::User, FormatColumn::Group, FormatColumn::Access]
        );
    }

    #[test]
    fn store_sanitize_keeps_the_last_entry_per_path() {
        let s = FormatStore {
            default: None,
            entries: vec![
                FormatEntry { path: "/a".into(), inherit: false, format: fmt("first") },
                FormatEntry { path: "/a".into(), inherit: false, format: fmt("last") },
            ],
        }
        .sanitized();
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].format.hide_glob, "last");
    }

    #[test]
    fn a_partial_format_falls_back_to_defaults_per_field() {
        let f: ListerFormat = serde_json::from_str(r#"{"hide_glob":"*.o"}"#).unwrap();
        assert_eq!(f.hide_glob, "*.o");
        assert_eq!(f.columns, FormatColumn::ALL.to_vec());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let td = tempfile::tempdir().unwrap();
        let s = store_with(&[("/tmp", true, "*.o")]);
        save_formats_to(td.path(), &s).unwrap();
        assert_eq!(load_formats_from(td.path()), s);
    }

    #[test]
    fn a_missing_file_is_an_empty_store() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(load_formats_from(td.path()), FormatStore::default());
    }

    #[test]
    fn an_unparsable_file_is_backed_up_and_loads_empty() {
        let td = tempfile::tempdir().unwrap();
        let bad = r#"{"entries":[{"path":"/a","format":{"columns":["Nmae"]}}]}"#;
        std::fs::write(td.path().join("formats.json"), bad).unwrap();
        assert_eq!(load_formats_from(td.path()), FormatStore::default());
        let kept = std::fs::read_to_string(td.path().join("formats.json.bak")).unwrap();
        assert_eq!(kept, bad);
    }

    #[test]
    fn the_stale_marker_never_equals_a_real_format() {
        let stale = ListerFormat::stale();
        assert_ne!(stale, ListerFormat::builtin());
        assert_ne!(stale.clone().sanitized(), stale, "sanitizing always puts Name back");
    }
}
