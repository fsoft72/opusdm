# Optimization TODO

> Generated on 2026-09-14. Items sorted by importance.

## Critical

- [x] **Bound and invalidate thumbnail work queues** - Every directory refresh enqueues every previewable file into an unbounded channel, while generation checks happen only after workers finish expensive decoding or subprocess work; rapid navigation through large folders can retain stale paths and spend minutes processing results that will be discarded.
  - File(s): `crates/iconview/src/field.rs`, `crates/iconview/src/thumbs/pool.rs`

## High

- [x] **Move directory and archive loading off the GTK thread** - `navigate_to` performs `read_dir`, per-entry metadata calls, and archive subprocess execution synchronously, so large, slow, or network-mounted directories freeze the whole Lister UI.
  - File(s): `crates/lister/src/window.rs`, `crates/lister/src/model.rs`, `crates/lister/src/archive.rs`

- [x] **Stop accumulating CSS providers** - Every desktop theme update adds a new display-level `CssProvider` without removing or reusing the previous provider, causing process-lifetime memory growth and an ever-growing cascade of obsolete styles.
  - File(s): `crates/hub/src/toolbar.rs`

- [x] **Keep blocking socket and disk I/O out of the hub owner loop** - The single owner thread synchronously writes IPC responses to every client and persists settings, so one non-reading client or slow filesystem can stall connection lifecycle, role changes, job progress, and all other clients.
  - File(s): `crates/hub/src/server.rs`, `crates/hub/src/settings.rs`, `crates/hub/src/desktop_settings.rs`

- [x] **Reload desktop entries outside the GTK executor** - The desktop watcher future calls `read_entries` synchronously on the main loop for every coalesced event, which can make the panel and desktop unresponsive during large directory rescans.
  - File(s): `crates/hub/src/toolbar.rs`, `crates/iconview/src/scan.rs`

## Medium

- [x] **Preserve view state when changing icon sets** - `set_icon_set` delegates to `set_entries`, which clears selection, cursor, free positions, thumbnail cache, and requeues all thumbnails even though only fallback icon rendering changed.
  - File(s): `crates/iconview/src/field.rs`

- [x] **Avoid full icon-model rebuilds for small updates** - Rename, row rebinding, sorting, and refresh paths repeatedly clone every visible row and call `set_entries`, rebuilding layout, selection, caches, and thumbnail work instead of applying an incremental update.
  - File(s): `crates/lister/src/window.rs`, `crates/iconview/src/field.rs`

- [x] **Make directory scans resilient per entry** - A single metadata or symlink metadata error aborts the entire listing after `read_dir` succeeds, whereas only `file_type` errors are skipped; transient races or one unreadable entry can therefore prevent users from opening an otherwise valid directory.
  - File(s): `crates/lister/src/model.rs`

- [x] **Replace the sysmon sample vector with a ring buffer** - Every tick removes index zero, shifts the history, clones every trace, and reinstalls the drawing closure; retain one draw function over shared state and use `VecDeque` or a circular index to eliminate recurring copies and callback churn.
  - File(s): `crates/panel/src/widgets/sysmon.rs`

- [x] **Split the hub owner-loop message handlers** - The large `owner_loop` combines connection allocation, authentication state, role transitions, settings persistence, job submission, conflict replies, and GTK commands in one match, making lifecycle invariants difficult to test independently and future protocol changes risky.
  - File(s): `crates/hub/src/server.rs`

- [ ] **Decompose the icon field implementation** - `IconField` centralizes GTK controller wiring, selection, drag-and-drop, keyboard navigation, virtualization, layout, renaming, and thumbnails in one 1,300-line module; extract behavior-focused controllers while retaining the existing `IconField` facade.
  - File(s): `crates/iconview/src/field.rs`

## Low / Nice to have

- [ ] **Centralize settings persistence** - Configuration path fallback, JSON loading, directory creation, and default-on-error behavior are duplicated across the hub and desktop configuration app, which can drift as persistence behavior evolves.
  - File(s): `crates/hub/src/settings.rs`, `crates/hub/src/desktop_settings.rs`, `crates/desktop-config/src/model.rs`

- [ ] **Report malformed settings instead of silently resetting them** - JSON parse and read failures currently collapse to defaults without diagnostics, hiding configuration corruption and making unexpected resets difficult to troubleshoot.
  - File(s): `crates/hub/src/settings.rs`, `crates/hub/src/desktop_settings.rs`, `crates/desktop-config/src/model.rs`

- [x] **Share the inotify debounce loop** - The hub desktop watcher and Lister watcher independently implement poll, drain, coalesce, shutdown, and channel notification logic with different timing behavior; move the reusable loop behind the IPC inotify utility to reduce Linux event-handling drift.
  - File(s): `crates/hub/src/desktop_files.rs`, `crates/lister/src/watcher.rs`, `crates/ipc/src/inotify.rs`
