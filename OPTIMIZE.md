# Optimization TODO

> Generated on 2026-10-02. Items sorted by importance.

## Critical

- [x] **Recycled connection ids let a stale EOF kill a new Lister** - `Hub::accepted` hands out the lowest free id, and `drop_conn` frees it right away. The old reader thread still sends `ListerEof(cid)` (and any queued `FromLister(cid, ..)`) after `Goodbye`/`ChildExited`, so a Lister that connects in between gets that id and is dropped or receives the old one's messages. Jobs, confirms and conflict prompts keyed by `cid` can also reach the wrong window. Use ids that only ever grow (drop `free_conns`), or tag each reader with a per-connection generation and ignore stale messages.
  - File(s): `crates/hub/src/server.rs` (`accepted`, `drop_conn`, `reader_loop`, `owner_loop`)

- [x] **Copy can truncate the source file to zero (no same-file check)** - `validate_job` only compares paths as text, so a DEST reached through a symlink, a bind mount or a different spelling of the same folder gets past it. If the user then picks "overwrite", `copy_file` opens the destination with `truncate(true)`, which is the source inode, and the data is gone. Function steps call `run_job` without even the text check. Compare `(dev, ino)` of source and target before opening for write, and run `validate_op` on the function path too.
  - File(s): `crates/fsops/src/copy.rs` (`copy_file`), `crates/hub/src/jobs.rs` (`validate_job`), `crates/hub/src/functions.rs` (`run_internal`)

- [x] **Overwrite destroys the existing file when the copy fails or is cancelled** - With `overwrite == true` the destination is truncated in place before any byte is copied. A cancel, a full disk or a read error then removes it (`remove_file(dst)`), so the user loses both the old version and the new one. Copy into a temp file in the same folder and `rename` it over the target only after it succeeds.
  - File(s): `crates/fsops/src/copy.rs` (`copy_file`)

- [x] **Every rename silently overwrites an existing file** - `std::fs::rename` replaces the target on POSIX, and none of the 7 call sites checks for it. Renaming `a.txt` to `b.txt` deletes the old `b.txt` without asking. Batch rename makes it worse: chains such as `a->b, b->c` depend on the order and can lose files. Add one `fsops::rename_noreplace` helper (`renameat2(RENAME_NOREPLACE)`) and use it everywhere, and plan batch renames through temporary names.
  - File(s): `crates/lister/src/actions.rs:162`, `crates/lister/src/commands.rs:146`, `crates/lister/src/context_menu.rs:77`, `crates/lister/src/batch_rename.rs:211`, `crates/lister/src/build.rs:685`, `crates/hub/src/functions.rs:374`, `crates/hub/src/desktop_ops.rs:80`

## High

- [x] **Rename dialogs read the folder when the user confirms, not when they opened** - The callbacks build the new path with `ui.cwd_path().join(&new_name)` when OK is pressed. If the Lister moved to another folder while the dialog was open (watcher, hub `GoTo`, a keyboard shortcut), the file is *moved* into that folder. Build the target from `old.parent()`, and reject names that contain `/` or are empty.
  - File(s): `crates/lister/src/actions.rs:161`, `crates/lister/src/commands.rs:145`, `crates/lister/src/context_menu.rs:76`

- [x] **Auto-refresh still clears the filter the user is typing** - `on_directory_changed` puts the filter text back right after `reload()`, but `reload` is asynchronous: `apply_navigation` runs later, when the listing arrives, and calls `dismiss_filter()`, which wipes the restored text. Pass a "keep filter" flag through `navigate` (or skip `dismiss_filter` when the folder did not change) instead of restoring it after the call.
  - File(s): `crates/lister/src/window.rs` (`on_directory_changed`, `apply_navigation`), `crates/lister/src/nav_cache.rs`

- [ ] **Restoring the selection after a reload is quadratic** - `select_names` checks each row with `names.contains` (O(rows x selected)) and calls `select_item` once per match. Each call fires `selection-changed`, which runs `update_status` (it walks the whole selection) and `update_preview_if_open`. With "select all" on 10k files every inotify refresh does on the order of 10^8 operations. Use a `HashSet`, fill one `gtk::Bitset` and call `set_selection` once.
  - File(s): `crates/lister/src/nav_cache.rs` (`select_names`), `crates/lister/src/build.rs:570`

- [ ] **Function steps for Copy/Move/Delete skip the job engine** - `run_internal` calls `run_job` with a fresh `CancelToken::new()` that nothing can cancel. These jobs get no hub progress window and send no `InvalidatePath`, so the Listers that are open stay stale. `run_steps` also has no cancel check and keeps running steps after the hub side has gone (send errors are ignored). Send these steps through `JobEngine::submit` and give each function run a cancel token.
  - File(s): `crates/hub/src/functions.rs` (`run_internal`, `run_steps`), `crates/hub/src/server/function_run.rs`

- [ ] **`functions.json` is read from disk on every use and written by each Lister on its own** - Bindings, formats and settings live in the hub and are pushed to the Listers. Functions are not: `load_functions()` is called on every right-click, palette open, hotkey-editor key press and bank-button check, and each Function Editor saves the file itself, so the last save wins between two Listers. Make the hub own the functions (`SetFunctions` / `FunctionsUpdate`), just like `SetBindings`.
  - File(s): `crates/ipc/src/function.rs`, `crates/lister/src/context_menu.rs:124`, `crates/lister/src/palette.rs:46`, `crates/lister/src/hotkey_editor.rs:295`, `crates/hub/src/bank_windows.rs:42`, `crates/lister/src/commands.rs:247`, `crates/hub/src/server/function_run.rs:72`

- [ ] **A new X11 connection for every window-manager call** - About 20 helpers each call `x11rb::connect(None)`, intern atoms, sync and close again. Placement is also retried at 80/400/1000 ms, so one map costs several connections. The last commit (`x11_sync`) worked around the side effects of these short-lived connections. Keep one lazily opened connection per process in `opusdm-ui` (atoms interned once), with shared `pin_geometry` / `set_below` / `raise_above` helpers.
  - File(s): `crates/ui/src/x11.rs`, `crates/panel/src/window.rs`, `crates/panel/src/ewmh.rs`, `crates/dock/src/window.rs`, `crates/hub/src/desktop.rs`, `crates/buttonbank/src/x11.rs`

- [ ] **The system tray thread wakes up 60 times a second forever** - The event loop polls `cmd_rx.try_recv()` and sleeps 16 ms when nothing is there, which keeps a CPU core waking up even when the desktop is idle. Wait on the X connection fd with `poll(2)` and wake the loop for commands with a pipe/eventfd, or send them as an X `ClientMessage` to the tray window.
  - File(s): `crates/panel/src/systray.rs:208`

## Medium

- [ ] **`build_window` is a ~630-line function** - It creates every widget and wires every signal of the Lister in one body, so it is hard to read and cannot be tested. Split it into `build_toolbar`, `build_file_view`, `wire_selection`, `wire_keys`, `wire_dnd` and so on, the way `actions::attach` already does for the button row.
  - File(s): `crates/lister/src/build.rs:97`

- [ ] **The `Ui` struct is still a god object** - About 30 fields: caches, format state, typeahead, output windows, remote sessions and history all sit next to each other, and every module reaches into `Ui` directly. Keep going with the split that `ViewState`/`StatusBar`/`NavHistory` started: a `FormatState` (`format`, `temp_format`), a `Caches` (`dir_cache`, `size_cache`), a `Typeahead`, with methods on each.
  - File(s): `crates/lister/src/window.rs:135`

- [ ] **The "send to every registered Lister" loop is repeated ~9 times** - `for c in self.conns.values_mut().filter(|c| c.registered) { let _ = c.tx.send(..) }` appears in the settings, bindings, formats, invalidate, free-caches, hidden and shutdown handlers. `apply_role` also builds the same `RoleChanged` message in two places. Add `Hub::broadcast(msg)` and a `roles_msg()` helper. Wrapping `Bindings`/`FormatStore` in `Arc` would also stop them being deep-cloned once per Lister.
  - File(s): `crates/hub/src/server.rs`

- [ ] **Job submission is written twice** - `HubCmd::RunJob` and `Hub::lister_run_job` both validate, compute `src_parents`, build a `JobSpec`, call `engine.submit` and insert a `JobMeta`. The hub path uses the `u64::MAX` sentinel as its requester. Pull this into one `submit_job(requester: Option<u64>, ..)`.
  - File(s): `crates/hub/src/server.rs` (`gtk_cmd`, `lister_run_job`)

- [ ] **`run_internal` has seven `unreachable!` arms** - The function step commands are dispatched partly in `run_steps` and partly in `run_internal`, so adding a new `InternalCmd` can panic at runtime instead of failing to compile. Do all the dispatch in one exhaustive `match` in `run_steps` and give `run_internal` only the file-operation commands.
  - File(s): `crates/hub/src/functions.rs:346`

- [ ] **Icon thumbnails go stale after a file is edited** - The in-memory `thumb_cache` is keyed by index and carried over by path in `refresh_entries`. `IconEntry` has no mtime or size, so an image that was overwritten keeps its old thumbnail until the folder is reopened. Add `modified` to `IconEntry` and key the cache by `(path, mtime)`. That also removes the index re-keying of four maps on every refresh.
  - File(s): `crates/iconview/src/field.rs:227`, `crates/iconview/src/types.rs:64`

- [ ] **Every auto-refresh replaces the whole list** - `apply_navigation` always does `store.splice(0, old_len, ..)` and sets the cursor back to row 0, even when only one file changed. So each inotify event rebinds every row, loses the scroll position and moves the keyboard cursor. When the folder is the same, compare by name and splice only the rows that changed, keeping the cursor on its row.
  - File(s): `crates/lister/src/window.rs` (`apply_navigation`), `crates/lister/src/nav_cache.rs` (`finish_listing`)

## Low / Nice to have

- [ ] **Two logging styles** - 46 `eprintln!` calls sit next to the `err_log!`/`debug_log!` macros (for example `settings::read_json`), so some errors skip the log file and its formatting. Use the macros everywhere outside tests.
  - File(s): `crates/ipc/src/settings.rs` and others (`grep -rn eprintln! crates`)

- [ ] **The same metadata-to-row code appears three times in `read_dir_rows`** - The mtime/uid/gid/mode extraction is written out for the symlink target, the broken link and the plain entry. Use one `row_fields(&Metadata)` helper.
  - File(s): `crates/lister/src/model.rs:93`

- [ ] **Window placement retry delays are spread out** - The `[80, 400, 1000]` retry lists are written inline in the panel and the dock, while the hub uses named constants for the same idea. Share one constant and a `retry_after_map(delays, f)` helper (ideally next to the shared X11 helper above).
  - File(s): `crates/panel/src/lib.rs:122`, `crates/dock/src/lib.rs:104`, `crates/hub/src/toolbar.rs:232`, `crates/hub/src/desktop_windows.rs:70`

- [ ] **A sentinel requester id** - `HUB_REQUESTER = u64::MAX` stands for "no Lister" in `JobMeta.requester`, so each routing site has to remember to treat it specially. Use `Option<u64>`.
  - File(s): `crates/hub/src/server.rs`
