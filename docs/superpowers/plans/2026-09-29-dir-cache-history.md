# Directory Cache, Path History and CacheList Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Instant revisits through a per-Lister LRU cache of directory listings (always revalidated from disk), browser-style back/forward with a history menu, and a hub-wide "Cache List" dialog with jump and free (F-24).

**Architecture:** Two pure modules in the Lister (`DirCache`, `History`) plus glue in a new `nav_cache.rs`. Each Lister pushes its cached paths to the hub (`CacheReport`); the hub answers `QueryCaches` with a snapshot, forwards `JumpTo` as `GoTo`, and broadcasts `FreeCaches`. The cache holds raw rows (before the Show/Hide filter) so formats never invalidate it.

**Tech Stack:** Rust workspace, GTK4, serde, unix-socket IPC.

**Spec:** `docs/superpowers/specs/2026-09-29-dir-cache-history-design.md`

## Global Constraints

- Code and comments in English. Never use the long dash character; use `-`.
- Constants in UPPERCASE with `_`. Every function and method gets a doc comment (`///` for public ones).
- Exit conditions at the top of a function, one-liners where possible.
- Text files end with an empty line.
- Commit after each task with a meaningful message, only the files you changed. Do NOT push. Do NOT add AI attribution lines. Never use `--no-verify`. Do not disable tests.
- Cache default size 20, range 0..=200 (0 turns it off); history limit 50 entries.
- The cache is never used for archive paths; a reload (`Ctrl+R`, auto-refresh, job refresh) always reads the disk.
- A cached listing is shown at once and the disk is ALWAYS re-read; the fresh result replaces the view only when it differs.
- Alt+Left / Alt+Right are back / forward, read in the window key handler before `keys::resolve`.
- Listers report cache paths to the hub with `CacheReport`; the hub never holds rows.

## Review Focus

1. A cached directory that was deleted or renamed: cached rows show, then the disk read fails, the error is shown and the entry leaves the cache (Task 5 manual check).
2. Cache capacity 0 stores nothing; shrinking the capacity evicts the oldest entries (Task 1 tests).
3. A disk listing with the same rows in another order counts as unchanged, so the view is not re-applied and the selection is not disturbed (Task 1 test).
4. `JumpTo` an unknown or unregistered Lister is ignored; a disconnected Lister's cached paths disappear from the CacheList (Task 4 tests).
5. History: pushing the current path again is a no-op, a new push drops the forward part, the 50 entry limit drops the oldest (Task 2 tests).

---

## File Structure

| File | Responsibility |
| :--- | :--- |
| `crates/lister/src/dircache.rs` (new) | `DirCache` LRU, `same_listing` |
| `crates/lister/src/history.rs` (new) | `History` stack |
| `crates/ipc/src/proto.rs`, `lib.rs` | `Behavior.cache_size`, `CacheEntry`, new `ToHub` / `ToLister` variants |
| `crates/hub/src/server.rs` | per-connection cache paths, query / free / jump |
| `crates/theme-config/src/ui.rs` | "Directory cache size" spin button |
| `crates/lister/src/nav_cache.rs` (new) | `impl Ui`: `navigate`, `finish_listing`, `reload`, cache report, history navigation and widgets |
| `crates/lister/src/cache_list.rs` (new) | the Cache List dialog |
| `crates/lister/src/window.rs`, `build.rs`, `keys.rs`, `palette.rs`, `main.rs` | state, wiring, commands |

---

### Task 1: `DirCache` and `same_listing`

**Files:**
- Create: `crates/lister/src/dircache.rs`
- Modify: `crates/lister/src/main.rs` (`mod dircache;`)

**Interfaces:**
- Consumes: `crate::model::Row`.
- Produces: `DirCache::{new(usize), get(&mut self, &Path) -> Option<Rc<Vec<Row>>>, put(&mut self, PathBuf, Vec<Row>) -> Rc<Vec<Row>>, remove(&mut self, &Path) -> bool, clear(&mut self), set_capacity(&mut self, usize), paths(&self) -> Vec<PathBuf>}`; `same_listing(a: &[Row], b: &[Row]) -> bool`.

- [ ] **Step 1: Write the module with its tests**

Create `crates/lister/src/dircache.rs`:

```rust
//! LRU cache of directory listings, so revisiting a directory shows its rows
//! at once. Holds the raw rows from disk, before any Show / Hide filtering.

use crate::model::Row;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Recently read directory listings, most recent first.
pub struct DirCache {
    capacity: usize,
    entries: Vec<(PathBuf, Rc<Vec<Row>>)>,
}

impl DirCache {
    /// An empty cache keeping at most `capacity` directories (0 keeps none).
    pub fn new(capacity: usize) -> Self {
        Self { capacity, entries: Vec::new() }
    }

    /// The cached rows of `dir`, marking it the most recently used.
    pub fn get(&mut self, dir: &Path) -> Option<Rc<Vec<Row>>> {
        let pos = self.entries.iter().position(|(p, _)| p == dir)?;
        let entry = self.entries.remove(pos);
        let rows = entry.1.clone();
        self.entries.insert(0, entry);
        Some(rows)
    }

    /// Store the rows of `dir` as the most recent entry, evicting the oldest
    /// entries beyond the capacity. With capacity 0 nothing is stored.
    pub fn put(&mut self, dir: PathBuf, rows: Vec<Row>) -> Rc<Vec<Row>> {
        let rows = Rc::new(rows);
        self.entries.retain(|(p, _)| *p != dir);
        self.entries.insert(0, (dir, rows.clone()));
        self.entries.truncate(self.capacity);
        rows
    }

    /// Drop the entry for `dir`. True when there was one.
    pub fn remove(&mut self, dir: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(p, _)| p != dir);
        self.entries.len() != before
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Change the capacity, evicting the oldest entries that no longer fit.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.entries.truncate(capacity);
    }

    /// The cached directories, most recently used first.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|(p, _)| p.clone()).collect()
    }
}

/// The fields of a row that make a listing look different on screen.
type RowKey<'a> = (&'a str, bool, bool, bool, u64, i64, u32, u32, u32);

/// Comparable summary of a row.
fn row_key(r: &Row) -> RowKey<'_> {
    (&r.name, r.is_dir, r.is_symlink, r.broken_link, r.size, r.modified_unix, r.uid, r.gid, r.mode)
}

/// True when `a` and `b` hold the same entries with the same attributes,
/// whatever order the filesystem returned them in.
pub fn same_listing(a: &[Row], b: &[Row]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut ka: Vec<RowKey> = a.iter().map(row_key).collect();
    let mut kb: Vec<RowKey> = b.iter().map(row_key).collect();
    ka.sort();
    kb.sort();
    ka == kb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, size: u64) -> Row {
        Row::new(name.to_string(), false, false, false, size, 100, 0, 0, 0o644)
    }

    fn dir(n: &str) -> PathBuf {
        PathBuf::from(format!("/d/{n}"))
    }

    #[test]
    fn a_put_directory_can_be_fetched() {
        let mut c = DirCache::new(3);
        c.put(dir("a"), vec![row("x", 1)]);
        assert_eq!(c.get(&dir("a")).unwrap().len(), 1);
        assert!(c.get(&dir("b")).is_none());
    }

    #[test]
    fn the_oldest_entry_is_evicted_past_the_capacity() {
        let mut c = DirCache::new(2);
        c.put(dir("a"), vec![]);
        c.put(dir("b"), vec![]);
        c.put(dir("c"), vec![]);
        assert_eq!(c.paths(), vec![dir("c"), dir("b")]);
        assert!(c.get(&dir("a")).is_none());
    }

    #[test]
    fn get_refreshes_recency() {
        let mut c = DirCache::new(2);
        c.put(dir("a"), vec![]);
        c.put(dir("b"), vec![]);
        c.get(&dir("a"));
        c.put(dir("c"), vec![]);
        assert_eq!(c.paths(), vec![dir("c"), dir("a")], "b was the least recently used");
    }

    #[test]
    fn putting_an_existing_directory_replaces_it_without_growing() {
        let mut c = DirCache::new(3);
        c.put(dir("a"), vec![row("old", 1)]);
        c.put(dir("a"), vec![row("new", 2), row("new2", 3)]);
        assert_eq!(c.paths().len(), 1);
        assert_eq!(c.get(&dir("a")).unwrap().len(), 2);
    }

    #[test]
    fn capacity_zero_stores_nothing() {
        let mut c = DirCache::new(0);
        let rows = c.put(dir("a"), vec![row("x", 1)]);
        assert_eq!(rows.len(), 1, "the caller still gets its rows back");
        assert!(c.paths().is_empty());
        assert!(c.get(&dir("a")).is_none());
    }

    #[test]
    fn shrinking_the_capacity_evicts_the_oldest() {
        let mut c = DirCache::new(3);
        for n in ["a", "b", "c"] {
            c.put(dir(n), vec![]);
        }
        c.set_capacity(1);
        assert_eq!(c.paths(), vec![dir("c")]);
        c.set_capacity(0);
        assert!(c.paths().is_empty());
    }

    #[test]
    fn remove_and_clear_report_and_empty() {
        let mut c = DirCache::new(3);
        c.put(dir("a"), vec![]);
        assert!(c.remove(&dir("a")));
        assert!(!c.remove(&dir("a")));
        c.put(dir("b"), vec![]);
        c.clear();
        assert!(c.paths().is_empty());
    }

    #[test]
    fn listings_in_a_different_order_are_the_same() {
        let a = vec![row("x", 1), row("y", 2)];
        let b = vec![row("y", 2), row("x", 1)];
        assert!(same_listing(&a, &b));
    }

    #[test]
    fn a_changed_size_added_or_removed_entry_is_a_different_listing() {
        let base = vec![row("x", 1), row("y", 2)];
        assert!(!same_listing(&base, &[row("x", 1), row("y", 3)]));
        assert!(!same_listing(&base, &[row("x", 1)]));
        assert!(!same_listing(&base, &[row("x", 1), row("z", 2)]));
        assert!(same_listing(&[], &[]));
    }
}
```

- [ ] **Step 2: Register and run**

Add `mod dircache;` to `crates/lister/src/main.rs` (alphabetical, after `mod copy_as;`... place it where `mod columns;`/`mod context_menu;` sit).

Run: `cargo test -p opusdm-lister dircache`
Expected: 9 tests PASS. To prove the tests bite, temporarily change `truncate(self.capacity)` in `put` to `truncate(self.capacity + 1)`: `the_oldest_entry_is_evicted_past_the_capacity` and `capacity_zero_stores_nothing` must FAIL; revert.

- [ ] **Step 3: Commit**

```bash
git add crates/lister/src/dircache.rs crates/lister/src/main.rs
git commit -m "feat(lister): DirCache LRU and same_listing"
```

---

### Task 2: `History` stack

**Files:**
- Create: `crates/lister/src/history.rs`
- Modify: `crates/lister/src/main.rs` (`mod history;`)

**Interfaces:**
- Produces: `History::{new(), push(PathBuf), back(&mut self) -> Option<PathBuf>, forward(&mut self) -> Option<PathBuf>, jump(&mut self, usize) -> Option<PathBuf>, entries(&self) -> &[PathBuf], index(&self) -> Option<usize>, can_back(&self) -> bool, can_forward(&self) -> bool}`, `const HISTORY_LIMIT: usize = 50`.

- [ ] **Step 1: Write the module with its tests**

Create `crates/lister/src/history.rs`:

```rust
//! Browser style navigation history of one Lister.

use std::path::PathBuf;

/// Most entries kept; the oldest falls off.
pub const HISTORY_LIMIT: usize = 50;

/// The directories a Lister visited, with a position in them.
#[derive(Default)]
pub struct History {
    entries: Vec<PathBuf>,
    index: usize,
}

impl History {
    /// An empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a navigation to `dir`. Pushing the current entry again does
    /// nothing; otherwise everything after the current entry is dropped and
    /// `dir` becomes the new current entry.
    pub fn push(&mut self, dir: PathBuf) {
        if self.entries.get(self.index) == Some(&dir) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(dir);
        if self.entries.len() > HISTORY_LIMIT {
            self.entries.remove(0);
        }
        self.index = self.entries.len() - 1;
    }

    /// Step back and return the directory to go to.
    pub fn back(&mut self) -> Option<PathBuf> {
        if !self.can_back() {
            return None;
        }
        self.index -= 1;
        Some(self.entries[self.index].clone())
    }

    /// Step forward and return the directory to go to.
    pub fn forward(&mut self) -> Option<PathBuf> {
        if !self.can_forward() {
            return None;
        }
        self.index += 1;
        Some(self.entries[self.index].clone())
    }

    /// Move to entry `index` and return its directory.
    pub fn jump(&mut self, index: usize) -> Option<PathBuf> {
        let dir = self.entries.get(index)?.clone();
        self.index = index;
        Some(dir)
    }

    /// Every entry, oldest first.
    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    /// Position of the current entry, `None` when the history is empty.
    pub fn index(&self) -> Option<usize> {
        if self.entries.is_empty() { None } else { Some(self.index) }
    }

    /// True when there is an older entry.
    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    /// True when there is a newer entry.
    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn visited(names: &[&str]) -> History {
        let mut h = History::new();
        for n in names {
            h.push(p(n));
        }
        h
    }

    #[test]
    fn an_empty_history_goes_nowhere() {
        let mut h = History::new();
        assert_eq!(h.index(), None);
        assert!(!h.can_back() && !h.can_forward());
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), None);
        assert_eq!(h.jump(0), None);
    }

    #[test]
    fn back_and_forward_walk_the_stack() {
        let mut h = visited(&["/a", "/b", "/c"]);
        assert_eq!(h.index(), Some(2));
        assert_eq!(h.back(), Some(p("/b")));
        assert_eq!(h.back(), Some(p("/a")));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some(p("/b")));
        assert!(h.can_back() && h.can_forward());
    }

    #[test]
    fn pushing_the_current_directory_again_does_nothing() {
        let mut h = visited(&["/a", "/b"]);
        h.push(p("/b"));
        assert_eq!(h.entries().len(), 2);
    }

    #[test]
    fn navigating_back_then_to_the_same_entry_keeps_the_forward_part() {
        let mut h = visited(&["/a", "/b", "/c"]);
        h.back();
        h.push(p("/b"));
        assert!(h.can_forward(), "going back and landing on /b must not erase /c");
        assert_eq!(h.entries().len(), 3);
    }

    #[test]
    fn a_new_directory_drops_the_forward_part() {
        let mut h = visited(&["/a", "/b", "/c"]);
        h.back();
        h.push(p("/x"));
        assert_eq!(h.entries(), &[p("/a"), p("/b"), p("/x")]);
        assert!(!h.can_forward());
    }

    #[test]
    fn jump_moves_to_an_entry() {
        let mut h = visited(&["/a", "/b", "/c"]);
        assert_eq!(h.jump(0), Some(p("/a")));
        assert_eq!(h.index(), Some(0));
        assert_eq!(h.jump(9), None);
        assert_eq!(h.index(), Some(0));
    }

    #[test]
    fn the_oldest_entry_falls_off_at_the_limit() {
        let mut h = History::new();
        for i in 0..(HISTORY_LIMIT + 5) {
            h.push(p(&format!("/d{i}")));
        }
        assert_eq!(h.entries().len(), HISTORY_LIMIT);
        assert_eq!(h.entries()[0], p("/d5"));
        assert_eq!(h.index(), Some(HISTORY_LIMIT - 1));
    }
}
```

- [ ] **Step 2: Register and run**

Add `mod history;` to `main.rs` (after `mod hotkey_editor;` is not required; keep alphabetical next to `mod glyphs;`).

Run: `cargo test -p opusdm-lister history`
Expected: 7 tests PASS. Mutation check: change `self.entries.truncate(self.index + 1);` to a no-op; `a_new_directory_drops_the_forward_part` must FAIL; revert.

- [ ] **Step 3: Commit**

```bash
git add crates/lister/src/history.rs crates/lister/src/main.rs
git commit -m "feat(lister): browser style navigation history"
```

---

### Task 3: `Behavior.cache_size` and the Settings control

**Files:**
- Modify: `crates/ipc/src/proto.rs`, `crates/ipc/src/lib.rs`, `crates/theme-config/src/ui.rs`

**Interfaces:**
- Produces: `Behavior.cache_size: usize` (default `DEFAULT_CACHE_SIZE` = 20), `Behavior::cache_capacity(&self) -> usize` (clamped), `CACHE_SIZE_RANGE: (usize, usize) = (0, 200)`.

- [ ] **Step 1: Write the failing tests**

In the `tests` module of `crates/ipc/src/proto.rs` add:

```rust
    #[test]
    fn the_directory_cache_holds_twenty_listings_by_default() {
        assert_eq!(Behavior::default().cache_size, 20);
        assert_eq!(Behavior::default().cache_capacity(), 20);
    }

    #[test]
    fn a_hand_edited_cache_size_is_clamped() {
        let at = |n| Behavior { cache_size: n, ..Behavior::default() }.cache_capacity();
        assert_eq!(at(0), 0, "0 turns the cache off");
        assert_eq!(at(50), 50);
        assert_eq!(at(1_000_000), CACHE_SIZE_RANGE.1);
    }

    #[test]
    fn a_behavior_block_without_cache_size_gets_the_default() {
        let b: Behavior = serde_json::from_str(r#"{"auto_refresh":false}"#).unwrap();
        assert_eq!(b.cache_size, 20);
    }
```

and add `cache_size: 7,` to the full `Behavior { ... }` literal in the settings roundtrip test (next to `status_format:`).

Run: `cargo test -p opusdm-ipc behavior`
Expected: FAIL to compile (`cache_size` unknown).

- [ ] **Step 2: Implement**

In `Behavior` (after `status_format`):

```rust
    /// How many directory listings each Lister keeps for instant revisits.
    /// 0 turns the cache off. Clamped by [`CACHE_SIZE_RANGE`] when used.
    pub cache_size: usize,
```

Below `DEFAULT_STATUS_FORMAT`:

```rust
/// Directory listings kept per Lister until the user says otherwise.
pub const DEFAULT_CACHE_SIZE: usize = 20;

/// Accepted range for [`Behavior::cache_size`].
pub const CACHE_SIZE_RANGE: (usize, usize) = (0, 200);
```

In `Default for Behavior` add `cache_size: DEFAULT_CACHE_SIZE,`; in `impl Behavior` add:

```rust
    /// The cache size to actually use, clamped into [`CACHE_SIZE_RANGE`] so a
    /// hand-edited settings.json cannot ask for an absurd amount of memory.
    pub fn cache_capacity(&self) -> usize {
        let (lo, hi) = CACHE_SIZE_RANGE;
        self.cache_size.clamp(lo, hi)
    }
```

In `lib.rs` extend the `pub use proto::{ ... }` list with `CACHE_SIZE_RANGE, DEFAULT_CACHE_SIZE,`.

In `crates/theme-config/src/ui.rs` `BehaviorSection`: add field `cache_size: gtk::SpinButton,`; after the `format_row` block in `new`:

```rust
        let cache_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        cache_row.append(&gtk::Label::new(Some("Directory cache size (0 = off)")));
        let (lo, hi) = opusdm_ipc::CACHE_SIZE_RANGE;
        let cache_size = gtk::SpinButton::with_range(lo as f64, hi as f64, 1.0);
        cache_size.set_value(initial.cache_size as f64);
        cache_row.append(&cache_size);
        root.append(&cache_row);
```

add `cache_size` to the `Self { ... }` literal, and in `read()` add `cache_size: self.cache_size.value_as_int().max(0) as usize,`.

- [ ] **Step 3: Run**

Run: `cargo test --workspace 2>&1 | grep -E "test result|FAILED|^error"`
Expected: all PASS (fix any other `Behavior { ... }` literal the compiler flags).

- [ ] **Step 4: Commit**

```bash
git add crates/ipc/src/proto.rs crates/ipc/src/lib.rs crates/theme-config/src/ui.rs
git commit -m "feat(settings): directory cache size"
```

---

### Task 4: Protocol and hub: CacheReport, QueryCaches, FreeAllCaches, JumpTo

**Files:**
- Modify: `crates/ipc/src/proto.rs`, `crates/ipc/src/lib.rs`, `crates/hub/src/server.rs`

**Interfaces:**
- Produces:
  - `struct CacheEntry { lister: ListerId, path: PathBuf }` (`pub`, serde, re-exported from `opusdm_ipc`)
  - `ToHub::CacheReport(Vec<PathBuf>)`, `ToHub::QueryCaches`, `ToHub::FreeAllCaches`, `ToHub::JumpTo { lister: ListerId, path: PathBuf }`
  - `ToLister::CacheList(Vec<CacheEntry>)`, `ToLister::FreeCaches`, `ToLister::GoTo { path: PathBuf }`

- [ ] **Step 1: Write the failing hub tests**

In the `tests` module of `crates/hub/src/server.rs`, near the formats tests, add:

```rust
    /// Read messages from `s` until `pick` returns a value, panicking after 30
    /// messages (role broadcasts may interleave with the reply we want).
    fn recv_until<T>(s: &mut UnixStream, pick: impl Fn(ToLister) -> Option<T>) -> T {
        for _ in 0..30 {
            let msg: ToLister = recv(s).unwrap().unwrap();
            if let Some(v) = pick(msg) {
                return v;
            }
        }
        panic!("the expected message never arrived");
    }

    /// Register a Lister and return its stream and id.
    fn register(path: &std::path::Path, pid: u32, start: &str) -> (UnixStream, ListerId) {
        let mut s = connect(path);
        send(&mut s, &ToHub::Register { pid, initial_path: start.into() }).unwrap();
        let welcome: Welcome = recv(&mut s).unwrap().unwrap();
        (s, welcome.lister_id)
    }

    #[test]
    fn query_caches_lists_every_listers_reported_paths() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, id_a) = register(&path, 1, "/a");
        let (mut b, id_b) = register(&path, 2, "/b");

        send(&mut a, &ToHub::CacheReport(vec!["/x".into()])).unwrap();
        send(&mut b, &ToHub::CacheReport(vec!["/y".into(), "/z".into()])).unwrap();
        send(&mut a, &ToHub::QueryCaches).unwrap();

        let entries = recv_until(&mut a, |m| match m {
            ToLister::CacheList(e) => Some(e),
            _ => None,
        });
        let got: Vec<(ListerId, String)> =
            entries.iter().map(|e| (e.lister, e.path.to_string_lossy().into_owned())).collect();
        assert_eq!(got, vec![(id_a, "/x".into()), (id_b, "/y".into()), (id_b, "/z".into())]);
    }

    #[test]
    fn a_disconnected_listers_cached_paths_disappear() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, _id_a) = register(&path, 1, "/a");
        let (mut b, _id_b) = register(&path, 2, "/b");
        send(&mut b, &ToHub::CacheReport(vec!["/y".into()])).unwrap();
        // Make sure the hub processed b's report before b goes away.
        send(&mut a, &ToHub::QueryCaches).unwrap();
        let first = recv_until(&mut a, |m| match m {
            ToLister::CacheList(e) => Some(e),
            _ => None,
        });
        assert_eq!(first.len(), 1);
        drop(b);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            send(&mut a, &ToHub::QueryCaches).unwrap();
            let now = recv_until(&mut a, |m| match m {
                ToLister::CacheList(e) => Some(e),
                _ => None,
            });
            if now.is_empty() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "b's paths never disappeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn free_all_caches_reaches_every_lister() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, _) = register(&path, 1, "/a");
        let (mut b, _) = register(&path, 2, "/b");
        send(&mut a, &ToHub::FreeAllCaches).unwrap();
        recv_until(&mut a, |m| matches!(m, ToLister::FreeCaches).then_some(()));
        recv_until(&mut b, |m| matches!(m, ToLister::FreeCaches).then_some(()));
    }

    #[test]
    fn jump_to_reaches_only_the_target_and_ignores_unknown_ids() {
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let (mut a, _id_a) = register(&path, 1, "/a");
        let (mut b, id_b) = register(&path, 2, "/b");

        send(&mut a, &ToHub::JumpTo { lister: ListerId(9999), path: "/nowhere".into() }).unwrap();
        send(&mut a, &ToHub::JumpTo { lister: id_b, path: "/target".into() }).unwrap();
        let goto = recv_until(&mut b, |m| match m {
            ToLister::GoTo { path } => Some(path),
            _ => None,
        });
        assert_eq!(goto, std::path::PathBuf::from("/target"));

        // a must not have been sent a GoTo: the next reply it reads is the
        // CacheList, with no GoTo before it.
        send(&mut a, &ToHub::QueryCaches).unwrap();
        recv_until(&mut a, |m| match m {
            ToLister::GoTo { .. } => panic!("a GoTo reached the wrong Lister"),
            ToLister::CacheList(_) => Some(()),
            _ => None,
        });
    }
```

Add roundtrip lines to the `proto.rs` tests: `roundtrip_tohub(ToHub::CacheReport(vec!["/a".into()]));`, `roundtrip_tohub(ToHub::QueryCaches);`, `roundtrip_tohub(ToHub::FreeAllCaches);`, `roundtrip_tohub(ToHub::JumpTo { lister: ListerId(2), path: "/p".into() });` and `roundtrip_tolister(ToLister::CacheList(vec![CacheEntry { lister: ListerId(1), path: "/p".into() }]));`, `roundtrip_tolister(ToLister::FreeCaches);`, `roundtrip_tolister(ToLister::GoTo { path: "/p".into() });`.

Run: `cargo test -p opusdm-ipc -p opusdm-hub 2>&1 | grep -E "^error" | head -3`
Expected: compile errors (unknown variants).

- [ ] **Step 2: Implement the protocol**

In `proto.rs`, near `ListerId`, add:

```rust
/// One directory a Lister keeps in its cache, as listed by the Cache List.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The Lister that holds it.
    pub lister: ListerId,
    /// The cached directory.
    pub path: PathBuf,
}
```

In `ToHub` (after `SetFormats`):

```rust
    /// The directories this Lister has cached, most recent first. Replaces the
    /// previous report. No reply.
    CacheReport(Vec<PathBuf>),
    /// Ask for every Lister's cached paths; answered with `ToLister::CacheList`.
    QueryCaches,
    /// Ask every Lister to drop its directory cache (`ToLister::FreeCaches`).
    FreeAllCaches,
    /// Ask `lister` to raise its window and go to `path` (`ToLister::GoTo`).
    /// Ignored when there is no such Lister.
    JumpTo { lister: ListerId, path: PathBuf },
```

In `ToLister` (after `FormatsUpdate`):

```rust
    /// Snapshot of every Lister's cached directories, in Lister id order.
    CacheList(Vec<CacheEntry>),
    /// Drop the directory cache.
    FreeCaches,
    /// Raise the window and navigate to `path`.
    GoTo { path: PathBuf },
```

Re-export `CacheEntry` in `lib.rs` (`proto::{ ..., CacheEntry, ... }`).

- [ ] **Step 3: Implement the hub**

In `Conn` add `cache_paths: Vec<PathBuf>,` (doc: "The directories this Lister reported as cached.") and set `cache_paths: Vec::new()` where `Conn { ... path: PathBuf::new() }` is built. Import `CacheEntry` from `opusdm_ipc`. Add arms next to `ToHub::SetFormats`:

```rust
            ToHub::CacheReport(paths) => {
                if let Some(c) = self.conns.get_mut(&cid) {
                    c.cache_paths = paths;
                }
            }

            ToHub::QueryCaches => {
                let mut ids: Vec<u64> = self.conns.iter().filter(|(_, c)| c.registered).map(|(id, _)| *id).collect();
                ids.sort_unstable();
                let entries: Vec<CacheEntry> = ids
                    .iter()
                    .flat_map(|id| {
                        self.conns[id].cache_paths.iter().map(move |p| CacheEntry { lister: ListerId(*id), path: p.clone() })
                    })
                    .collect();
                if let Some(c) = self.conns.get(&cid) {
                    let _ = c.tx.send(ToLister::CacheList(entries).into());
                }
            }

            ToHub::FreeAllCaches => {
                for c in self.conns.values_mut().filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::FreeCaches.into());
                }
            }

            ToHub::JumpTo { lister, path } => {
                if let Some(c) = self.conns.get(&lister.0).filter(|c| c.registered) {
                    let _ = c.tx.send(ToLister::GoTo { path }.into());
                }
            }
```

- [ ] **Step 4: Run**

Run: `cargo test -p opusdm-ipc -p opusdm-hub 2>&1 | grep -E "test result|FAILED|^error"`
Expected: all PASS (4 new hub tests, 7 new roundtrip lines). The lister crate's `apply_inbound` will now fail to compile on the new `ToLister` variants: add temporary arms `ToLister::CacheList(_) | ToLister::FreeCaches | ToLister::GoTo { .. } => {}` there so the workspace builds (Tasks 5 and 7 replace them), then `cargo test --workspace`.

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/proto.rs crates/ipc/src/lib.rs crates/hub/src/server.rs crates/lister/src/window.rs
git commit -m "feat(hub): cache reports, cache list, free and jump-to messages"
```

---

### Task 5: The Lister uses the cache

**Files:**
- Create: `crates/lister/src/nav_cache.rs`
- Modify: `crates/lister/src/window.rs`, `crates/lister/src/build.rs`, `crates/lister/src/main.rs`

**Interfaces:**
- Consumes: `DirCache`, `same_listing` (Task 1), `History` (Task 2), `Behavior::cache_capacity` (Task 3), `ToHub::CacheReport`, `ToLister::{FreeCaches, GoTo}` (Task 4).
- Produces on `impl Ui`: `navigate(&Rc<Self>, &Path, use_cache: bool, on_done: impl Fn() + 'static)`, `select_names(&self, &[String])`, `report_cache(&self)`, `free_caches(&self)`; fields `Ui.dir_cache: RefCell<DirCache>`, `Ui.history: RefCell<History>`. `apply_navigation` becomes `pub(crate)`.

GTK glue: the pure parts are tested in Tasks 1 and 2; verification here is the build plus the manual check.

- [ ] **Step 1: State**

`window.rs`, in `Ui` add:

```rust
    /// Recently read directory listings (raw rows, before the Show / Hide filter).
    pub dir_cache: RefCell<crate::dircache::DirCache>,
    /// Back / forward history.
    pub history: RefCell<crate::history::History>,
```

`build.rs`: before the `Ui { ... }` literal add `let cache_capacity = settings.behavior.cache_capacity();` (must come before `settings` is moved into `HubConnection`), and in the literal `dir_cache: RefCell::new(crate::dircache::DirCache::new(cache_capacity)), history: RefCell::new(crate::history::History::new()),`.

- [ ] **Step 2: `nav_cache.rs`**

Create `crates/lister/src/nav_cache.rs`:

```rust
//! Navigation with the directory cache: show the cached listing at once, then
//! re-read the disk and update the view only if it changed.

use crate::model::{read_dir_rows, Row};
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use opusdm_ipc::ToHub;
use std::io;
use std::path::Path;
use std::rc::Rc;

impl Ui {
    /// Go to `dir`. With `use_cache` a cached listing is shown immediately;
    /// the disk is read either way, on a thread, and the result replaces the
    /// view only when it differs from what is shown. `on_done` runs once the
    /// final listing is on screen.
    pub fn navigate(self: &Rc<Self>, dir: &Path, use_cache: bool, on_done: impl Fn() + 'static) {
        crate::dlog!("navigate {} (cache: {use_cache})", dir.display());
        let gen = self.nav_generation.get() + 1;
        self.nav_generation.set(gen);

        let is_archive = crate::archive::parse_archive_path(dir).is_some();
        let cached = if use_cache && !is_archive { self.dir_cache.borrow_mut().get(dir) } else { None };
        if let Some(rows) = &cached {
            self.apply_navigation(dir, Ok((**rows).clone()));
        }

        let dir_owned = dir.to_path_buf();
        let (tx, rx) = async_channel::bounded::<io::Result<Vec<Row>>>(1);
        std::thread::spawn(move || {
            let result = if let Some((arch, inner)) = crate::archive::parse_archive_path(&dir_owned) {
                crate::archive::read_archive_rows(&arch, &inner)
            } else {
                read_dir_rows(&dir_owned)
            };
            let _ = tx.send_blocking(result);
        });

        let this = self.clone();
        let dir_owned = dir.to_path_buf();
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            // Superseded by a later navigation while this was loading.
            if this.nav_generation.get() != gen {
                return;
            }
            this.finish_listing(&dir_owned, result, cached);
            on_done();
        });
    }

    /// Handle the disk read that follows a navigation. `cached` is the
    /// listing already shown for it, if any.
    fn finish_listing(self: &Rc<Self>, dir: &Path, result: io::Result<Vec<Row>>, cached: Option<Rc<Vec<Row>>>) {
        let is_archive = crate::archive::parse_archive_path(dir).is_some();
        let rows = match result {
            Ok(rows) => rows,
            Err(e) => {
                // The directory is gone or unreadable: forget the stale copy.
                if cached.is_some() && self.dir_cache.borrow_mut().remove(dir) {
                    self.report_cache();
                }
                self.apply_navigation(dir, Err(e));
                return;
            }
        };
        if cached.as_ref().is_some_and(|c| crate::dircache::same_listing(c, &rows)) {
            return;
        }
        // Keep the user's selection when the fresh listing replaces a cached one.
        let selected = if cached.is_some() { self.selected_names() } else { Vec::new() };
        if !is_archive {
            self.dir_cache.borrow_mut().put(dir.to_path_buf(), rows.clone());
            self.report_cache();
        }
        self.apply_navigation(dir, Ok(rows));
        self.select_names(&selected);
    }

    /// Select the rows called `names`, adding to the current selection.
    pub fn select_names(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        for i in 0..self.view.selection.n_items() {
            if self.row_at(i).is_some_and(|row| names.contains(&row.name)) {
                self.view.selection.select_item(i, false);
            }
        }
    }

    /// Tell the hub which directories this Lister has cached.
    pub fn report_cache(&self) {
        let paths = self.dir_cache.borrow().paths();
        let _ = self.hub.client.send(&ToHub::CacheReport(paths));
    }

    /// Drop the whole cache and tell the hub.
    pub fn free_caches(&self) {
        self.dir_cache.borrow_mut().clear();
        self.report_cache();
    }
}
```

- [ ] **Step 3: Rewire `window.rs`**

1. Replace the bodies of `navigate_to` / `navigate_to_then` and delete the old thread-and-future code:

```rust
    /// Go to `dir` (see [`Ui::navigate`]).
    pub fn navigate_to(self: &Rc<Self>, dir: &Path) {
        self.navigate(dir, true, || {});
    }

    /// `navigate_to`, running `on_done` once the final listing is applied
    /// (skipped if a later navigation supersedes this one first).
    pub fn navigate_to_then(self: &Rc<Self>, dir: &Path, on_done: impl Fn() + 'static) {
        self.navigate(dir, true, on_done);
    }
```

2. Make `apply_navigation` `pub(crate)`.
3. Replace `reload` with:

```rust
    /// Re-read the current directory from disk (never from the cache),
    /// keeping the selection by name once the new listing lands.
    pub fn reload(self: &Rc<Self>) {
        let selected = self.selected_names();
        let cwd = self.cwd.borrow().clone();
        let this = self.clone();
        self.navigate(&cwd, false, move || this.select_names(&selected));
    }
```

4. In `apply_navigation`, right after `*self.cwd.borrow_mut() = dir.to_path_buf();` add `self.history.borrow_mut().push(dir.to_path_buf());`.
5. In `apply_inbound`: replace the temporary arm from Task 4 with
```rust
            ToLister::FreeCaches => self.free_caches(),
            ToLister::GoTo { path } => {
                self.window.present();
                self.navigate_to(&path);
            }
            ToLister::CacheList(_) => {}
```
   (`CacheList` is handled in Task 7). In the `InvalidatePath` arm insert first:
```rust
                if self.dir_cache.borrow_mut().remove(&p) {
                    self.report_cache();
                }
```
   and in the `SettingsUpdate` arm, after the new settings are stored:
```rust
                let capacity = self.hub.settings.borrow().behavior.cache_capacity();
                self.dir_cache.borrow_mut().set_capacity(capacity);
                self.report_cache();
```
6. Register `mod nav_cache;` in `main.rs`. If `async_channel`/`read_dir_rows` imports in `window.rs` become unused, remove them.

- [ ] **Step 4: Build and run**

Run: `cargo build -p opusdm-lister 2>&1 | grep -E "^(error|warning)" -A8 | head -30 ; cargo test --workspace 2>&1 | grep -E "test result|FAILED|^error"`
Expected: builds without warnings, all tests PASS.

- [ ] **Step 5: Manual check (needs a display; do it on a scratch config)**

Start OpusDM with `XDG_CONFIG_HOME=$(mktemp -d)`. Open a directory with thousands of files, go to its parent and back by typing the path again: the list appears at once. `touch` a new file in it from a shell while in the parent, re-enter: the cached list shows, then the new file appears without losing the selection. Delete the directory from a shell while away, re-enter from history: the error is shown. `Ctrl+R` always reflects the disk.

- [ ] **Step 6: Commit**

```bash
git add crates/lister/src/nav_cache.rs crates/lister/src/window.rs crates/lister/src/build.rs crates/lister/src/main.rs
git commit -m "feat(lister): directory cache with background revalidation"
```

---

### Task 6: History buttons, menu, shortcuts and commands

**Files:**
- Modify: `crates/lister/src/nav_cache.rs`, `crates/lister/src/window.rs`, `crates/lister/src/build.rs`, `crates/lister/src/keys.rs`, `crates/lister/src/palette.rs`

**Interfaces:**
- Consumes: `Ui.history` (Task 5).
- Produces: `struct NavHistory { back: gtk::Button, forward: gtk::Button, list: gtk::ListBox }` (a `Ui` field `nav_history`), `Ui::{history_back, history_forward, history_jump(usize), refresh_history_ui}`, `install_history(&Rc<Ui>)`, `CommandId::{HistoryBack, HistoryForward}`.

- [ ] **Step 1: Widgets and state**

`window.rs`:

```rust
/// The back / forward buttons and the history drop-down of the path bar.
pub struct NavHistory {
    /// Go back one entry.
    pub back: gtk::Button,
    /// Go forward one entry.
    pub forward: gtk::Button,
    /// The rows of the history pop-over, newest first.
    pub list: gtk::ListBox,
}
```

and a `Ui` field `pub nav_history: NavHistory,`.

`build.rs`, in the nav bar section (before `nav_bar.append(&home_btn);`):

```rust
    let back_btn = gtk::Button::with_label("\u{25C0}");
    back_btn.set_tooltip_text(Some("Back (Alt+Left)"));
    back_btn.add_css_class("opus-btn");
    let forward_btn = gtk::Button::with_label("\u{25B6}");
    forward_btn.set_tooltip_text(Some("Forward (Alt+Right)"));
    forward_btn.add_css_class("opus-btn");
    let history_list = gtk::ListBox::new();
    history_list.set_selection_mode(gtk::SelectionMode::None);
    let history_scroll = gtk::ScrolledWindow::builder().child(&history_list).min_content_width(360).max_content_height(320).propagate_natural_height(true).build();
    let history_popover = gtk::Popover::new();
    history_popover.set_child(Some(&history_scroll));
    let history_btn = gtk::MenuButton::new();
    history_btn.set_label("\u{25BE}");
    history_btn.set_tooltip_text(Some("History"));
    history_btn.set_popover(Some(&history_popover));
    history_btn.add_css_class("opus-btn");
    nav_bar.append(&back_btn);
    nav_bar.append(&forward_btn);
    nav_bar.append(&history_btn);
```

and in the `Ui { ... }` literal `nav_history: NavHistory { back: back_btn.clone(), forward: forward_btn.clone(), list: history_list.clone() },`. After the Ui exists (next to `crate::user_menu::install(&ui);`) add `crate::nav_cache::install_history(&ui, &history_popover);`.

- [ ] **Step 2: Logic in `nav_cache.rs`**

Append inside `impl Ui`:

```rust
    /// Go to the previous history entry.
    pub fn history_back(self: &Rc<Self>) {
        let target = self.history.borrow_mut().back();
        if let Some(dir) = target {
            self.navigate_to(&dir);
        }
    }

    /// Go to the next history entry.
    pub fn history_forward(self: &Rc<Self>) {
        let target = self.history.borrow_mut().forward();
        if let Some(dir) = target {
            self.navigate_to(&dir);
        }
    }

    /// Go to history entry `index`.
    pub fn history_jump(self: &Rc<Self>, index: usize) {
        let target = self.history.borrow_mut().jump(index);
        if let Some(dir) = target {
            self.navigate_to(&dir);
        }
    }

    /// Enable / disable the buttons and rebuild the menu rows (newest first,
    /// the current entry in bold).
    pub fn refresh_history_ui(&self) {
        use gtk::prelude::*;
        let history = self.history.borrow();
        self.nav_history.back.set_sensitive(history.can_back());
        self.nav_history.forward.set_sensitive(history.can_forward());
        let list = &self.nav_history.list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        let current = history.index();
        for (i, path) in history.entries().iter().enumerate().rev() {
            let label = gtk::Label::new(None);
            label.set_xalign(0.0);
            let text = glib::markup_escape_text(&path.to_string_lossy());
            if Some(i) == current {
                label.set_markup(&format!("<b>{text}</b>"));
            } else {
                label.set_markup(&text);
            }
            list.append(&label);
        }
    }
```

and, as a free function at module level:

```rust
/// Wire the back / forward buttons and the history menu.
pub fn install_history(ui: &Rc<Ui>, popover: &gtk::Popover) {
    use gtk::prelude::*;
    {
        let ui = ui.clone();
        ui.clone().nav_history.back.connect_clicked(move |_| ui.history_back());
    }
    {
        let ui = ui.clone();
        ui.clone().nav_history.forward.connect_clicked(move |_| ui.history_forward());
    }
    {
        let ui_for_click = ui.clone();
        let popover = popover.clone();
        ui.nav_history.list.connect_row_activated(move |_, row| {
            let len = ui_for_click.history.borrow().entries().len();
            // Rows are listed newest first.
            let Some(index) = len.checked_sub(1 + row.index() as usize) else { return };
            popover.popdown();
            ui_for_click.history_jump(index);
        });
    }
    ui.refresh_history_ui();
}
```

In `window.rs` `apply_navigation`, right after the `history.push(...)` line from Task 5 add `self.refresh_history_ui();`.

- [ ] **Step 3: Commands and the Alt keys**

`keys.rs` `CommandId`: add `/// Go back in the history.\n HistoryBack,` and `/// Go forward in the history.\n HistoryForward,`. `palette.rs`: two entries in `builtin_commands()` (category "Navigation", shortcuts `Some("Alt+Left")` / `Some("Alt+Right")`, titles "History Back" / "History Forward") and in `execute_command`: `CommandId::HistoryBack => ui.history_back(), CommandId::HistoryForward => ui.history_forward(),`.

`build.rs` key handler, right after the `typeahead_feed` block and before the user hotkey lookup:

```rust
            // Alt+Left / Alt+Right walk the history (resolve() has no Alt).
            if focus == crate::keys::Focus::List && state.contains(gdk::ModifierType::ALT_MASK) {
                match keyval {
                    gdk::Key::Left => {
                        ui_for_keys.history_back();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::Right => {
                        ui_for_keys.history_forward();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }
```

- [ ] **Step 4: Build, test, manual check**

Run: `cargo test --workspace 2>&1 | grep -E "test result|FAILED|^error"; cargo clippy -p opusdm-lister 2>&1 | grep -E "nav_cache|dircache|history" | head`
Expected: PASS, no clippy findings in the new files. Manual: visit three directories, back / forward buttons enable and disable correctly, `Alt+Left` / `Alt+Right` work from the list, the menu lists newest first with the current one in bold and clicking a row jumps; a new navigation after going back drops the forward entries.

- [ ] **Step 5: Commit**

```bash
git add crates/lister/src
git commit -m "feat(lister): back/forward buttons, history menu and Alt+Left/Right"
```

---

### Task 7: Cache List dialog and commands

**Files:**
- Create: `crates/lister/src/cache_list.rs`
- Modify: `crates/lister/src/main.rs` (`mod cache_list;`), `crates/lister/src/window.rs`, `crates/lister/src/keys.rs`, `crates/lister/src/palette.rs`

**Interfaces:**
- Consumes: `ToHub::{QueryCaches, FreeAllCaches, JumpTo}`, `ToLister::CacheList`, `CacheEntry`.
- Produces: `cache_list::open(&Rc<Ui>)` (sends the query), `cache_list::show(&Rc<Ui>, Vec<CacheEntry>)` (called when the reply arrives), `CommandId::{CacheList, FreeCaches}`.

- [ ] **Step 1: Write the pure helper test first**

The row text is the one pure piece. Create `crates/lister/src/cache_list.rs` starting with:

```rust
//! The "Cache List..." dialog: every Lister's cached directories.

use opusdm_ipc::CacheEntry;

/// The text of one dialog row.
pub fn entry_label(entry: &CacheEntry) -> String {
    format!("Lister {}: {}", entry.lister.0, entry.path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::ListerId;

    #[test]
    fn a_row_names_the_lister_and_the_path() {
        let e = CacheEntry { lister: ListerId(3), path: "/home/x".into() };
        assert_eq!(entry_label(&e), "Lister 3: /home/x");
    }
}
```

Add `mod cache_list;` to `main.rs` and run `cargo test -p opusdm-lister cache_list` -> PASS (to see it fail first, write the test before `entry_label`, run, then add the function).

- [ ] **Step 2: The dialog**

Extend `cache_list.rs` (add these imports at the top: `use crate::window::Ui; use gtk4 as gtk; use gtk::prelude::*; use opusdm_ipc::ToHub; use std::rc::Rc;`) with:

```rust
/// Ask the hub for every Lister's cached paths; the reply opens the dialog.
pub fn open(ui: &Rc<Ui>) {
    if let Err(e) = ui.hub.client.send(&ToHub::QueryCaches) {
        crate::actions::flash(ui, &format!("cannot reach the hub: {e}"));
    }
}

/// Ask every Lister to drop its cache.
pub fn free_all(ui: &Rc<Ui>) {
    if let Err(e) = ui.hub.client.send(&ToHub::FreeAllCaches) {
        crate::actions::flash(ui, &format!("cannot reach the hub: {e}"));
    }
}

/// Show the dialog for a `CacheList` reply. Double click raises the owning
/// Lister and navigates it there; "Open here" navigates this Lister.
pub fn show(ui: &Rc<Ui>, entries: Vec<CacheEntry>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Cache List").size(560, 420);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    for entry in &entries {
        let label = gtk::Label::new(Some(&entry_label(entry)));
        label.set_xalign(0.0);
        list.append(&label);
    }
    if entries.is_empty() {
        shell.content().append(&gtk::Label::new(Some("No directory is cached.")));
    }
    let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    shell.content().append(&scroller);

    let entries = Rc::new(entries);
    {
        let (ui, entries) = (ui.clone(), entries.clone());
        list.connect_row_activated(move |_, row| {
            let Some(entry) = entries.get(row.index() as usize) else { return };
            let msg = ToHub::JumpTo { lister: entry.lister, path: entry.path.clone() };
            if let Err(e) = ui.hub.client.send(&msg) {
                crate::actions::flash(&ui, &format!("cannot reach the hub: {e}"));
            }
        });
    }
    let here = shell.button("Open here");
    let free = shell.button("Free all caches");
    shell.close_button("Close");
    let window = shell.window().clone();
    {
        let (ui, entries, list, window) = (ui.clone(), entries.clone(), list.clone(), window.clone());
        here.connect_clicked(move |_| {
            let Some(row) = list.selected_row() else {
                crate::actions::flash(&ui, "select a directory first");
                return;
            };
            let Some(entry) = entries.get(row.index() as usize) else { return };
            ui.navigate_to(&entry.path);
            window.close();
        });
    }
    {
        let (ui, window) = (ui.clone(), window.clone());
        free.connect_clicked(move |_| {
            free_all(&ui);
            window.close();
        });
    }
    shell.present();
}
```

- [ ] **Step 3: Wire the reply and the commands**

`window.rs`: replace the `ToLister::CacheList(_) => {}` arm from Task 5 with `ToLister::CacheList(entries) => crate::cache_list::show(self, entries),`.

`keys.rs` `CommandId`: add `CacheList` and `FreeCaches` (doc: "Open the Cache List." / "Drop every Lister's directory cache."). `palette.rs`: entries (category "View", no shortcut) "Cache List..." and "Free Caches", and in `execute_command`: `CommandId::CacheList => crate::cache_list::open(ui), CommandId::FreeCaches => crate::cache_list::free_all(ui),`.

- [ ] **Step 4: Build, test, manual check**

Run: `cargo test --workspace 2>&1 | grep -E "test result|FAILED|^error"; cargo clippy -p opusdm-lister 2>&1 | grep -E "cache_list" | head`
Expected: PASS. Manual: with two Listers open and a few directories visited in each, palette -> "Cache List..." shows both Listers' directories; double click on the other Lister's entry raises that window and shows the directory instantly; "Open here" opens the selected path in the current Lister; "Free all caches" empties the list when reopened; "Free Caches" from the palette does the same without a dialog.

- [ ] **Step 5: Commit**

```bash
git add crates/lister/src
git commit -m "feat(lister): Cache List dialog, Free Caches and jump to a cached directory"
```

---

### Task 8: Documentation, tracking and final verification

**Files:**
- Modify: `docs/usage.md`, `CHANGES.md`, `TODO.md`

- [ ] **Step 1: usage.md**

After the "3.10c Lister formats and Show/Hide patterns" section add:

```markdown
### 3.10d Directory cache, history and Cache List
- Each Lister remembers the directories it read recently (20 by default; **Directory cache size** under **BEHAVIOUR** in Settings, 0 turns it off). Going back to one shows its list at once, and the disk is read again in the background: if something changed the list updates without losing your selection. `Ctrl+R` always reads the disk.
- Next to the path bar, **back** and **forward** buttons and a **history** drop-down list the directories you visited in this Lister (newest first, the current one in bold). `Alt+Left` and `Alt+Right` do the same from the keyboard. Going somewhere new after stepping back drops the entries you stepped back over. The history keeps 50 entries and is not saved between sessions.
- **Cache List...** (command palette) shows the cached directories of every Lister. Double click an entry to raise the Lister that holds it and show the directory there; **Open here** opens the selected directory in the Lister you are in; **Free all caches** empties every Lister's cache (also available on its own as **Free Caches**).
```

- [ ] **Step 2: CHANGES.md**

Append to the `## 2026-09-29` section:

```markdown
- Feature (F-24): directory cache, history and Cache List. New `dircache.rs` (LRU of raw listings per Lister, `same_listing` order-insensitive comparison) and `history.rs` (browser style stack, 50 entries). Navigating to a cached directory shows it at once and always re-reads the disk on a thread, replacing the view only when the listing changed (selection kept by name); reload, auto-refresh and job refreshes bypass the cache; archives are never cached. Back / forward / history-menu widgets beside the path bar, `Alt+Left` / `Alt+Right`, palette "History Back/Forward". Protocol: `ToHub::{CacheReport, QueryCaches, FreeAllCaches, JumpTo}`, `ToLister::{CacheList, FreeCaches, GoTo}`, `CacheEntry`; the hub keeps each Lister's reported paths per connection. "Cache List..." dialog (double click raises the owning Lister, "Open here", "Free all caches") and "Free Caches" in the palette. `Behavior.cache_size` (default 20, clamped 0..=200) with a spin button in the BEHAVIOUR section of Settings.
```

- [ ] **Step 3: TODO.md**

Tick `F-24` (`- [x] F-24 Directory buffer cache + CacheList + PathHistory`) and tick item 4 of "Suggested order" (`4. [x] Lister formats and patterns (F-22, F-23, F-24, F-29, F-32)`).

- [ ] **Step 4: Final verification**

Run: `cargo test --workspace 2>&1 | grep -E "^test result|FAILED|SIGABRT"` and `cargo clippy --workspace 2>&1 | grep -E "dircache|history|nav_cache|cache_list" | head`
Expected: all results `ok`, no clippy findings in the new files. Then run the acceptance checks: revisit a 10k-file directory from history and see it instantly; a Cache List jump raises the right window.

- [ ] **Step 5: Commit**

```bash
git add docs/usage.md CHANGES.md TODO.md
git commit -m "docs: directory cache, history and Cache List (F-24)"
```
