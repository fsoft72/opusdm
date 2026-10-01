# Optimization TODO

> Generated on 2026-10-01. Items sorted by importance.

## Critical

- [x] **Config saves are not atomic** - `save_json` writes straight over the target with `fs::write`; a crash, a full disk or a kill mid-write leaves a truncated `settings.json`, `functions.json`, `bindings` file, bank or group. Write to a sibling temp file, `fsync`, then `rename` over the target (the theme writer already does a staging swap).
  - File(s): `crates/ipc/src/settings.rs`
- [x] **A malformed `settings.json` / `functions.json` is silently replaced by defaults** - `load_json` returns `T::default()` on a parse error without keeping a copy, so the next save destroys every user setting or every saved Function. Bookmarks, formats, file types, bindings and banks already copy the bad file to `.bak`; move that logic into `load_json` itself so every entity gets it.
  - File(s): `crates/ipc/src/settings.rs`, `crates/ipc/src/function.rs`, `crates/ipc/src/leftout.rs`

## High

- [x] **Icon textures are decoded on every row bind** - the Name column calls `Texture::from_bytes` (PNG/SVG decode) each time a row is bound, so scrolling a large directory decodes the same handful of icons thousands of times; the icon view does the same per item. Cache `gdk::Texture` per `(IconSetId, IconKind, ext)` in `opusdm_iconview::icon_set` and share it between both views.
  - File(s): `crates/lister/src/columns.rs`, `crates/iconview/src/item.rs`, `crates/iconview/src/icon_set.rs`
- [ ] **Copying out of an archive blocks the GTK thread** - `wire_transfer` runs `extract_items_to_dir` (a synchronous `unzip` / `tar` subprocess) inside the button handler, freezing the Lister for big archives. Run the extraction on a thread and submit the job from `glib::spawn_future_local`, like `preview.rs` and `tools.rs` already do.
  - File(s): `crates/lister/src/actions.rs`, `crates/lister/src/archive.rs`
- [ ] **Archive temp folders are shared and never cleaned** - extraction goes to `opusdm-extract-<pid>` (reused by every copy of the session, never removed) and previews to a fixed `/tmp/opusdm-preview` (shared by every Lister and every user, a predictable path). Use a `tempfile::TempDir` per operation, removed when the job ends.
  - File(s): `crates/lister/src/actions.rs`, `crates/lister/src/archive.rs`
- [x] **`extract_single_file_temp` returns the wrong path and ignores failures** - the tar branch extracts `inner_file` with its subfolders but returns `temp_dir/<file name>`, so any nested entry opens a missing file; neither branch checks the exit status (a failed `unzip -p` writes an empty file that is then shown as the content), and `.7z` (accepted by `is_archive_file`) always falls to `tar`. Check `status.success()`, return `temp_dir.join(inner_file)` and reject unsupported formats explicitly.
  - File(s): `crates/lister/src/archive.rs`
- [ ] **Move from an archive does not move** - `wire_transfer` extracts the items and submits the same `Op::Move` on the extracted copies, so the user sees a successful Move while the archive is untouched. Either refuse Move inside an archive with a clear message or turn it into Copy and say so.
  - File(s): `crates/lister/src/actions.rs`
- [ ] **A new command must be added in three places** - `CommandId` (`keys.rs`), the `COMMANDS` table and the 330-line `execute_command` match (`palette.rs`) are kept in step by hand; it is easy to commit one without the others (it happened with `SizeFormat`). Keep title, category and shortcut next to the variant (a `CommandId::meta()` method) and split `execute_command` into per-category functions.
  - File(s): `crates/lister/src/keys.rs`, `crates/lister/src/palette.rs`

## Medium

- [ ] **`hub/src/server.rs` is a god module** - about 1,670 lines of production code (plus 1,300 of tests) mixing connection handling, roles, Function runs, script events, persistence and hide/reveal; `lister_msg` alone is a 200-line match. Split the Function-run state machine (`start_function_run`, `selection_reply`, `function_event`, `function_conflict`, `function_confirm`) and the persistence thread into their own modules.
  - File(s): `crates/hub/src/server.rs`
- [ ] **`hub/src/toolbar.rs` holds the whole desktop** - despite its name it builds desktop windows, icon fields, desktop menus, groups, the About dialog and quit handling (1,670 lines). Split into `desktop_windows.rs`, `desktop_menu.rs` and `groups_ui.rs`, keeping only the toolbar in `toolbar.rs`.
  - File(s): `crates/hub/src/toolbar.rs`
- [ ] **"Load JSON, back up if malformed" is copied in six modules** - `bookmarks`, `format`, `filetypes`, `bindings`, `buttonbank` and `groups` each reimplement the read / parse / copy-to-`.bak` / log sequence with small differences (some keep an existing backup, some overwrite it). One `load_json_with_backup` in `settings.rs` removes the drift (pairs with the Critical item above).
  - File(s): `crates/ipc/src/bookmarks.rs`, `crates/ipc/src/format.rs`, `crates/ipc/src/filetypes.rs`, `crates/ipc/src/bindings.rs`, `crates/ipc/src/buttonbank.rs`, `crates/ipc/src/groups.rs`
- [x] **Archive item names can be read as options** - `extract_items_to_dir` appends entry names straight after `unzip` / `tar` arguments; an entry named `-something` becomes a flag, and `unzip` also treats `[`, `*`, `?` in names as wildcards. Pass `--` before the names for `tar` and escape the wildcard characters for `unzip`.
  - File(s): `crates/lister/src/archive.rs`
- [ ] **Remote and archive operations ignore setup errors** - `let _ = std::fs::create_dir_all(&temp_extract)` and similar calls swallow the real cause, so the user later sees a vaguer "extraction failed". Propagate the error into the `flash` message.
  - File(s): `crates/lister/src/actions.rs`, `crates/lister/src/remote_xfer.rs`
- [ ] **`apply_navigation` does too much** - one function filters rows, totals sizes, applies formats, resets the cursor, syncs the terminal, the watcher, the history, the gauge and the icon view. Extract the pure "rows -> visible rows + totals" step into `model.rs` so it can be unit-tested without GTK.
  - File(s): `crates/lister/src/window.rs`, `crates/lister/src/model.rs`

## Low / Nice to have

- [ ] **Changing the Size Format re-reads the directory** - `size_format::open` calls `ui.reload()`, which hits the disk again; `ui.touch_rows()` already rebinds the rows without I/O.
  - File(s): `crates/lister/src/size_format.rs`
- [ ] **Timezone offset is computed once per column factory** - `local_tz_offset_secs()` is captured when the Modified column is built, so after a DST change every Lister shows times off by one hour until it is reopened. Compute it per bind (cheap) or refresh it on navigation.
  - File(s): `crates/lister/src/columns.rs`
- [ ] **User and group name caches are duplicated** - `format_user` and `format_group` repeat the same `Mutex<Option<HashMap>>` lookup code. One generic `cached_name(cache, id, resolve)` helper removes it.
  - File(s): `crates/lister/src/model.rs`
- [ ] **`save_functions` clones the whole list just to serialize it** - `&functions.to_vec()` allocates a copy; `save_json` can take the slice directly.
  - File(s): `crates/ipc/src/function.rs`
- [ ] **`wire_transfer` repeats the "nothing selected" / submit tail three times** - the remote, archive and local branches each check the selection and call `submit_job` with the same options. Resolve the sources first, then submit once.
  - File(s): `crates/lister/src/actions.rs`
