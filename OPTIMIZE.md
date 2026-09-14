# Optimization TODO

> Generated on 2026-04-15. Items sorted by importance.

## Critical

- [x] **Sort comparator does a downcast + `Rc` clone per comparison** — The `CustomSorter` closure in `build_window` downcasts each item to `BoxedAnyObject`, borrows and clones an `Rc<Row>` for every pairwise comparison; on a large directory this turns sorting into the hottest, most allocation-heavy path in the UI. Borrow in place (`o.borrow::<Rc<Row>>()` then compare the refs) instead of cloning the `Rc`.
  - File(s): `crates/lister/src/window.rs` (model_sorter closure)

- [x] **Size calculation result lookup is O(n) per result and replaces rows by name** — `calculate_dir_sizes_async` scans the entire store for every computed directory and matches on `row.name`, which both costs O(n*m) and silently writes the size into the wrong row if two entries share a name (e.g. after filtering or with symlinked names). Keep the original store index and splice that directly.
  - File(s): `crates/lister/src/tools.rs`

- [x] **`reload()` re-reads the whole directory for single-file changes** — Icon rename, thumbnail updates and settings changes all call `reload()`, which runs `navigate_to` (full `read_dir_rows` + store splice of every item). For a 10k-file directory this is a visible stall for a one-entry change; add a targeted "update this row" path.
  - File(s): `crates/lister/src/window.rs` (`reload`, icon rename handler), `crates/lister/src/tools.rs`

## High

- [ ] **Icon view is one GTK widget per file** — `IconField` keeps a `Vec<IconItem>` of widgets inside a `gtk::Fixed`; large directories will exhaust widget resources and layout time. For directories beyond a threshold, switch to a `ListView`/grid with factories or a single custom-drawn widget.
  - File(s): `crates/iconview/src/field.rs`, `crates/iconview/src/item.rs`

- [x] **`IconField` Inner is a god struct** — 20+ fields mixing layout state, selection state, drag state, 8 listener lists and thumbnail plumbing in one struct makes every interaction handler reach across unrelated concerns. Split into `LayoutState`, `SelectionState` and a small event-emitter helper.
  - File(s): `crates/iconview/src/field.rs`

- [x] **Two hand-rolled inotify implementations** — `lister/src/watcher.rs` and `hub/src/desktop_files.rs` each implement raw `libc::inotify_*` setup, poll loops and event parsing independently; fixes to one (event parsing, fd lifecycle) will not reach the other. Extract a shared inotify helper (e.g. into `crates/ipc` or a new small crate).
  - File(s): `crates/lister/src/watcher.rs`, `crates/hub/src/desktop_files.rs`

- [x] **Checksums computed by spawning external processes** — `compute_file_hashes` shells out to `sha256sum` and `md5sum` per file, adding process-spawn overhead per file and a hard runtime dependency on coreutils, while `md-5` is already a workspace dependency and sha2 is trivially addable. Hash in-process with a streaming reader.
  - File(s): `crates/lister/src/tools.rs`

- [x] **`Ui` struct is a god object shared by every callback** — 30+ public fields on `Ui` (window, models, labels, settings, watcher, terminal, preview...) mean any callback can touch anything, and modules like `actions.rs`/`palette.rs`/`tools.rs` couple to the whole window. Group into sub-structs (status bar, view state, hub connection) and pass only what each module needs.
  - File(s): `crates/lister/src/window.rs`

- [x] **409 `unwrap()` calls across crates** — Many are on GTK downcasts and `Display::default()` in UI code, which is mostly benign, but the same habit in hub/fsops paths (e.g. `expect("activate once")`, thread spawns, sorter downcasts) can panic on legitimate runtime conditions. Audit unwraps outside tests and downgrade to logged errors where the app can continue.
  - File(s): repo-wide, notably `crates/lister/src/window.rs`, `crates/hub/src/server.rs`

## Medium

- [ ] **`window.rs` is a 1100-line module doing build, wiring and behavior** — `build_window` alone constructs every widget, wires ~15 signal handlers and defines the sort machinery; navigation, status, preview and key handling also live in the same file. Split into `build.rs` (construction) and keep behavior in focused modules.
  - File(s): `crates/lister/src/window.rs`

- [ ] **Selection scans are O(n) and repeated per event** — `selected_names`, `update_status` and `sync_cursor_from_selection` each walk `0..selection.n_items()` with `is_selected(i)` + `row_at(i)`; the selection-changed signal triggers several of these back to back. Use `gtk::Bitset` from `MultiSelection::selection()` to iterate only selected indices once.
  - File(s): `crates/lister/src/window.rs`

- [x] **`detect_file_kind` hardcodes long extension lists in a match** — The image/archive/text extension tables are inline string literals duplicated in spirit with `archive::is_archive_file`; one new format means editing two places. Move the tables to a shared const/module (e.g. in `ipc` or `archive`) and reuse.
  - File(s): `crates/lister/src/preview.rs`, `crates/lister/src/archive.rs`

- [x] **Icon zoom steps are magic numbers with a fragile `unwrap_or(1)` fallback** — `STEPS: [48, 64, 96, 128]` in the zoom handler silently resets to index 1 if `icon_size` was ever set to a value not in the list; the default 64 in `build_window` and `IconField::new` is also repeated by literal. Define the steps and default as shared constants.
  - File(s): `crates/lister/src/window.rs`, `crates/iconview/src/field.rs`

- [ ] **Settings `Welcome` variant handled with a comment, not the type system** — `ToLister::Welcome` is documented as "consumed synchronously, never reaches here" yet still has a match arm every Lister must write. Split the handshake message from the steady-state protocol (separate enum or a dedicated connect function returning the welcome payload).
  - File(s): `crates/lister/src/window.rs` (`apply_inbound`), `crates/lister/src/client.rs`, `crates/ipc/src/proto.rs`

- [ ] **Hub server spawns unnamed, untracked forwarder threads** — Two `std::thread::spawn` forwarder loops (GTK cmd, job progress) plus per-connection reader threads are created without names or join handles; a leaked Lister connection keeps a reader thread forever. Name them consistently and log thread exit, or consolidate the forwarders into the owner loop's `Receiver` set.
  - File(s): `crates/hub/src/server.rs`

- [x] **Checksum dialog builds near-identical SHA/MD5 rows by copy-paste** — ~40 lines duplicated for the two hash rows including the Copy-button closure; extract a small `hash_row(label, value)` helper.
  - File(s): `crates/lister/src/tools.rs` (`open_checksum_dialog`)

## Low / Nice to have

- [ ] **`filter_model` field marked `#[allow(dead_code)]`** — The `Ui::filter_model` field is stored but never read (the count closure captures its own clone). Either remove the field or route the count logic through it.
  - File(s): `crates/lister/src/window.rs`

- [x] **Fallback hash path reads only 64 KiB then reports "unavailable"** — In `compute_file_hashes`, if spawning `sha256sum` fails the code reads one buffer into `file` and discards the result; the `file`/`buffer` are dead work. Drop the fallback or actually hash in-process.
  - File(s): `crates/lister/src/tools.rs`

- [ ] **`Role` cycle logic embedded in a gesture closure** — The SOURCE -> DEST -> NONE transition with the pinned-source shortcut is business logic hidden inside a `connect_released` closure; move it to a pure function on `Roles` so it can be unit tested.
  - File(s): `crates/lister/src/window.rs`, `crates/lister/src/client.rs`

- [ ] **Preview kind detection reads 512 bytes on the UI thread** — `detect_file_kind` does a synchronous `File::open` + `read` during `load_file`, on the GTK thread; fine for local disk, but it stalls on network mounts. Move the probe off-thread or make it lazy.
  - File(s): `crates/lister/src/preview.rs`

- [ ] **`dlog!`/`eprintln!` used ad hoc for diagnostics** — Hub startup, watcher failures and debug traces go to stderr with no level control; introduce a tiny log macro with an env-var level so production runs are quiet and debugging does not need recompiles.
  - File(s): `crates/lister/src/window.rs`, `crates/hub/src/server.rs`, `crates/lister/src/watcher.rs`
