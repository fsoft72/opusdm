# Directory cache, path history and CacheList (F-24)

Block C of item 4 of the suggested order (see `TODO.md`). Blocks A (F-29, F-32)
and B (F-22, F-23) are done.

## Goal

Make going back to a directory instant, give every Lister back/forward and a
history menu, and let the user see every cached directory of every Lister in a
hub-wide list to jump to, or to free.

## Decisions taken with the user

- Staleness: **show the cache at once and always re-read the directory in the
  background** (stale-while-revalidate). No invalidation logic, no inotify on
  cached directories.
- CacheList entry: double click **raises the owning Lister and navigates it
  there**; an **"Open here"** button opens the path in the Lister the dialog
  was opened from.
- History: **browser style stack** (back, forward, a new navigation drops the
  forward part), menu shows the stack, `Alt+Left` / `Alt+Right`.
- The hub learns the caches by **push**: each Lister reports its cached paths
  when they change.

## 1. Pure modules (`crates/lister/src`, no GTK)

### `dircache.rs`

```rust
pub struct DirCache { /* capacity, map PathBuf -> Rc<Vec<Row>>, MRU order */ }
impl DirCache {
    pub fn new(capacity: usize) -> Self;
    pub fn get(&mut self, dir: &Path) -> Option<Rc<Vec<Row>>>;   // marks most recent
    pub fn put(&mut self, dir: PathBuf, rows: Vec<Row>) -> Rc<Vec<Row>>;
    pub fn remove(&mut self, dir: &Path) -> bool;
    pub fn clear(&mut self);
    pub fn set_capacity(&mut self, capacity: usize);             // evicts oldest
    pub fn paths(&self) -> Vec<PathBuf>;                          // most recent first
}
```

Capacity 0 stores nothing. `Behavior.cache_size` (default 20) is clamped to
`0..=200` by `Behavior::cache_capacity()`. The cache holds the raw rows from
disk, before the Show/Hide filter and the dot-file filter, so changing a
format never invalidates it.

### `history.rs`

```rust
pub struct History { /* entries: Vec<PathBuf>, index */ }
impl History {
    pub fn new() -> Self;
    pub fn push(&mut self, dir: PathBuf);         // no-op if it equals the current entry
    pub fn back(&mut self) -> Option<PathBuf>;    // moves the index
    pub fn forward(&mut self) -> Option<PathBuf>;
    pub fn jump(&mut self, index: usize) -> Option<PathBuf>;
    pub fn entries(&self) -> &[PathBuf];
    pub fn index(&self) -> Option<usize>;
    pub fn can_back(&self) -> bool;
    pub fn can_forward(&self) -> bool;
}
```

`push` drops everything after the current index first, then appends; the stack
holds at most 50 entries (`HISTORY_LIMIT`), the oldest falls off.

## 2. Lister integration

- `navigate_to_then`: for a non-archive path found in the cache, apply the
  cached rows immediately (`apply_navigation`), then still start the disk read.
  When the read returns, compare it with what is on screen (name, is_dir, size,
  mtime, mode per row); if it differs apply it keeping the selection by name
  (the same helper `reload` uses), if not do nothing. Either way the cache is
  updated with the fresh rows.
- Reload (`Ctrl+R`), auto-refresh and job refreshes read the disk and update the
  cache; `ToLister::InvalidatePath` removes the entry. Archive paths are never
  cached.
- After every successful `apply_navigation` the Lister sends
  `ToHub::CacheReport(cache.paths())`.
- History: `apply_navigation` pushes `dir` unless the navigation came from
  back / forward / the menu (a `history_move: Cell<bool>` set by
  `navigate_history`). Failed navigations (error path) are not pushed.
- The cache size follows `SettingsUpdate` (`set_capacity`).

## 3. History UI

Three widgets next to the path entry: back, forward, and a drop-down button
listing the stack (current entry marked, click to jump). Back/forward are
insensitive when there is nothing to go to. `Alt+Left` / `Alt+Right` are read in
the window key handler before `keys::resolve` (which does not take Alt) and run
the new `CommandId::HistoryBack` / `HistoryForward`, which are also in the
palette.

## 4. Protocol and hub

New messages:

- `ToHub::CacheReport(Vec<PathBuf>)`: this Lister's cached paths (replaces the previous report).
- `ToHub::QueryCaches`: answered with `ToLister::CacheList(Vec<CacheEntry>)` to the sender.
- `ToHub::FreeAllCaches`: the hub broadcasts `ToLister::FreeCaches` to every registered Lister.
- `ToHub::JumpTo { lister: ListerId, path: PathBuf }`: the hub forwards `ToLister::GoTo { path }` to that Lister.
- `CacheEntry { lister: ListerId, path: PathBuf }`.

`Conn` gets `cache_paths: Vec<PathBuf>`; entries vanish with the connection. On
`GoTo` the Lister presents its window and navigates (its own cache makes it
instant). On `FreeCaches` it clears the cache and reports the empty list.
`JumpTo` to an unknown or unregistered id is ignored.

## 5. Dialog, commands, setting

- `cache_list.rs`: "Cache List..." (palette). Sends `QueryCaches`; the
  `CacheList` reply fills a list of `Lister N: /path` rows (N is the ListerId).
  Double click sends `JumpTo`. Buttons: **Open here** (navigates the current
  Lister), **Free all caches** (`FreeAllCaches`), **Close**.
- Palette "Free Caches": `FreeAllCaches` without opening the dialog.
- `Behavior.cache_size` with a spin button "Directory cache size (0 = off)" in
  the BEHAVIOUR section of Settings.

## Edge cases

- A cached directory that was deleted or renamed: the cached rows show for a
  moment, then the failed disk read shows the usual error; the entry is removed
  from the cache.
- Cache 0: behaves as today, nothing is stored, history still works.
- The dialog opened while the Lister list changes: the reply is a snapshot;
  `JumpTo` to a Lister that closed is ignored.
- Huge directories: the cache keeps `Rc<Vec<Row>>`, so applying a cached
  listing clones rows into the store only once and eviction frees them.

## Testing

Pure unit tests: LRU order, eviction, `get` refreshing recency, `set_capacity`
shrinking, capacity 0; history push / back / forward / jump, forward dropped on
a new push, same path not duplicated, limit of 50. `Behavior::cache_capacity`
clamping and the default. Protocol roundtrips for the new messages. Hub tests:
`CacheReport` then `QueryCaches` from another Lister returns both Listers'
paths, a disconnected Lister's paths disappear, `FreeAllCaches` reaches every
Lister, `JumpTo` reaches only the target.

Acceptance (from FEATURES.md): revisit a 10k-file directory instantly from
history; a CacheList jump focuses the right window.

## Out of scope

Persisting history or cache between sessions, caching archive listings,
inotify on cached directories, drag reordering of the history.
