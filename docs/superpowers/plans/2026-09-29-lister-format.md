# Lister Format and Show/Hide Patterns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-directory Lister formats (columns, order, sort, directory separation, show/hide patterns, free-space gauge) saved in `formats.json`, with an editor dialog and two palette prompts for the patterns (F-22, F-23).

**Architecture:** A pure data model and resolver in `opusdm-ipc` (`format.rs`, `NameFilter` in `glob.rs`). The hub owns and persists `formats.json` like the bindings (`SetFormats` / `FormatsUpdate`, `Welcome.formats`). Each Lister builds its six columns once, then on every navigation resolves the effective format and applies it (column visibility/order, sort, separation, row filter, gauge).

**Tech Stack:** Rust workspace, GTK4 (`gtk4` crate, `ColumnView`), serde/serde_json, unix domain socket IPC.

**Spec:** `docs/superpowers/specs/2026-09-29-lister-format-design.md`

## Global Constraints

- Code and comments in English. Never use the long dash character; use `-`.
- Constants in UPPERCASE with `_`. Every function and method gets a doc comment; public ones use `///` rustdoc.
- Exit conditions at the top of a function, one-liners where possible (`let Some(x) = y else { return };`).
- Text files end with an empty line.
- Package manager is `pnpm` (not relevant here); build/test with `cargo`.
- Commit after each task with a meaningful message, only the files you changed. Do NOT push. Do NOT add AI attribution lines (no `Co-Authored-By`) to commit messages. Never use `--no-verify`.
- Do not disable tests; fix them.
- Column set is exactly Name, Size, Date, User, Group, Access (Comment waits for F-11).
- Show/hide patterns apply to files only; directories are always listed. `*`/`?` globs, case-insensitive, separators `,` and `;`, `!pattern` excludes.
- Format resolution order: exact entry, nearest ancestor entry with `inherit`, saved default, built-in.
- `Use` is valid only for the directory it was applied in; the next navigation to another directory drops it.
- Header clicks change the order live but are only persisted by Save.
- The hub owns `~/.config/opusdm/formats.json`; Listers never write it.

## Review Focus

1. A hand-edited `formats.json` with duplicate columns, no `Name`, or a `sort` column that is not shown: `sanitized` repairs it (Task 1 tests).
2. A `formats.json` that does not parse (typo in a column name): the file is copied to `formats.json.bak` and the store loads empty instead of destroying the user's text on the next save (Task 1 test).
3. A sibling directory with the same textual prefix (`/tmpx` vs an inherit entry for `/tmp`) must NOT inherit; a non-inherit ancestor entry must not apply to children (Task 1 tests).
4. A show list made only of negations (`!*.o`) lists everything except `*.o`; a hide list of only negations hides nothing; `!` alone is ignored (Task 2 tests).
5. A hide pattern that matches every file leaves only directories, and the status bar totals must follow (Task 5 manual check).

---

## File Structure

| File | Responsibility |
| :--- | :--- |
| `crates/ipc/src/format.rs` (new) | `FormatColumn`, `Separate`, `ListerFormat`, `FormatEntry`, `FormatStore`, `FormatSource`, resolve/upsert/remove, sanitize, load/save `formats.json` |
| `crates/ipc/src/glob.rs` | `NameFilter` (show/hide with negation) |
| `crates/ipc/src/proto.rs` | `Welcome.formats`, `ToHub::SetFormats`, `ToLister::FormatsUpdate` |
| `crates/ipc/src/lib.rs` | module + re-exports |
| `crates/hub/src/server.rs` | own, persist, broadcast the store |
| `crates/lister/src/client.rs`, `main.rs`, `build.rs` | carry `FormatStore` from `Welcome` into `HubConnection.formats` |
| `crates/lister/src/model.rs` | `dir_first_cmp` takes `Separate`; `SortCol: From<FormatColumn>` |
| `crates/lister/src/format_apply.rs` (new) | `impl Ui`: effective format, apply, use, formats-changed |
| `crates/lister/src/gauge.rs` (new) | `usage()` via `statvfs`, `impl Ui { update_gauge }` |
| `crates/lister/src/format_editor.rs` (new) | dialog, pure row helpers, pattern prompts |
| `crates/lister/src/window.rs`, `build.rs`, `keys.rs`, `palette.rs` | state fields, navigation hook, commands |

---

### Task 1: Format data model in `opusdm-ipc`

**Files:**
- Create: `crates/ipc/src/format.rs`
- Modify: `crates/ipc/src/lib.rs`

**Interfaces:**
- Consumes: `crate::settings::{config_dir, save_json}` (existing, both `pub`), `crate::err_log!`.
- Produces (used by every later task):
  - `enum FormatColumn { Name, Size, Date, User, Group, Access }` with `const ALL: [FormatColumn; 6]`, `fn title(self) -> &'static str`
  - `enum Separate { DirsFirst, FilesFirst, Mixed }` (`Default` = `DirsFirst`)
  - `struct ListerFormat { columns: Vec<FormatColumn>, sort: FormatColumn, reverse: bool, separate: Separate, show_glob: String, hide_glob: String, gauge: bool }` with `builtin()`, `sanitized(self)`, `hidden_columns(&self) -> Vec<FormatColumn>`
  - `struct FormatEntry { path: PathBuf, inherit: bool, format: ListerFormat }`
  - `enum FormatSource { Entry { path: PathBuf, inherit: bool }, Default, Builtin }`
  - `struct FormatStore { default: Option<ListerFormat>, entries: Vec<FormatEntry> }` with `resolve(&self, &Path) -> ListerFormat`, `resolve_with_source(&self, &Path) -> (ListerFormat, FormatSource)`, `fallback(&self) -> ListerFormat`, `upsert(&mut self, PathBuf, bool, ListerFormat)`, `remove(&mut self, &Path) -> bool`, `set_default(&mut self, ListerFormat)`, `save_from(&mut self, &FormatSource, &Path, ListerFormat)`, `sanitized(self)`
  - `load_formats() -> FormatStore`, `load_formats_from(&Path)`, `save_formats(&FormatStore) -> io::Result<()>`, `save_formats_to(&Path, &FormatStore)`

- [ ] **Step 1: Write the module with its tests first (tests reference the API above)**

Create `crates/ipc/src/format.rs`:

```rust
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
}
```

- [ ] **Step 2: Register the module**

In `crates/ipc/src/lib.rs` add `pub mod format;` after `pub mod dock;` and, after `pub use dock::DockConfig;`, add:

```rust
pub use format::{FormatColumn, FormatEntry, FormatSource, FormatStore, ListerFormat, Separate};
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p opusdm-ipc format`
Expected: all `format::tests::*` PASS (16 tests). If `a_sibling_with_the_same_text_prefix_does_not_inherit` fails, `starts_with` is being used on strings somewhere - it must be `Path::starts_with` (component based).

- [ ] **Step 4: Commit**

```bash
git add crates/ipc/src/format.rs crates/ipc/src/lib.rs
git commit -m "feat(ipc): lister format model, resolver and formats.json persistence"
```

---

### Task 2: `NameFilter` for show/hide patterns

**Files:**
- Modify: `crates/ipc/src/glob.rs`
- Modify: `crates/ipc/src/lib.rs` (re-export)

**Interfaces:**
- Consumes: `glob_match`, `PATTERN_SEPARATORS` (same file).
- Produces: `NameFilter::new(show: &str, hide: &str) -> NameFilter`, `NameFilter::allows(&self, name: &str) -> bool`, `NameFilter::is_empty(&self) -> bool`. Callers must only apply it to files.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` of `crates/ipc/src/glob.rs`:

```rust
    #[test]
    fn an_empty_filter_allows_everything() {
        let f = NameFilter::new("", "");
        assert!(f.is_empty());
        assert!(f.allows("anything.txt"));
    }

    #[test]
    fn hide_removes_matching_names() {
        let f = NameFilter::new("", "*.o; *.tmp");
        assert!(!f.allows("a.o"));
        assert!(!f.allows("B.TMP"));
        assert!(f.allows("a.rs"));
    }

    #[test]
    fn show_keeps_only_matching_names() {
        let f = NameFilter::new("*.rs,*.toml", "");
        assert!(f.allows("lib.rs"));
        assert!(f.allows("Cargo.toml"));
        assert!(!f.allows("notes.txt"));
    }

    #[test]
    fn hide_wins_over_show() {
        let f = NameFilter::new("*.rs", "test_*");
        assert!(f.allows("lib.rs"));
        assert!(!f.allows("test_lib.rs"));
    }

    #[test]
    fn a_bang_excludes_from_a_list() {
        let f = NameFilter::new("*.rs,!test*", "");
        assert!(f.allows("lib.rs"));
        assert!(!f.allows("test_a.rs"));
        let h = NameFilter::new("", "*.o,!keep.o");
        assert!(!h.allows("a.o"));
        assert!(h.allows("keep.o"));
    }

    #[test]
    fn a_show_list_of_only_negations_shows_everything_else() {
        let f = NameFilter::new("!*.o", "");
        assert!(f.allows("a.rs"));
        assert!(!f.allows("a.o"));
    }

    #[test]
    fn a_hide_list_of_only_negations_hides_nothing() {
        let f = NameFilter::new("", "!*.o");
        assert!(f.allows("a.o"));
        assert!(f.allows("a.rs"));
    }

    #[test]
    fn a_lone_bang_is_ignored() {
        let f = NameFilter::new("!", " ! ");
        assert!(f.is_empty());
        assert!(f.allows("x"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-ipc glob`
Expected: FAIL to compile, "cannot find type `NameFilter`".

- [ ] **Step 3: Implement**

In `crates/ipc/src/glob.rs`, after the `GlobSet` impl (before `expand_new_name`), add:

```rust
/// One comma/semicolon separated list of patterns, where a leading `!` marks
/// an exclusion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PatternList {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl PatternList {
    /// Split `text` into inclusions and `!` exclusions; blank items and a
    /// lone `!` are dropped.
    fn parse(text: &str) -> Self {
        let mut list = Self::default();
        for item in text.split(PATTERN_SEPARATORS).map(str::trim).filter(|p| !p.is_empty()) {
            match item.strip_prefix('!') {
                Some(rest) => {
                    let rest = rest.trim();
                    if !rest.is_empty() {
                        list.exclude.push(rest.to_string());
                    }
                }
                None => list.include.push(item.to_string()),
            }
        }
        list
    }

    /// True when the list holds no pattern at all.
    fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// True when `name` matches an inclusion.
    fn included(&self, name: &str) -> bool {
        self.include.iter().any(|p| glob_match(p, name))
    }

    /// True when `name` matches an exclusion.
    fn excluded(&self, name: &str) -> bool {
        self.exclude.iter().any(|p| glob_match(p, name))
    }
}

/// The persistent Show / Hide filter of a Lister format. Callers apply it to
/// files only; directories are always listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameFilter {
    show: PatternList,
    hide: PatternList,
}

impl NameFilter {
    /// Build a filter from a show list and a hide list (see [`PatternList`]).
    pub fn new(show: &str, hide: &str) -> Self {
        Self { show: PatternList::parse(show), hide: PatternList::parse(hide) }
    }

    /// True when neither list has any pattern.
    pub fn is_empty(&self) -> bool {
        self.show.is_empty() && self.hide.is_empty()
    }

    /// Whether a file called `name` is listed: it passes the show list (no
    /// inclusion means everything passes, minus exclusions) and the hide list
    /// does not hide it.
    pub fn allows(&self, name: &str) -> bool {
        let shown = (self.show.include.is_empty() || self.show.included(name)) && !self.show.excluded(name);
        let hidden = self.hide.included(name) && !self.hide.excluded(name);
        shown && !hidden
    }
}
```

In `crates/ipc/src/lib.rs` add after `pub use dock::DockConfig;`:

```rust
pub use glob::NameFilter;
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p opusdm-ipc glob`
Expected: PASS (existing glob tests plus 8 new).

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/glob.rs crates/ipc/src/lib.rs
git commit -m "feat(ipc): NameFilter with show/hide lists and ! exclusions"
```

---

### Task 3: Protocol, hub ownership and Lister plumbing

**Files:**
- Modify: `crates/ipc/src/proto.rs`
- Modify: `crates/hub/src/server.rs`
- Modify: `crates/lister/src/client.rs`, `crates/lister/src/main.rs`, `crates/lister/src/build.rs`, `crates/lister/src/window.rs`

**Interfaces:**
- Consumes: `FormatStore`, `format::{load_formats, save_formats}` (Task 1).
- Produces:
  - `Welcome.formats: FormatStore` (`#[serde(default)]`)
  - `ToHub::SetFormats(FormatStore)`, `ToLister::FormatsUpdate(FormatStore)`
  - `Client::connect(...) -> io::Result<(Client, ListerId, Roles, Settings, Bindings, FormatStore)>`
  - `HubConnection.formats: RefCell<FormatStore>` in the Lister
  - `ServerOptions.initial_formats: Option<FormatStore>` (tests: `Some(default)`, production: `None` = load from disk)

- [ ] **Step 1: Write the failing hub tests**

In `crates/hub/src/server.rs` tests module, next to `set_bindings_sanitizes_persists_and_broadcasts`, add:

```rust
    #[test]
    fn set_formats_sanitizes_persists_and_broadcasts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.formats, opusdm_ipc::FormatStore::default());

        let mut dirty = opusdm_ipc::FormatStore::default();
        dirty.upsert(
            "/tmp".into(),
            false,
            opusdm_ipc::ListerFormat {
                columns: vec![opusdm_ipc::FormatColumn::Size, opusdm_ipc::FormatColumn::Size],
                ..opusdm_ipc::ListerFormat::builtin()
            },
        );
        send(&mut a, &ToHub::SetFormats(dirty)).unwrap();

        let got: ToLister = recv(&mut a).unwrap().unwrap();
        let ToLister::FormatsUpdate(store) = got else { panic!("expected FormatsUpdate, got {got:?}") };
        assert_eq!(
            store.entries[0].format.columns,
            vec![opusdm_ipc::FormatColumn::Size, opusdm_ipc::FormatColumn::Name],
            "duplicates dropped and Name restored"
        );

        let written = wait_for_file(&cfg.path().join("opusdm").join("formats.json"));
        let back: opusdm_ipc::FormatStore = serde_json::from_str(&written).unwrap();
        assert_eq!(back, store);
    }

    #[test]
    fn welcome_carries_the_formats_loaded_from_disk() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let mut on_disk = opusdm_ipc::FormatStore::default();
        on_disk.upsert(
            "/tmp".into(),
            true,
            opusdm_ipc::ListerFormat { hide_glob: "*.o".into(), ..opusdm_ipc::ListerFormat::builtin() },
        );
        opusdm_ipc::format::save_formats(&on_disk).unwrap();

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server_with(listener, Box::new(|_p| {}), ServerOptions::production());

        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let welcome: Welcome = recv(&mut a).unwrap().unwrap();
        assert_eq!(welcome.formats, on_disk);
    }
```

Also add to the `welcome_roundtrips` test in `proto.rs` the field `formats: crate::format::FormatStore::default(),` (it will not compile otherwise) and add these two lines to the roundtrip lists:

```rust
        roundtrip_tohub(ToHub::SetFormats(crate::format::FormatStore::default()));
```
in the ToHub roundtrip test (next to `SetBindings`), and
```rust
        roundtrip_tolister(ToLister::FormatsUpdate(crate::format::FormatStore::default()));
```
next to the `BindingsUpdate` roundtrip.

- [ ] **Step 2: Add the protocol variants**

In `crates/ipc/src/proto.rs`:

After `pub bindings: crate::bindings::Bindings,` in `Welcome` add:

```rust
    /// The saved Lister formats.
    #[serde(default)]
    pub formats: crate::format::FormatStore,
```

In `ToHub`, after `SetBindings(...)` (line ~182) add:

```rust
    /// Replace the saved Lister formats. The hub sanitizes them, persists
    /// `formats.json` and broadcasts `ToLister::FormatsUpdate`. No reply.
    SetFormats(crate::format::FormatStore),
```

In `ToLister`, after `BindingsUpdate(...)` (line ~412) add:

```rust
    /// The saved Lister formats changed: the full new store.
    FormatsUpdate(crate::format::FormatStore),
```

- [ ] **Step 3: Hub implementation**

In `crates/hub/src/server.rs`:

1. Imports: `use opusdm_ipc::format::{self, FormatStore};`
2. `ServerOptions`: add field
```rust
    /// Formats to start with. `None` loads `formats.json` from disk
    /// (production). The default is an empty store so a test never reads a
    /// developer's real formats.
    pub initial_formats: Option<FormatStore>,
```
   `Default`: `Self { global_keys: false, initial_bindings: Some(Bindings::default()), initial_formats: Some(FormatStore::default()) }`; `production()`: `Self { global_keys: true, initial_bindings: None, initial_formats: None }`. Grep for any other full `ServerOptions { ... }` literal and add the field.
3. `enum PersistMsg`: add `Formats(FormatStore),`; persist thread arm:
```rust
                    PersistMsg::Formats(f) => {
                        if let Err(e) = format::save_formats(&f) {
                            opusdm_ipc::err_log!("hub", "cannot save formats.json: {e}");
                        }
                    }
```
4. Owner struct: next to `current_bindings: Bindings,` add `current_formats: FormatStore,` with doc comment "Loaded once at startup; updated in place by `ToHub::SetFormats`."; initializer next to `current_bindings: match ...`:
```rust
        current_formats: match opts.initial_formats {
            Some(f) => f,
            None => format::load_formats(),
        },
```
5. Message arm after `ToHub::SetBindings`:
```rust
            ToHub::SetFormats(new_formats) => {
                self.current_formats = new_formats.sanitized();
                let _ = self.persist_tx.send(PersistMsg::Formats(self.current_formats.clone()));
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::FormatsUpdate(self.current_formats.clone()).into());
                }
            }
```
6. In the `Welcome { ... }` built on `Register` add `formats: self.current_formats.clone(),`.

Any other `Welcome { .. }` literal in the workspace (grep `Welcome {`; expected in `lister/src/client.rs` tests) needs `formats: opusdm_ipc::FormatStore::default(),`.

- [ ] **Step 4: Lister plumbing**

- `crates/lister/src/client.rs`: `connect` destructures `formats` from `Welcome` and returns it as the 6th tuple element (`io::Result<(Client, ListerId, Roles, Settings, Bindings, FormatStore)>`; import `FormatStore` from `opusdm_ipc`). The two existing tests bind the tuple with `_bindings`; add `, _formats` to both patterns.
- `crates/lister/src/main.rs`: `let (client, lister_id, roles, settings, bindings, formats) = ...` and `build::run(client, lister_id, roles, settings, bindings, formats, start_path)`.
- `crates/lister/src/build.rs`: mirror every occurrence of `bindings` in `run`, `StartState` (line ~26/36), the `build_window` call (line ~61) and `build_window`'s parameters (line ~89/100) with a `formats: opusdm_ipc::FormatStore` twin. At the `HubConnection { ... }` literal (line ~369) add `formats: RefCell::new(formats),`.
- `crates/lister/src/window.rs`: in `HubConnection` add
```rust
    /// Latest saved formats. Updated wholesale from `Welcome` and
    /// `FormatsUpdate`.
    pub formats: RefCell<opusdm_ipc::FormatStore>,
```
  and in `apply_inbound` add an arm (the reload logic arrives in Task 5; for now store only):
```rust
            ToLister::FormatsUpdate(new_formats) => {
                *self.hub.formats.borrow_mut() = new_formats;
            }
```

- [ ] **Step 5: Run everything**

Run: `cargo test --workspace`
Expected: PASS, including the two new hub tests and the updated proto roundtrips. Fix any exhaustive `match` on `ToHub`/`ToLister` the compiler reports.

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/proto.rs crates/hub/src/server.rs crates/lister/src/client.rs crates/lister/src/main.rs crates/lister/src/build.rs crates/lister/src/window.rs
git commit -m "feat(hub): own and broadcast lister formats (SetFormats, FormatsUpdate, Welcome.formats)"
```

---

### Task 4: Comparator honours `Separate`

**Files:**
- Modify: `crates/lister/src/model.rs`
- Modify: `crates/lister/src/columns.rs`, `crates/lister/src/build.rs` (call sites)

**Interfaces:**
- Consumes: `opusdm_ipc::{FormatColumn, Separate}`.
- Produces: `dir_first_cmp(a: &Row, b: &Row, col: SortCol, desc: bool, separate: Separate) -> Ordering`; `impl From<FormatColumn> for SortCol`.

- [ ] **Step 1: Write the failing tests**

In the `tests` module of `crates/lister/src/model.rs` (grep for existing `dir_first_cmp` tests and update their calls to pass `Separate::DirsFirst` as the 5th argument), add:

```rust
    fn row(name: &str, is_dir: bool, size: u64) -> Row {
        Row::new(name.to_string(), is_dir, false, false, size, 0, 0, 0, 0o644)
    }

    fn sorted(names: &mut Vec<Row>, desc: bool, separate: Separate) -> Vec<String> {
        names.sort_by(|a, b| dir_first_cmp(a, b, SortCol::Name, desc, separate));
        names.iter().map(|r| r.name.clone()).collect()
    }

    fn sample() -> Vec<Row> {
        vec![row("b_file", false, 1), row("a_dir", true, 0), row("a_file", false, 1), row("c_dir", true, 0)]
    }

    #[test]
    fn dirs_first_keeps_a_leading_directory_block_in_both_directions() {
        assert_eq!(sorted(&mut sample(), false, Separate::DirsFirst), ["a_dir", "c_dir", "a_file", "b_file"]);
        assert_eq!(sorted(&mut sample(), true, Separate::DirsFirst), ["c_dir", "a_dir", "b_file", "a_file"]);
    }

    #[test]
    fn files_first_puts_directories_last() {
        assert_eq!(sorted(&mut sample(), false, Separate::FilesFirst), ["a_file", "b_file", "a_dir", "c_dir"]);
        assert_eq!(sorted(&mut sample(), true, Separate::FilesFirst), ["b_file", "a_file", "c_dir", "a_dir"]);
    }

    #[test]
    fn mixed_sorts_directories_and_files_together() {
        assert_eq!(sorted(&mut sample(), false, Separate::Mixed), ["a_dir", "a_file", "b_file", "c_dir"]);
        assert_eq!(sorted(&mut sample(), true, Separate::Mixed), ["c_dir", "b_file", "a_file", "a_dir"]);
    }

    #[test]
    fn format_columns_map_to_sort_columns() {
        assert!(SortCol::from(FormatColumn::Date) == SortCol::Modified);
        assert!(SortCol::from(FormatColumn::Access) == SortCol::Access);
        assert!(SortCol::from(FormatColumn::Name) == SortCol::Name);
    }
```

(`Row::new` takes name, is_dir, is_symlink, broken_link, size, modified_unix, uid, gid, mode - keep in sync with the constructor if it changed.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p opusdm-lister model`
Expected: FAIL to compile (wrong argument count / missing `From`).

- [ ] **Step 3: Implement**

In `crates/lister/src/model.rs`, add `use opusdm_ipc::{FormatColumn, Separate};` and replace `dir_first_cmp`:

```rust
/// Compare two rows: directories and files are kept in separate blocks as
/// `separate` says (or sorted together for `Mixed`), then ordered by `col`.
/// `desc` reverses the key within a block, never the block order.
pub fn dir_first_cmp(a: &Row, b: &Row, col: SortCol, desc: bool, separate: Separate) -> Ordering {
    if a.is_dir != b.is_dir {
        match separate {
            Separate::DirsFirst => return if a.is_dir { Ordering::Less } else { Ordering::Greater },
            Separate::FilesFirst => return if a.is_dir { Ordering::Greater } else { Ordering::Less },
            Separate::Mixed => {}
        }
    }
    let base = match col {
        SortCol::Name => a.name_lower.cmp(&b.name_lower),
        SortCol::Size => a.size.cmp(&b.size),
        SortCol::Modified => a.modified_unix.cmp(&b.modified_unix),
        SortCol::User => a.user_lower.cmp(&b.user_lower),
        SortCol::Group => a.group_lower.cmp(&b.group_lower),
        SortCol::Access => a.mode.cmp(&b.mode),
    };
    if desc {
        base.reverse()
    } else {
        base
    }
}

impl From<FormatColumn> for SortCol {
    /// The ordering key of a format column.
    fn from(column: FormatColumn) -> Self {
        match column {
            FormatColumn::Name => SortCol::Name,
            FormatColumn::Size => SortCol::Size,
            FormatColumn::Date => SortCol::Modified,
            FormatColumn::User => SortCol::User,
            FormatColumn::Group => SortCol::Group,
            FormatColumn::Access => SortCol::Access,
        }
    }
}
```

Call sites: `columns.rs` per-column sorter -> `dir_first_cmp(&ra, &rb, sort_col, false, opusdm_ipc::Separate::DirsFirst)` (this sorter only feeds the header arrow; the model sorter in `build.rs` does the real ordering, updated in Task 5). In `build.rs` temporarily pass `Separate::DirsFirst` in the model sorter call so the workspace compiles. Also update any other caller the compiler reports.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p opusdm-lister`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lister/src/model.rs crates/lister/src/columns.rs crates/lister/src/build.rs
git commit -m "feat(lister): sort comparator honours the dir/file separation mode"
```

---

### Task 5: Lister applies the effective format

**Files:**
- Create: `crates/lister/src/format_apply.rs`
- Modify: `crates/lister/src/main.rs` (`mod format_apply;`), `crates/lister/src/window.rs`, `crates/lister/src/build.rs`

**Interfaces:**
- Consumes: Tasks 1, 2, 3, 4.
- Produces (used by Tasks 6 and 7), all on `impl Ui`:
  - `effective_format(&self, dir: &Path) -> ListerFormat`
  - `format_source(&self, dir: &Path) -> FormatSource`
  - `apply_format(&self, format: &ListerFormat)`
  - `use_format(self: &Rc<Self>, format: ListerFormat)`
  - `on_formats_changed(self: &Rc<Self>)`
  - fields `Ui.format: RefCell<ListerFormat>`, `Ui.temp_format: RefCell<Option<(PathBuf, ListerFormat)>>`, `ViewState.col_map: Rc<Vec<(gtk::ColumnViewColumn, FormatColumn)>>`, `ViewState.separate: Rc<Cell<Separate>>`, `ViewState.model_sorter: gtk::CustomSorter`

GTK code has no unit tests here; the pure logic is covered in Tasks 1, 2 and 4. Verification is a build plus the manual checks in Step 6.

- [ ] **Step 1: State fields**

`window.rs`: in `ViewState` add

```rust
    /// Every column with the format column it shows. The columns are built
    /// once; a format only changes visibility and order.
    pub col_map: Rc<Vec<(gtk::ColumnViewColumn, opusdm_ipc::FormatColumn)>>,
    /// Directory / file separation read by the model sorter.
    pub separate: Rc<Cell<opusdm_ipc::Separate>>,
    /// The sorter feeding the list; told when the separation changes.
    pub model_sorter: gtk::CustomSorter,
```

and in `Ui`:

```rust
    /// The format applied to the current directory.
    pub format: RefCell<opusdm_ipc::ListerFormat>,
    /// A format applied with "Use": valid only for the directory it was
    /// applied in.
    pub temp_format: RefCell<Option<(PathBuf, opusdm_ipc::ListerFormat)>>,
```

- [ ] **Step 2: `build.rs` wiring**

1. Change `col_map`'s second tuple element from `SortCol::X` to `opusdm_ipc::FormatColumn::X` (Name, Size, Date, User, Group, Access - same order as today).
2. Before creating `model_sorter` add `let separate = Rc::new(Cell::new(opusdm_ipc::Separate::DirsFirst));`.
3. In the `model_sorter` closure: clone `separate` into it, map the active column with `SortCol::from(*fc)`:
```rust
                    let col = col_map.iter().find(|(c, _)| *c == active).map(|(_, fc)| SortCol::from(*fc))?;
```
   and call `dir_first_cmp(&ra, &rb, col, desc, separate.get())`.
4. In the `ViewState { ... }` literal add `col_map: col_map.clone(), separate: separate.clone(), model_sorter: model_sorter.clone(),` and in the `Ui { ... }` literal `format: RefCell::new(opusdm_ipc::ListerFormat::builtin()), temp_format: RefCell::new(None),`.

- [ ] **Step 3: `format_apply.rs`**

Create `crates/lister/src/format_apply.rs`:

```rust
//! Choosing and applying the Lister format (F-22) for the current directory.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{FormatColumn, FormatSource, ListerFormat};
use std::path::Path;
use std::rc::Rc;

impl Ui {
    /// The format in effect for `dir`: a "Use" format for this very directory,
    /// else the saved one. Archive paths use the default, since no saved path
    /// can name them.
    pub fn effective_format(&self, dir: &Path) -> ListerFormat {
        if let Some((path, format)) = self.temp_format.borrow().as_ref() {
            if path == dir {
                return format.clone();
            }
        }
        let store = self.hub.formats.borrow();
        if crate::archive::parse_archive_path(dir).is_some() {
            return store.fallback();
        }
        store.resolve(dir)
    }

    /// Where the saved format for `dir` comes from (used to save a pattern
    /// change back to the same place).
    pub fn format_source(&self, dir: &Path) -> FormatSource {
        self.hub.formats.borrow().resolve_with_source(dir).1
    }

    /// Show `format`: column visibility and order, sort column and direction,
    /// directory separation, gauge.
    pub fn apply_format(&self, format: &ListerFormat) {
        let view = &self.view;
        let column_for = |wanted: FormatColumn| {
            view.col_map.iter().find(|(_, fc)| *fc == wanted).map(|(col, _)| col.clone())
        };
        // Detach every column, then re-attach the shown ones in order and the
        // hidden ones after them, so display order follows the format.
        for (col, _) in view.col_map.iter() {
            view.column_view.remove_column(col);
        }
        for fc in &format.columns {
            let Some(col) = column_for(*fc) else { continue };
            col.set_visible(true);
            view.column_view.append_column(&col);
        }
        for fc in format.hidden_columns() {
            let Some(col) = column_for(fc) else { continue };
            col.set_visible(false);
            view.column_view.append_column(&col);
        }
        view.separate.set(format.separate);
        let order = if format.reverse { gtk::SortType::Descending } else { gtk::SortType::Ascending };
        view.column_view.sort_by_column(column_for(format.sort).as_ref(), order);
        // The separation can change without the header state changing.
        view.model_sorter.changed(gtk::SorterChange::Different);
        *self.format.borrow_mut() = format.clone();
        self.update_gauge(&self.cwd_path());
    }

    /// Apply `format` to the current directory only ("Use").
    pub fn use_format(self: &Rc<Self>, format: ListerFormat) {
        *self.temp_format.borrow_mut() = Some((self.cwd_path(), format));
        self.reload();
    }

    /// The saved formats changed: reload when that changes the format of the
    /// current directory.
    pub fn on_formats_changed(self: &Rc<Self>) {
        let cwd = self.cwd_path();
        if self.effective_format(&cwd) != *self.format.borrow() {
            self.reload();
        }
    }
}
```

`update_gauge` is added in Task 6; until then add a temporary empty `pub fn update_gauge(&self, _dir: &Path) {}` in `gauge.rs`? No - to keep every task compiling, create `crates/lister/src/gauge.rs` now with only:

```rust
//! Free-space gauge in the status bar (filled in by Task 6).

use crate::window::Ui;
use std::path::Path;

impl Ui {
    /// Refresh the gauge for `dir`.
    pub fn update_gauge(&self, _dir: &Path) {}
}
```

and register `mod format_apply; mod gauge;` in `main.rs` (alphabetical among the existing `mod` lines).

- [ ] **Step 4: Hook navigation and inbound updates**

In `window.rs` `apply_navigation`, right after `*self.cwd.borrow_mut() = dir.to_path_buf();` and before `let show_hidden`, insert:

```rust
        // A "Use" format only lives in the directory it was applied in.
        if self.temp_format.borrow().as_ref().is_some_and(|(p, _)| p != dir) {
            *self.temp_format.borrow_mut() = None;
        }
        let format = self.effective_format(dir);
        if format != *self.format.borrow() {
            self.apply_format(&format);
        }
        let name_filter = opusdm_ipc::NameFilter::new(&format.show_glob, &format.hide_glob);
```

and in the row loop, after the `show_hidden` check:

```rust
            if !row.is_dir && !name_filter.allows(&row.name) {
                continue;
            }
```

Change the `FormatsUpdate` arm to:

```rust
            ToLister::FormatsUpdate(new_formats) => {
                *self.hub.formats.borrow_mut() = new_formats;
                self.on_formats_changed();
            }
```

Note `apply_format` is called with the new `cwd` already set (so `update_gauge(&self.cwd_path())` sees the right directory).

- [ ] **Step 5: Build and run the suite**

Run: `cargo build -p opusdm-lister && cargo test --workspace`
Expected: builds; all tests PASS.

- [ ] **Step 6: Manual check**

Write `~/.config/opusdm/formats.json` by hand (with the hub running the change is only read at hub start, so restart it):

```json
{"entries":[{"path":"/tmp","inherit":false,"format":{"columns":["Name","Size"],"sort":"Size","reverse":true,"separate":"Mixed","hide_glob":"*.o"}}]}
```

Start OpusDM, open `/tmp`: only Name and Size columns, sorted by size descending with the arrow on Size, directories mixed with files, `*.o` files gone, and the top status counts exclude them. Go to `/var`: all six columns back, sorted by Name, directories first. Hide pattern `*` (edit the file to `"hide_glob":"*"` and restart): only directories listed, totals show F: [0/0].

- [ ] **Step 7: Commit**

```bash
git add crates/lister/src/format_apply.rs crates/lister/src/gauge.rs crates/lister/src/main.rs crates/lister/src/window.rs crates/lister/src/build.rs
git commit -m "feat(lister): resolve and apply the saved format on every navigation"
```

---

### Task 6: Free-space gauge

**Files:**
- Modify: `crates/lister/src/gauge.rs` (replace the stub)
- Modify: `crates/lister/src/window.rs` (`StatusBar.gauge`), `crates/lister/src/build.rs` (create the widget)

**Interfaces:**
- Consumes: `Ui.format`, `Ui::update_gauge` call sites from Task 5 (`apply_format`); add one more call at the end of `apply_navigation`.
- Produces: `gauge::usage(dir: &Path) -> io::Result<(u64, u64)>` (total, free), `gauge::used_fraction(total: u64, free: u64) -> f64`, `StatusBar.gauge: gtk::LevelBar`.

The Lister crate has no `opusdm-fsops` dependency but already has `libc`, so the `statvfs` call lives here instead of in `fsops::checkfit` (small deviation from the spec text).

- [ ] **Step 1: Write the failing tests**

Replace `crates/lister/src/gauge.rs` with the tests plus stubs that do not compile yet:

```rust
//! Free-space gauge in the status bar.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use std::io;
use std::path::Path;

/// (total bytes, bytes free to unprivileged users) of the filesystem holding
/// `dir`.
pub fn usage(dir: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    // SAFETY: `statvfs` is plain data, zero is a valid initial value, and the
    // call only writes into the buffer we own.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let frsize = u64::from(st.f_frsize);
    Ok((u64::from(st.f_blocks) * frsize, u64::from(st.f_bavail) * frsize))
}

/// Share of the filesystem in use, `0.0..=1.0`. An empty filesystem reads as
/// 0 so the bar never divides by zero.
pub fn used_fraction(total: u64, free: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    (total.saturating_sub(free) as f64 / total as f64).clamp(0.0, 1.0)
}

/// Human readable size for the tooltip (binary units).
fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

impl Ui {
    /// Show or hide the gauge and, when shown, fill it for `dir`. Hidden when
    /// the format does not ask for it, inside an archive, or when the
    /// filesystem cannot be queried.
    pub fn update_gauge(&self, dir: &Path) {
        let bar = &self.status_bar.gauge;
        let wanted = self.format.borrow().gauge && crate::archive::parse_archive_path(dir).is_none();
        let usage = if wanted { usage(dir).ok() } else { None };
        let Some((total, free)) = usage else {
            bar.set_visible(false);
            return;
        };
        bar.set_value(used_fraction(total, free));
        bar.set_tooltip_text(Some(&format!("{} free of {}", human(free), human(total))));
        bar.set_visible(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_used_share_is_total_minus_free_over_total() {
        assert_eq!(used_fraction(1000, 250), 0.75);
        assert_eq!(used_fraction(1000, 1000), 0.0);
        assert_eq!(used_fraction(1000, 0), 1.0);
    }

    #[test]
    fn an_empty_or_inconsistent_filesystem_never_divides_by_zero_or_overflows() {
        assert_eq!(used_fraction(0, 0), 0.0);
        assert_eq!(used_fraction(100, 500), 0.0, "free above total clamps to empty use");
    }

    #[test]
    fn the_root_filesystem_reports_a_total_at_least_as_big_as_the_free_space() {
        let (total, free) = usage(Path::new("/")).unwrap();
        assert!(total > 0);
        assert!(free <= total);
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        assert!(usage(Path::new("/definitely/not/here")).is_err());
    }

    #[test]
    fn sizes_are_shown_in_binary_units() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }
}
```

- [ ] **Step 2: Add the widget**

`window.rs`, `StatusBar`: add

```rust
    /// Used-space gauge; visible only when the format asks for it.
    pub gauge: gtk::LevelBar,
```

`build.rs`: after `stat_bar.append(&stat_info);` add

```rust
    let gauge = gtk::LevelBar::for_interval(0.0, 1.0);
    gauge.set_size_request(90, -1);
    gauge.set_valign(gtk::Align::Center);
    gauge.set_visible(false);
    stat_bar.append(&gauge);
```

and put `gauge,` into the `StatusBar { ... }` literal (or `gauge: gauge.clone(),` if `gauge` is used afterwards).

At the end of `apply_navigation` (after `self.update_status();`) add `self.update_gauge(dir);` so the value follows reloads even when the format did not change.

- [ ] **Step 3: Run**

Run: `cargo test -p opusdm-lister gauge && cargo build -p opusdm-lister`
Expected: PASS.

- [ ] **Step 4: Manual check**

Set `"gauge":true` in a saved format, restart, open the directory: a bar shows next to the counts with a tooltip like `120.4 GiB free of 466.0 GiB`; another directory without a gauge format hides it.

- [ ] **Step 5: Commit**

```bash
git add crates/lister/src/gauge.rs crates/lister/src/window.rs crates/lister/src/build.rs
git commit -m "feat(lister): free-space gauge in the status bar"
```

---

### Task 7: Editor dialog and pattern commands

**Files:**
- Create: `crates/lister/src/format_editor.rs`
- Modify: `crates/lister/src/main.rs` (`mod format_editor;`), `crates/lister/src/keys.rs` (`CommandId`), `crates/lister/src/palette.rs`

**Interfaces:**
- Consumes: Tasks 1, 5 (`effective_format`, `format_source`, `use_format`, `hub.formats`, `temp_format`), `opusdm_ui::DialogShell`, `opusdm_ui::requester_entry`, `crate::actions::flash`.
- Produces: `format_editor::open(&Rc<Ui>)`, `format_editor::set_pattern(&Rc<Ui>, PatternKind)`, pure helpers `move_column`, `rows_from_format`, `format_from_rows`.

- [ ] **Step 1: Write the failing tests for the pure helpers**

Create `crates/lister/src/format_editor.rs` starting with the helpers and their tests (the GTK part follows in Step 3):

```rust
//! "Lister Format..." dialog (F-22) and the Show / Hide pattern prompts (F-23).

use opusdm_ipc::{FormatColumn, ListerFormat, Separate};

/// Choices of the separation dropdown, in display order.
pub const SEPARATE_CHOICES: [(&str, Separate); 3] = [
    ("Directories first", Separate::DirsFirst),
    ("Files first", Separate::FilesFirst),
    ("Mixed", Separate::Mixed),
];

/// Swap the row at `idx` with its neighbour above (`up`) or below. Returns the
/// new index, or `None` when the row is already at that edge.
pub fn move_column(rows: &mut [(FormatColumn, bool)], idx: usize, up: bool) -> Option<usize> {
    let target = if up { idx.checked_sub(1)? } else { idx + 1 };
    if idx >= rows.len() || target >= rows.len() {
        return None;
    }
    rows.swap(idx, target);
    Some(target)
}

/// The editor's column list for `format`: shown columns in order (checked),
/// then the hidden ones (unchecked).
pub fn rows_from_format(format: &ListerFormat) -> Vec<(FormatColumn, bool)> {
    let mut rows: Vec<(FormatColumn, bool)> = format.columns.iter().map(|c| (*c, true)).collect();
    rows.extend(format.hidden_columns().into_iter().map(|c| (c, false)));
    rows
}

/// Build the format the editor widgets describe.
pub fn format_from_rows(
    rows: &[(FormatColumn, bool)],
    sort: FormatColumn,
    reverse: bool,
    separate: Separate,
    show: &str,
    hide: &str,
    gauge: bool,
) -> ListerFormat {
    ListerFormat {
        columns: rows.iter().filter(|(_, on)| *on).map(|(c, _)| *c).collect(),
        sort,
        reverse,
        separate,
        show_glob: show.trim().to_string(),
        hide_glob: hide.trim().to_string(),
        gauge,
    }
    .sanitized()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_row_swaps_it_with_its_neighbour() {
        let mut rows = rows_from_format(&ListerFormat::builtin());
        assert_eq!(move_column(&mut rows, 1, true), Some(0));
        assert_eq!(rows[0].0, FormatColumn::Size);
        assert_eq!(rows[1].0, FormatColumn::Name);
        assert_eq!(move_column(&mut rows, 0, false), Some(1));
        assert_eq!(rows[0].0, FormatColumn::Name);
    }

    #[test]
    fn moving_past_either_edge_does_nothing() {
        let mut rows = rows_from_format(&ListerFormat::builtin());
        let before = rows.clone();
        assert_eq!(move_column(&mut rows, 0, true), None);
        assert_eq!(move_column(&mut rows, rows.len() - 1, false), None);
        assert_eq!(move_column(&mut rows, 99, true), None);
        assert_eq!(rows, before);
    }

    #[test]
    fn rows_list_shown_columns_first_then_hidden() {
        let f = ListerFormat { columns: vec![FormatColumn::Size, FormatColumn::Name], ..ListerFormat::builtin() };
        let rows = rows_from_format(&f);
        assert_eq!(&rows[..2], &[(FormatColumn::Size, true), (FormatColumn::Name, true)]);
        assert!(rows[2..].iter().all(|(_, on)| !on));
        assert_eq!(rows.len(), FormatColumn::ALL.len());
    }

    #[test]
    fn rows_roundtrip_through_format_from_rows() {
        let f = ListerFormat {
            columns: vec![FormatColumn::Date, FormatColumn::Name],
            sort: FormatColumn::Date,
            reverse: true,
            separate: Separate::Mixed,
            show_glob: "*.rs".into(),
            hide_glob: "*.o".into(),
            gauge: true,
        };
        let back = format_from_rows(&rows_from_format(&f), f.sort, f.reverse, f.separate, "*.rs", "*.o", true);
        assert_eq!(back, f);
    }

    #[test]
    fn unchecking_name_still_keeps_name_and_a_hidden_sort_falls_back() {
        let mut rows = rows_from_format(&ListerFormat::builtin());
        for r in rows.iter_mut() {
            r.1 = r.0 == FormatColumn::Size;
        }
        let f = format_from_rows(&rows, FormatColumn::Date, false, Separate::DirsFirst, "", "", false);
        assert_eq!(f.columns, vec![FormatColumn::Size, FormatColumn::Name]);
        assert_eq!(f.sort, FormatColumn::Name);
    }

    #[test]
    fn patterns_are_trimmed() {
        let f = format_from_rows(&rows_from_format(&ListerFormat::builtin()), FormatColumn::Name, false, Separate::DirsFirst, "  *.rs ", " ", false);
        assert_eq!(f.show_glob, "*.rs");
        assert_eq!(f.hide_glob, "");
    }
}
```

Register the module (`mod format_editor;` in `main.rs`) and run `cargo test -p opusdm-lister format_editor` -> Expected PASS (helpers are already implemented above; if you prefer strict red-green, comment the bodies out first).

- [ ] **Step 2: Commands**

`keys.rs`, `CommandId`: add

```rust
    /// Open the Lister Format editor.
    ListerFormat,
    /// Ask for the persistent Show pattern list.
    SetShowPattern,
    /// Ask for the persistent Hide pattern list.
    SetHidePattern,
```

`palette.rs`, in `builtin_commands()` next to `ToggleHidden`:

```rust
        CommandItem { id: CommandId::ListerFormat, title: Cow::Borrowed("Lister Format..."), category: Cow::Borrowed("View"), shortcut: None },
        CommandItem { id: CommandId::SetShowPattern, title: Cow::Borrowed("Set Show Pattern..."), category: Cow::Borrowed("View"), shortcut: None },
        CommandItem { id: CommandId::SetHidePattern, title: Cow::Borrowed("Set Hide Pattern..."), category: Cow::Borrowed("View"), shortcut: None },
```

and in `execute_command`:

```rust
        CommandId::ListerFormat => crate::format_editor::open(ui),
        CommandId::SetShowPattern => crate::format_editor::set_pattern(ui, crate::format_editor::PatternKind::Show),
        CommandId::SetHidePattern => crate::format_editor::set_pattern(ui, crate::format_editor::PatternKind::Hide),
```

- [ ] **Step 3: The GTK part**

Append to `format_editor.rs` (above the `#[cfg(test)]` module), adding the imports at the top of the file: `use crate::window::Ui; use gtk4 as gtk; use gtk::prelude::*; use opusdm_ipc::ToHub; use std::cell::RefCell; use std::rc::Rc;`

```rust
/// Which pattern list a prompt edits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    Show,
    Hide,
}

/// Rebuild the column list widget from `rows`.
fn rebuild(list: &gtk::Box, rows: &Rc<RefCell<Vec<(FormatColumn, bool)>>>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let snapshot = rows.borrow().clone();
    for (idx, (column, on)) in snapshot.iter().enumerate() {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let check = gtk::CheckButton::with_label(column.title());
        check.set_active(*on);
        check.set_hexpand(true);
        // Name is what the list is about; it can move but not disappear.
        check.set_sensitive(*column != FormatColumn::Name);
        {
            let rows = rows.clone();
            let column = *column;
            check.connect_toggled(move |c| {
                if let Some(row) = rows.borrow_mut().iter_mut().find(|r| r.0 == column) {
                    row.1 = c.is_active();
                }
            });
        }
        line.append(&check);
        for (label, up) in [("Up", true), ("Down", false)] {
            let button = gtk::Button::with_label(label);
            let (list, rows) = (list.clone(), rows.clone());
            button.connect_clicked(move |_| {
                let moved = move_column(&mut rows.borrow_mut(), idx, up);
                if moved.is_some() {
                    rebuild(&list, &rows);
                }
            });
            line.append(&button);
        }
        list.append(&line);
    }
}

/// Send the store to the hub. The `FormatsUpdate` that comes back applies it.
fn send_store(ui: &Rc<Ui>, store: opusdm_ipc::FormatStore) {
    *ui.temp_format.borrow_mut() = None;
    if let Err(e) = ui.hub.client.send(&ToHub::SetFormats(store)) {
        crate::actions::flash(ui, &format!("cannot save format: {e}"));
    }
}

/// Open the "Lister Format..." dialog for the current directory.
pub fn open(ui: &Rc<Ui>) {
    let dir = ui.cwd_path();
    if crate::archive::parse_archive_path(&dir).is_some() {
        crate::actions::flash(ui, "formats do not apply inside an archive");
        return;
    }
    let initial = ui.effective_format(&dir);
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Lister Format").size(480, 600).scrolled();

    let rows = Rc::new(RefCell::new(rows_from_format(&initial)));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    shell.content().append(&gtk::Label::new(Some("Columns (top to bottom = left to right)")));
    shell.content().append(&list);
    rebuild(&list, &rows);

    let titles: Vec<&str> = FormatColumn::ALL.iter().map(|c| c.title()).collect();
    let sort_dd = gtk::DropDown::from_strings(&titles);
    sort_dd.set_selected(FormatColumn::ALL.iter().position(|c| *c == initial.sort).unwrap_or(0) as u32);
    shell.content().append(&gtk::Label::new(Some("Sort by")));
    shell.content().append(&sort_dd);
    let reverse = gtk::CheckButton::with_label("Reverse (descending)");
    reverse.set_active(initial.reverse);
    shell.content().append(&reverse);

    let separate_labels: Vec<&str> = SEPARATE_CHOICES.iter().map(|(l, _)| *l).collect();
    let separate_dd = gtk::DropDown::from_strings(&separate_labels);
    separate_dd.set_selected(SEPARATE_CHOICES.iter().position(|(_, s)| *s == initial.separate).unwrap_or(0) as u32);
    shell.content().append(&gtk::Label::new(Some("Separation")));
    shell.content().append(&separate_dd);

    let show = gtk::Entry::new();
    show.set_text(&initial.show_glob);
    show.set_placeholder_text(Some("*.rs, *.toml, !test*"));
    let hide = gtk::Entry::new();
    hide.set_text(&initial.hide_glob);
    hide.set_placeholder_text(Some("*.o, *.tmp, !keep.o"));
    shell.content().append(&gtk::Label::new(Some("Show only (files; empty = all)")));
    shell.content().append(&show);
    shell.content().append(&gtk::Label::new(Some("Hide (files)")));
    shell.content().append(&hide);
    let gauge = gtk::CheckButton::with_label("Free space gauge");
    gauge.set_active(initial.gauge);
    shell.content().append(&gauge);

    // Read the widgets back into a format.
    let collect: Rc<dyn Fn() -> ListerFormat> = {
        let (rows, sort_dd, reverse, separate_dd, show, hide, gauge) =
            (rows.clone(), sort_dd.clone(), reverse.clone(), separate_dd.clone(), show.clone(), hide.clone(), gauge.clone());
        Rc::new(move || {
            let sort = FormatColumn::ALL.get(sort_dd.selected() as usize).copied().unwrap_or(FormatColumn::Name);
            let separate = SEPARATE_CHOICES.get(separate_dd.selected() as usize).map(|(_, s)| *s).unwrap_or_default();
            format_from_rows(&rows.borrow(), sort, reverse.is_active(), separate, show.text().as_str(), hide.text().as_str(), gauge.is_active())
        })
    };

    let window = shell.window().clone();
    let use_btn = shell.button("Use");
    let save_dir = shell.button("Save (this directory)");
    let save_tree = shell.button("Save (with subdirectories)");
    let save_default = shell.button("Save as default");
    let remove = shell.button("Remove saved format");
    let reset = shell.button("Reset");
    shell.close_button("Cancel");

    {
        let (ui, collect, window) = (ui.clone(), collect.clone(), window.clone());
        use_btn.connect_clicked(move |_| {
            ui.use_format(collect());
            window.close();
        });
    }
    for (button, inherit) in [(save_dir, Some(false)), (save_tree, Some(true)), (save_default, None)] {
        let (ui, collect, window, dir) = (ui.clone(), collect.clone(), window.clone(), dir.clone());
        button.connect_clicked(move |_| {
            let mut store = ui.hub.formats.borrow().clone();
            match inherit {
                Some(inherit) => store.upsert(dir.clone(), inherit, collect()),
                None => store.set_default(collect()),
            }
            send_store(&ui, store);
            window.close();
        });
    }
    {
        let (ui, window, dir) = (ui.clone(), window.clone(), dir.clone());
        remove.connect_clicked(move |_| {
            let mut store = ui.hub.formats.borrow().clone();
            if !store.remove(&dir) {
                crate::actions::flash(&ui, "this directory has no saved format of its own");
                return;
            }
            send_store(&ui, store);
            window.close();
        });
    }
    {
        let (rows, list, sort_dd, reverse, separate_dd, show, hide, gauge) =
            (rows.clone(), list.clone(), sort_dd.clone(), reverse.clone(), separate_dd.clone(), show.clone(), hide.clone(), gauge.clone());
        reset.connect_clicked(move |_| {
            *rows.borrow_mut() = rows_from_format(&ListerFormat::builtin());
            rebuild(&list, &rows);
            sort_dd.set_selected(0);
            reverse.set_active(false);
            separate_dd.set_selected(0);
            show.set_text("");
            hide.set_text("");
            gauge.set_active(false);
        });
    }
    shell.present();
}

/// Ask for a Show or Hide pattern list and save it where the format in effect
/// came from (F-23). An empty answer clears the list.
pub fn set_pattern(ui: &Rc<Ui>, kind: PatternKind) {
    let dir = ui.cwd_path();
    if crate::archive::parse_archive_path(&dir).is_some() {
        crate::actions::flash(ui, "patterns do not apply inside an archive");
        return;
    }
    let format = ui.effective_format(&dir);
    let (title, initial) = match kind {
        PatternKind::Show => ("Show only files matching (empty = all)", format.show_glob.clone()),
        PatternKind::Hide => ("Hide files matching (empty = none)", format.hide_glob.clone()),
    };
    let ui = ui.clone();
    opusdm_ui::requester_entry(&ui.window.clone(), title, &initial, &[("OK", true), ("Cancel", false)], move |accepted, text| {
        if !accepted {
            return;
        }
        let mut format = ui.effective_format(&dir);
        match kind {
            PatternKind::Show => format.show_glob = text.trim().to_string(),
            PatternKind::Hide => format.hide_glob = text.trim().to_string(),
        }
        let source = ui.format_source(&dir);
        let mut store = ui.hub.formats.borrow().clone();
        store.save_from(&source, &dir, format);
        send_store(&ui, store);
    });
}
```

If `DialogShell::scrolled` / `present` signatures differ from the calls above (see `crates/ui/src/shell.rs`), follow the shell as it is: `present(self)` consumes the shell, so every `shell.button(..)` and `shell.window().clone()` must happen before it (they do above).

- [ ] **Step 4: Build and test**

Run: `cargo test --workspace && cargo clippy -p opusdm-lister 2>&1 | grep -E "format_editor|format_apply|gauge"`
Expected: tests PASS, no clippy warnings in the new files. The keys test `every_command_id_the_map_emits_exists_in_the_palette` still passes.

- [ ] **Step 5: Manual check**

Palette (`Ctrl+P`) -> "Lister Format...": untick User and Group, Save (this directory) in `/tmp`; the columns disappear at once in every Lister showing `/tmp`; `/var` still shows six. "Set Hide Pattern..." with `*.o` in a directory without a saved format creates its entry; reload (`Ctrl+R`) and re-enter the directory: still hidden. Set it to empty: files return. "Save (with subdirectories)" on `/tmp`, open `/tmp/x`: same columns. "Remove saved format" on `/tmp/x` says it has none of its own. "Use" changes only the current directory and reverts after navigating away and back.

- [ ] **Step 6: Commit**

```bash
git add crates/lister/src/format_editor.rs crates/lister/src/main.rs crates/lister/src/keys.rs crates/lister/src/palette.rs
git commit -m "feat(lister): Lister Format editor and Show/Hide pattern commands"
```

---

### Task 8: Documentation, tracking and final verification

**Files:**
- Modify: `docs/usage.md`, `CHANGES.md`, `TODO.md`

- [ ] **Step 1: usage.md**

After the "3.10b Status bar format" section add:

```markdown
### 3.10c Lister formats and Show/Hide patterns
Open **Lister Format...** from the command palette (`Ctrl+P`).
- **Columns**: tick the ones to show, move them with **Up** / **Down** (Name can move but not be removed). **Sort by** and **Reverse** choose the order, **Separation** puts directories first, last or mixed with the files.
- **Show only** / **Hide** are pattern lists for *files* (directories are always listed): `*` and `?`, separated by `,` or `;`, and `!pattern` excludes (`*.rs,!test*`). **Hide** wins over **Show**.
- **Free space gauge** adds a used-space bar to the top status bar.
- **Use** applies the format to the current directory until you leave it. **Save (this directory)**, **Save (with subdirectories)** and **Save as default** store it in `~/.config/opusdm/formats.json`; every Lister picks the change up at once. Entering a directory applies its own format, else the nearest parent saved with subdirectories, else the default, else the built-in one. **Remove saved format** deletes this directory's entry, **Reset** refills the dialog with the built-in format.
- **Set Show Pattern...** and **Set Hide Pattern...** (palette) edit just the pattern and save it where the current format came from (a new entry for this directory when nothing was saved). An empty answer clears it.
- Clicking a column header still sorts on the spot, but only **Save** keeps it.
```

- [ ] **Step 2: CHANGES.md**

Append to the `## 2026-09-29` section:

```markdown
- Feature (F-22, F-23): Lister formats. New `opusdm_ipc::format` (`ListerFormat`, `FormatStore`, resolve by exact entry / inheriting ancestor / default / built-in, `sanitized`, `formats.json` load with `.bak` on parse errors) and `glob::NameFilter` (show/hide lists with `!` exclusions, files only). The hub owns and persists `formats.json` (`ToHub::SetFormats`, `ToLister::FormatsUpdate`, `Welcome.formats`). The Lister builds its six columns once and applies the effective format on every navigation (`format_apply.rs`): column visibility and order, sort column and direction, dir/file separation (`dir_first_cmp` takes `Separate`), row filter, free-space gauge (`gauge.rs`, `statvfs`). "Lister Format..." dialog and "Set Show/Hide Pattern..." in the palette (`format_editor.rs`).
```

- [ ] **Step 3: TODO.md**

Tick `F-22` and `F-23` (`- [x]`); keep item 4 of "Suggested order" unticked (F-24 is still open), and append to its line nothing else.

- [ ] **Step 4: Full verification**

Run: `cargo test --workspace 2>&1 | grep -E "test result|FAILED"` and `cargo clippy --workspace 2>&1 | grep -E "format|gauge" | head`
Expected: all results `ok`, no clippy findings in the new files. Then run the acceptance checks of the spec: save a format for `/tmp` without User/Group and confirm `/tmp` applies it while `/var` keeps the default; hide `*.o` and confirm it survives a reload and leaving/re-entering the directory.

- [ ] **Step 5: Commit**

```bash
git add docs/usage.md CHANGES.md TODO.md
git commit -m "docs: lister formats and show/hide patterns (F-22, F-23)"
```
