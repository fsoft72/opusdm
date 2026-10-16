# Optimization TODO

> Generated on 2026-10-16. Items sorted by importance.
>
> Status 2026-10-16: every item is fixed on master (see CHANGES.md). Open leftovers are listed under the item they belong to.

## Critical

- [x] **Cross-filesystem Move deletes the source files the user chose to Skip** - `is_partial()` only looks at the update/newer/glob options, so a Move that skips files on conflict still runs `remove_any(src)` on the whole tree. The skipped files are copied nowhere and then deleted. Fix: set `partial_move` whenever `shared.skipped > 0` (or always track copied sources when `delete_src_after`) and use `remove_copied`. Apply the same fix to the cross-fs branch of `moveop.rs`.
  - File(s): `crates/fsops/src/copy.rs:571,597-605`, `crates/fsops/src/types.rs:45`, `crates/fsops/src/moveop.rs:96-102`

## High

- [x] **Symlink entries replace whatever is at the destination without asking** - `Item::Symlink` runs `let _ = remove_file(&link)` with no conflict prompt, so an existing regular file with the same name is deleted. This skips Skip/Overwrite and `skip_all`. Fix: send symlinks through `decide()` like files, then create the link under a temp name and `rename` it into place.
  - File(s): `crates/fsops/src/copy.rs:462-473`

- [x] **Copy and Move never fsync before deleting the source** - `copy_file` renames the temp file into place without `sync_all()`, and a Move deletes the source right after. After a power loss on XFS/btrfs the destination can be empty while the source is already gone. Fix: `writer.sync_all()` before the rename, and fsync each destination directory before `remove_any` / `remove_copied`.
  - File(s): `crates/fsops/src/copy.rs:82-92,597-607`

- [x] **FTP/SFTP download destroys an existing local file when it fails** - `download_file` truncates the target with `File::create` and removes it on any error. A failed re-download therefore deletes the user's existing copy. Cancel is reported as a generic `Failed("cancelled")`. Fix: download to a temp sibling with `create_new`, rename it on success, and add a `RemoteError::Cancelled` variant.
  - File(s): `crates/ftp/src/transfer.rs:59-76,135`

- [x] **Hub accept loop stops for good on the first accept error** - `Err(_) => break` ends the `opusdm-accept` thread on any error, even a temporary one like EMFILE or ECONNABORTED, and logs nothing. The hub keeps running but no client can connect again. Fix: log the error, `continue` on temporary errors (with a short back-off for EMFILE), and exit only on a fatal one.
  - File(s): `crates/hub/src/server.rs:192-200`

- [x] **Run steps that show an output window lose the exit status to the SIGCHLD reaper** - `run_run` calls `child.wait()`, but the hub reaper already collects children with `waitpid(-1)`. The wait can fail with ECHILD, so a successful wait+window step is reported as "exited with None". Fix: use `opusdm_ipc::reaped::wait`, as `run_external` already does, and remove the extra reaping thread.
  - File(s): `crates/hub/src/functions.rs:708-742`, `crates/hub/src/children.rs:94`

- [x] **Cancelling a run does not stop a step that is already running** - The cancel token is only checked between steps. External programs, waits, and script `confirm` / `alert` block with no timeout. A `tail -f` step or an unanswered dialog keeps the run alive forever, and `EventGuard` then drops every later firing of that script event. Fix: pass `&CancelToken` into the steps, poll the child and kill it on cancel, and use `recv_timeout` in a loop that checks for cancel.
  - File(s): `crates/hub/src/functions.rs:166,739-742,787-829`, `crates/hub/src/scripting.rs:249,257`, `crates/hub/src/server.rs:688`

- [x] **Choosing "Save" when closing network-config can lose the save** - `ask_before_leaving` sends `Job::Save` and closes the window straight away, so the app can exit while the worker is still saving, and a save error is never shown. `close_after_save` is never set to `true`, so the code meant to wait for the save never runs. Fix: set `close_after_save(true)` and let `Done::Saved` close the window.
  - File(s): `crates/network-config/src/window.rs:59,93,176-184,233-239,354-376`

- [x] **Closures hold strong `Rc` references that keep views and their workers alive** - `IconField` closures hold strong `field.clone()` references, so every closed lister view leaks its `Inner`, the thumbnail textures and up to 4 `thumb-N` threads. The crate has 0 uses of `Weak`. The same pattern keeps the lister `Ui` alive: `ui_slot`, 87 strong captures, and FTP/SFTP sessions and the watcher are never shut down. It also affects the viewer, player (GStreamer pipeline), network-config and theme-config editor loops. Fix: capture `Weak` in handlers on widgets the object owns, end loops when `upgrade()` fails, and add an explicit `shutdown()` on close.
  - File(s): `crates/iconview/src/field.rs:166-185`, `crates/iconview/src/field/controllers.rs`, `crates/iconview/src/thumbs/pool.rs:132-139`, `crates/lister/src/build.rs:741-746,915-949`, `crates/viewer/src/window.rs:156-184`, `crates/player/src/window.rs:62-67`, `crates/network-config/src/window.rs:67,100-115`, `crates/theme-config/src/editor/mod.rs:253-256`

- [x] **Blocking file I/O on the GTK main thread on every keystroke and click** - In the lister:
  - path completion lists and stats the whole folder on each keystroke;
  - `filetypes.json` is parsed again and the file's first bytes are read on each double and right click;
  - the bookmarks file is reloaded on each Enter;
  - the Sidecar stats every selected path on each selection change.

  The hub's desktop double click (`dblclick_for`) re-reads `filetypes.json` the same way. Fix: cache file types and bookmarks, refreshed through the existing inotify or `gio::FileMonitor`, use `DirEntry::file_type()`, and move path completion to a worker with a generation counter.
  - File(s): `crates/lister/src/pathcomplete.rs:21-39`, `crates/lister/src/filetypes_ui.rs:18-26`, `crates/lister/src/bookmarks_ui.rs:15`, `crates/lister/src/sidecar.rs:80-88,211`, `crates/hub/src/desktop_windows.rs:206-220`, `crates/ipc/src/filetypes.rs:343-345`

- [x] **Config and settings apps run external commands and big copies on the GTK thread** - These freeze the window while they run:
  - input-config runs `xinput` / `setxkbmap` / `xset` on startup and on each change, with sliders debounced only 30 ms;
  - theme-config Save copies the whole icons/fonts/sounds/wallpapers tree each time, and Import/Export zip and unzip in the dialog callback;
  - the screen tool runs xrandr/wlr-randr with no timeout, which also blocks the keep/revert countdown.

  Fix: run these through `gio::spawn_blocking` or a worker like network-config's `worker::spawn`, and make the buttons insensitive meanwhile.
  - File(s): `crates/input-config/src/backend.rs:203-226,284-293`, `crates/input-config/src/ui.rs:19,48-53`, `crates/theme-config/src/writer.rs:142-231,323-341`, `crates/theme-config/src/main.rs:406,432`, `crates/screen/src/ui.rs:516-539`, `crates/screen/src/backend.rs:54-86`

- [x] **Sysmon timer leaks each time the panel rebuilds its bars** - `sysmon::build` adds a 1 s timeout that always returns `Continue`, so each hot-plug or position change leaves another timer reading `/proc` forever. With the CPU popover open, it also scans every `/proc/*/stat` on the main thread every tick. Fix: add the same `root().is_none()` guard that `clock.rs` has, and move `read_processes()` to a worker.
  - File(s): `crates/panel/src/widgets/sysmon.rs:379-424`, `crates/panel/src/widgets/clock.rs:41-43`, `crates/panel/src/sysmon_model.rs:182-191`

## Medium

- [x] **Any window title change re-reads all windows and rebuilds the whole tasklist** - Each `_NET_WM_NAME` / `_NET_WM_STATE` change re-reads about 7 properties per window, including the full `_NET_WM_ICON`. `Tasklist::update` then recreates every button, menu and texture. Fix: re-read only `e.window`, cache icons per window id, and diff the tasklist by window id.
  - File(s): `crates/panel/src/ewmh.rs:228-234,291-355`, `crates/panel/src/widgets/tasklist.rs:39-61,171-195`, `crates/panel/src/lib.rs:443-471`

- [x] **Errors are swallowed and the UI reports success** - Examples:
  - hub non-test code has 136 `let _ =`, including persist sends that lose settings silently;
  - the lister ignores 25 hub `send` results, and `start_run` then spins forever if its send fails;
  - `terminal::config::save` returns `()`;
  - theme-config Apply says "Applied" after a `dock.json` write failure;
  - apps-editor clears `to_delete` even when a delete fails;
  - batch rename shows only "{n} error(s)" and can leave files under hidden `.opusdm-rename-*` names.

  Fix: `send_or_log` / `Ui::send_hub` helpers, `Result`-returning save functions, and show the error list to the user.
  - File(s): `crates/hub/src/server.rs:546,554`, `crates/lister/src/window.rs:664-668`, `crates/lister/src/job_submit.rs:66`, `crates/lister/src/batch_rename.rs:204-244`, `crates/terminal/src/config.rs:118-122`, `crates/theme-config/src/main.rs:373-378`, `crates/apps-editor/src/ui.rs:701-705`

- [x] **Hub IPC has unbounded queues and re-serializes every broadcast for each client** - Each connection's writer uses an unbounded channel, program output is sent one event per line with a cloned title, and `check_fit` starts a thread per request. `broadcast` clones the message and serializes it to JSON again for each Lister. Fix: `sync_channel(N)` and drop clients whose queue is full, batch output lines, serialize once into `Arc<[u8]>`, and use one fit-check worker.
  - Leftover: output of `External` steps (`run_external`, shown in the function log) is still sent one line per message; only `Run` steps are batched.
  - File(s): `crates/hub/src/server.rs:279-289,383-398,568-571,837,1331-1335`, `crates/hub/src/functions.rs:722-726,814-821`

- [x] **Compress and extract jobs can hang or leave corrupt files** - `wait_for` reads stderr only after the child exits, so `tar`/`zip` deadlocks once it prints more than about 64 KiB of warnings. A cancelled extraction leaves a truncated file, and later runs keep it because existing files are skipped. Fix: drain stderr on a helper thread, and remove the partial entry on cancel or error (or write to a temp name and rename).
  - File(s): `crates/fsops/src/compress.rs:249,276-293`, `crates/archive/src/extract.rs:96-108,120-128`

- [x] **RefCell borrows and unchecked indexing can panic at runtime** - The shot editor holds `state.borrow_mut()` while calling `im.focus_in/out()`, which can emit `commit` synchronously and borrow again. `IconSetSection::value` indexes `options[selected()]` with no check, which panics when no themes are found. iconview calls listeners while their list is still borrowed (14 sites). Fix: drop borrows before calling out, use `options.get(..)`, and copy the listener list before calling it.
  - File(s): `crates/shot/src/editor.rs:396-433,561-572`, `crates/theme-config/src/ui.rs:15-17,44-46`, `crates/iconview/src/field.rs:644-676`

- [x] **Shared helpers are duplicated across crates instead of living in a common one** - Examples:
  - `NetError` / `DeviceError` / `PowerError` are the same D-Bus error type, each with its own `proxy()` and 38 hand-written `map_err`;
  - the app icon lookup is written 3-4 times;
  - panel, runner and others depend on the dock crate just for `opusdm_dock::apps`;
  - X11 window type, strut and size hint setup is copied in panel, dock and notify.

  Fix: a shared `DbusError` with `From<zbus::Error>`, move `apps` to `opusdm-ui` (or `opusdm-apps`) with one `app_image()` helper, and add `set_type_and_states` / `set_fixed_geometry` to `opusdm-x11`.
  - File(s): `crates/network/src/error.rs:31`, `crates/devices/src/error.rs:23`, `crates/power/src/error.rs:24`, `crates/panel/src/widgets/tasklist.rs:199-205`, `crates/notify/src/popup.rs:417-437,497-516`, `crates/panel/src/window.rs:122-160`, `crates/dock/src/window.rs:91-130`

- [x] **Each config app rewrites the same UI boilerplate** - Repeated code:
  - `heading()` is defined 4 times and written inline 11 more;
  - an unsaved-changes guard is written by hand in 5 crates;
  - `while let Some(c) = first_child()` appears 12 times;
  - `FileDialog::builder` with its own filter setup appears 14 times.

  Fix: add `section_heading`, `clear_children`, `labeled_row`, `pick_file` and a `DirtyGuard` to `opusdm_ui`.
  - File(s): `crates/desktop-config/src/ui.rs`, `crates/terminal/src/prefs.rs`, `crates/apps-editor/src/ui.rs:107-129`, `crates/theme-config/src/editor/mod.rs:285-309,449-468`, `crates/input-config/src/ui.rs:363-367`

- [x] **Lister background work is hand-rolled 18 times, often with no cancel** - Each of the 18 places uses its own `thread::spawn` + `async_channel` + `spawn_future_local`. Navigation starts a new OS thread even when a newer one replaced it, and the checksum and dir-size workers keep running after their dialog closes. The child-reaper thread is copied 3 times. Fix: one `run_blocking(work, on_done)` helper on `gio::spawn_blocking` with a cancel token, plus one child-reap helper.
  - File(s): `crates/lister/src/nav_cache.rs:57`, `crates/lister/src/tools.rs:84,213`, `crates/lister/src/function_editor.rs:31-37`, `crates/lister/src/image_viewer.rs:30-36`, `crates/lister/src/sidecar.rs:52-58`

- [x] **Every navigation deep-copies the whole folder listing** - The nav cache clones the full `Vec<Row>` on a hit and again when storing, and each `Row` takes two global mutex locks and clones the user and group names. Watcher reloads repeat this for the folder already on screen. Fix: cache `Rc<[Rc<Row>]>` and intern user and group names as `Rc<str>`.
  - File(s): `crates/lister/src/nav_cache.rs:20,51,128`, `crates/lister/src/model.rs:32-38,57-61`

- [x] **God objects and long string-keyed dispatchers** - Examples:
  - lister `Ui` has 26 fields, `impl` blocks in 9 files, and a 125-line `apply_inbound`;
  - hub `function_event` mixes requests, actions and progress in one `FnEvent`;
  - `scripting::dispatch` is a 130-line match on op-name strings;
  - step parameters are read with 18 literal `args.get("...")` keys;
  - the function-id to steps logic exists twice and has already drifted.

  Fix: split `Ui` into focused parts, split `FnEvent` into `FnRequest` / `FnAction` / `FnProgress`, use an op table with a prelude coverage test, use `pub const` keys, and add one `resolve_steps` helper.
  - File(s): `crates/lister/src/window.rs:176-235,536-660`, `crates/hub/src/server/function_run.rs:103-124,172-289`, `crates/hub/src/server.rs:710-730`, `crates/hub/src/scripting.rs:214-343`, `crates/ipc/src/function.rs:195,250-300`

## Low / Nice to have

- [x] **`file://` URIs are built with `format!`** - Names containing `#`, `%`, `?` or non-UTF-8 bytes give broken URIs, and some launch errors are discarded with `let _ =`. Fix: one `open_default(&Path)` helper that uses `gio::File::for_path(p).uri()` and reports the error.
  - File(s): `crates/lister/src/window.rs:747,765-780`, `crates/hub/src/desktop_windows.rs:206-220`

- [x] **Alt+Left/Right are hard-coded ahead of user hotkeys** - `wire_keys` handles history before the user hotkey lookup, so a user binding on these keys never fires and the hotkey editor does not warn about it. Fix: add `Action::HistoryBack` / `HistoryForward` to `keys::resolve`.
  - File(s): `crates/lister/src/build.rs:771-786`, `crates/lister/src/hotkey_editor.rs:61-64`

- [x] **IPC has no protocol version, and the one-shot client is copied 5 times** - A client sending an unknown variant is dropped without a log line. Fix: add `PROTOCOL_VERSION` to `Register` / `Welcome`, log decode errors, and add `opusdm_ipc::sockpath::send_oneshot`.
  - File(s): `crates/ipc/src/proto.rs:156`, `crates/hub/src/server.rs:1275`, `crates/hub/src/remote.rs:43`, `crates/theme-config/src/model.rs:62,74`, `crates/terminal/src/prefs.rs:416`

- [x] **Slow backend round trips** - Examples:
  - SFTP/FTP use `io::copy`'s 8 KiB buffer and cannot be cancelled mid-file;
  - `NmBackend::state()` makes 1+5N D-Bus calls through short-lived caching proxies and polls `State` every 250 ms;
  - `ddcutil` re-detects displays (about 1 s) on every brightness get/set.

  Fix: a 256 KiB copy loop with cancel and progress, `CacheProperties::No` with one `GetAll` per object plus `StateChanged` signals, and cache the DDC bus number.
  - Leftover: no per-byte transfer progress, because the progress callback only takes a file path.
  - File(s): `crates/ftp/src/sftp_backend.rs:106-114`, `crates/ftp/src/ftp_backend.rs:105-117`, `crates/network/src/nm.rs:81-206,267-293`, `crates/brightness/src/backend.rs:152-164`

- [x] **Panel strut uses a screen height captured once at startup** - `X11::screen_height` is read once in `shared()`, so after an xrandr resize the bottom panel reserves the wrong strip. Fix: read the root geometry when needed, or pass the height from the GDK monitor rectangles.
  - File(s): `crates/x11/src/lib.rs:116,147,165-167`, `crates/panel/src/window.rs:128`
