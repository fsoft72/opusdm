# Optimization TODO

> Generated on 2026-10-17. Items sorted by importance.
>
> Second review round. The items of the 2026-10-16 round are all fixed (see CHANGES.md) and are not repeated here.
>
> Status 2026-10-17: every item is fixed on master (see CHANGES.md). Leftovers:
> - Brightness: the item's example was wrong (`acpi_video0` reports `firmware`, which the kernel ranking prefers over `raw` ones like `intel_backlight`); backlights are now ranked by `type`, but that exact laptop case is unchanged.
> - Scripts: JS run from a native callback (e.g. inside `forEach`) or after an `await` is not budgeted by Boa and still stops only at its next `opus.*` call.
> - The hub's `watch_screen_size` callback still re-reads GDK: it only gets the primary size and the desktop needs all monitor rects.
> - `fsops/src/trash.rs` keeps its own percent-encoding (trashinfo `Path`, not a file URI).

## Critical

- [x] **Deleting or moving an SFTP symlink to a folder empties the folder it points to** - `SftpFs::list` sets `is_dir` from the link target, and `resolve_items` keeps `is_dir` but drops `is_symlink`. The top-level `remove_item` then lists through the link and deletes every file in the target before `rmdir` fails on the link. This hits Remove and every Move with `delete_source` (download, upload, relay). The symlink guard only covers children. Fix: carry `is_symlink` in `RemoteItem` and always remove a link with `remove_file`.
  - File(s): `crates/ftp/src/sftp_backend.rs:90-96`, `crates/ftp/src/transfer.rs:256-270,291`, `crates/lister/src/remote_xfer.rs:109,131,153,160`

## High

- [x] **Copy and Move treat FIFOs, sockets and device files as regular files** - `scan_with` puts anything that is not a folder or symlink into `Item::File`. `File::open` on a FIFO blocks forever waiting for a writer, cancel cannot stop it, and the job (and its Move) hangs. `/dev/zero` is copied without end. Fix: plan only `ft.is_file()` as files, and recreate FIFOs with `mkfifo` or report special files as skipped (never delete them in a Move).
  - File(s): `crates/fsops/src/scan.rs:80-88`, `crates/fsops/src/copy.rs:76`

- [x] **Archive scratch folder is deleted by an unrelated job** - `on_job_done` calls `scratch_job_done()` for every `JobDone` and ignores the job id, and `scratch_job_done` always counts down the oldest folder. A quick delete finishing during a big copy out of an archive deletes the extracted folder mid-copy. Fix: key `SCRATCH` by the job ids returned by `submit_job` and release only on those ids.
  - File(s): `crates/lister/src/actions.rs:257-262,283-292`, `crates/lister/src/archive.rs:250-267`

- [x] **`percent_decode` panics on clipboard and drop text, and mangles non-UTF-8 names** - `&s[i + 1..i + 3]` slices a `&str` by byte index, so text like `file:///tmp/a%1é` cuts into a multi-byte char and panics inside a GTK callback. `from_utf8_lossy` also corrupts Latin-1 names (`%E9`), and `file://localhost/...` becomes a relative path. Fix: decode hex from the byte slice, build the path with `OsString::from_vec`, strip an optional host, and use one shared decoder.
  - File(s): `crates/iconview/src/dnd.rs:38-77`, `crates/lister/src/clip.rs:49`, `crates/hub/src/desktop_ops.rs:60`

- [x] **Archive hardlink entries can rewrite a file already in the destination** - `extract` only checks that a hardlink target is a safe relative path. A crafted tar with `evil` as a hardlink to an existing `notes.txt` makes libarchive link and then write into the user's file, breaking the "never overwrite" rule. Fix: track names written by this extraction and refuse hardlinks whose target is not in that set (add a regression test with a crafted tar).
  - File(s): `crates/archive/src/extract.rs:93-108`, `crates/archive/src/writer.rs:54-57`

- [x] **Cancelling a queued job hangs its Function step forever and leaks `JobMeta`** - `JobEngine::cancel` removes a queued job without emitting `Progress::Done`, so the hub never removes its `JobMeta` and the `waiter` Sender stays alive, while `run_file_job` blocks on a plain `done_rx.recv()`. Closing a Lister with more than `MAX_CONCURRENT` jobs started from Functions leaves executor threads blocked for good. Fix: have `cancel()` report a synthetic `Done { cancelled: true }`, wait with `recv_or_cancel`, and add a test with the fake `Runner`.
  - File(s): `crates/hub/src/jobs.rs:156-162`, `crates/hub/src/server/function_run.rs:429-437`, `crates/hub/src/server.rs:1078`, `crates/hub/src/functions.rs:678-684`

- [x] **Thumbnail subprocess deadlocks on a full stdout pipe** - `run_with_timeout` polls `try_wait()` and reads stdout only after exit, so a PNG over the 64 KiB pipe buffer blocks the generator, which is killed after the 20 s `TIMEOUT`. Large video/PDF thumbnails fail and each one holds one of the 4 workers for 20 s. Fix: drain stdout on a helper thread (as done for tar/zip stderr), or use `wait_with_output` with a kill timer.
  - File(s): `crates/iconview/src/thumbs/generate.rs:149-178`

- [x] **polkit agent respawns its helper forever when it fails immediately** - `HelperEvent::Failure` always calls `start_run()` again, even when no password prompt was shown. When `polkit-agent-helper-1` exits at once (not setuid, stale cookie, PAM error) the dialog spawns a process and reader thread in a tight loop on the GTK thread until the user cancels. Fix: restart only after a run that showed a prompt, otherwise show the error, with a retry limit.
  - File(s): `crates/polkit/src/dialog.rs:183-206,219-224`

## Medium

- [x] **Thumbnails are re-queued on every scroll event and failures retry forever** - Each `value_changed` runs `sync_realized` -> `request_missing_thumbs`, which re-submits every wanted index not in `thumb_cache` with no in-flight tracking. A fast scroll fills the unbounded channel with duplicates and a corrupt video re-runs ffmpeg on every scroll. Fix: keep `pending` and `failed` sets per generation and have the pool report failures.
  - File(s): `crates/iconview/src/field.rs:219-221,344-371`, `crates/iconview/src/field/virtualization.rs:204`, `crates/iconview/src/thumbs/pool.rs:75-100`

- [x] **Thumbnail cache temp file is shared by all workers and store errors are dropped** - The temp name `{hash}.tmp{pid}` is the same for all 4 threads, so duplicate requests (see above) race on write and `rename` and can leave a corrupt entry; `let _ = cache::store(...)` hides it. Fix: `tempfile::NamedTempFile::new_in(dir)` and log store failures.
  - File(s): `crates/iconview/src/thumbs/cache.rs:76-83`, `crates/iconview/src/thumbs/pool.rs:79`

- [x] **Texture cache in `IconField` grows without limit** - `thumb_cache: HashMap<usize, Texture>` keeps every decoded texture until the folder changes, about 256 KB each, so scrolling a 20k-photo folder reaches several GB. Fix: cap it as an LRU or drop textures far outside `wanted_indices`; the PNGs stay on disk.
  - File(s): `crates/iconview/src/field.rs:56,408-410`

- [x] **Cross-filesystem Move deletes files that appeared after the scan** - With no filter and nothing skipped, the Move ends with `remove_any(src)` on the whole tree, so a file created in the source during the copy is deleted without being copied. Fix: always delete through the existing `remove_copied`.
  - File(s): `crates/fsops/src/copy.rs:660-670`

- [x] **Folder mode and mtime are lost on Copy and cross-filesystem Move** - `Item::Dir` uses `create_dir_all` with the default mode, so a private `0700` folder becomes `0755` (readable by others) after a Move to another disk, and folder mtimes become "now". Fix: after `run_plan`, apply each folder's mode and mtime deepest first.
  - File(s): `crates/fsops/src/copy.rs:499-503`

- [x] **Delete retry chmods the wrong file and a big delete cannot be cancelled** - On `remove_file` failure the entry itself is made writable, which never helps (the parent controls deletion), and `set_permissions` follows symlinks so a link target outside the tree is changed and left changed. `remove_recursive` has no cancel token, so one huge tree cannot be stopped. Fix: drop the retry (or never on symlinks) and pass the `CancelToken` down.
  - File(s): `crates/fsops/src/delete.rs:18-37,102-157`

- [x] **"Never overwrite" is check-then-rename in Encrypt, Trash and Restore** - Each checks that the target is free and then calls `fs::rename`, which replaces anything created in between. Fix: use the existing atomic `rename::rename_noreplace`.
  - File(s): `crates/fsops/src/crypt.rs:179,193`, `crates/fsops/src/trash.rs:166-170`, `crates/fsops/src/trash_items.rs:127-132`

- [x] **Skipped entries are hidden, and Upload-Move then deletes them** - Encrypt ignores folder walk errors and reports success; Upload drops `read_dir` errors and silently skips symlinks and special files, and an Upload-Move then runs `remove_dir_all` on the local folder. A cross-disk Restore treats a dropped reply as Cancel but ignores `cancelled`, so a partial restore removes its `.trashinfo`. Fix: report these as errors or skips and refuse the source deletion when anything was skipped.
  - File(s): `crates/fsops/src/crypt.rs:224`, `crates/ftp/src/transfer.rs:213-223`, `crates/lister/src/remote_xfer.rs:139-141`, `crates/fsops/src/trash_items.rs:150-157`

- [x] **Hub blocks the GTK main thread on quit, watch teardown, saves and compositor reload** - `request_quit` waits up to 11 s with `recv_timeout` inside the dialog callback; `DesktopWatch::drop` joins a thread that polls every 500 ms; `save_leftout` / `save_group` fsync on every icon drag; `compositor::apply` spawns and waits for `pkill` on every transparency change. Fix: await quit through `async_channel` + `spawn_future_local`, detach or eventfd-wake the watcher, and move saves and `compositor::apply` to a worker.
  - File(s): `crates/hub/src/toolbar.rs:290-305,389-391`, `crates/hub/src/desktop_files.rs:106-129`, `crates/hub/src/desktop_windows.rs:92,302-311,517`, `crates/hub/src/groups_ui.rs:12-16`, `crates/hub/src/compositor.rs:75-86`

- [x] **A script stuck in a pure-JS loop cannot be cancelled** - The Boa `Context` has no runtime limits and the cancel check runs only on `opus.*` calls, so `while (true) {}` pins a core until the hub exits and blocks its script event for 60 s. Fix: set `context.runtime_limits_mut()` (loop and recursion limits) or check the cancel token through Boa's interrupt support.
  - File(s): `crates/hub/src/scripting.rs:121-132,247-251`

- [x] **Settings saved right before Quit can be lost** - The persist thread is detached and nobody waits for its queue before `std::process::exit`, so a `SetSettings` / `SetFunctions` / `SetBindings` just before Quit is dropped and a `.tmp` may be left. Fix: keep the `JoinHandle`, drop the Sender and join with a timeout before acknowledging `ShutdownAndWait`.
  - File(s): `crates/hub/src/server/persist.rs:23-60`, `crates/hub/src/server.rs:1098-1108,1212,1295-1297`

- [x] **Panel adds a new `CssProvider` on every bar rebuild, and one screen change rebuilds several times** - `build_bar_window` calls `install_css()`, which adds a provider at APPLICATION priority each time and never removes it. `rebuild` never checks if the monitor rects changed, and `watch_monitors` plus `watch_screen_size` fire about 3 rebuilds per mode switch; the `watch_screen_size` callbacks ignore the `(w, h)` they get and re-read the GDK list that is stale under Xephyr. Fix: install CSS once, skip unchanged rects, coalesce in an idle source, and use the size from the callback.
  - File(s): `crates/panel/src/lib.rs:179,307-355,475-500`, `crates/ui/src/screen.rs:34-54,183-197`, `crates/dock/src/lib.rs:486-509`, `crates/hub/src/toolbar.rs:224-230`

- [x] **Window drags cause round-trip and connect storms (EWMH and Wayfire)** - `emit_geometries` re-queries every watched window with 2 blocking round trips on each collapsed ConfigureNotify (about 60 per event with 30 windows). On Wayfire, panel clicks call `wayfire::call` on the GTK thread with no read timeout (a stalled compositor freezes the panel), and the watcher opens 2 new connections for a full `State::read()` per event. Fix: read only the windows named in the events and pipeline requests; set socket timeouts or use a worker, and drain queued events before one re-read.
  - File(s): `crates/panel/src/ewmh.rs:254-266,310-340`, `crates/panel/src/wl_windows.rs:123-160`, `crates/wayland/src/wayfire.rs:78-80,210-279`

- [x] **shot "Save as" can overwrite a file without asking, on the GTK thread** - `resolve_target` adds the extension after the dialog's overwrite check, so typing `a` writes `a.webp` over an existing file with `fs::write`; encoding a large capture also blocks the UI. Fix: set the extension on the dialog's initial name or confirm the final path, and run `export::save` in `gio::spawn_blocking`.
  - File(s): `crates/shot/src/editor.rs:719-739`, `crates/shot/src/export.rs:128-131`

- [x] **Viewer ignores EXIF orientation and decodes Markdown images on the GTK thread** - `load_raw` / `load_thumb` never call `apply_embedded_orientation()`, so phone JPEGs show sideways. `markdown_view::_picture` decodes every image at full size with `Texture::from_filename` while building the text. Fix: apply orientation before `RawImage::from_pixbuf`, and load Markdown images scaled through `loader::load_thumb` on a worker.
  - File(s): `crates/viewer/src/loader.rs:24-33`, `crates/viewer/src/markdown_view.rs:119-137`

- [x] **Brightness picks the first backlight by name, not by type** - `Sysfs::find` takes the first folder by name with a non-zero `max_brightness`, so `acpi_video0` wins over `intel_backlight` / `amdgpu_bl0` on many laptops and the slider does nothing (and the DDC/xrandr fallbacks are never tried). Fix: read each `type` file and prefer `firmware`, then `platform`, then `raw`.
  - File(s): `crates/brightness/src/backend.rs:87-93`

## Low / Nice to have

- [x] **Hub accepts requests before `Register` and a repeated `Register`** - `lister_msg` handles `SetSettings`, `RunJob`, `LeaveOut` and more from unregistered connections, and a second `Register` re-sends `Welcome`, re-fires `ListerOpened` and uses a `visible_spawns_pending` slot. Fix: allow only `Register`, `Ping` and tool-window requests until registered, and ignore a repeat.
  - File(s): `crates/hub/src/server.rs:490-623,698-736`

- [x] **Small correctness gaps in shell and media apps** - `notify` restarts a replaced popup's timer while hovered (`popup.rs:104-116,264-285`); `audio::set_volume` flattens channel balance (`pulse.rs:224-240`); `apps-editor::write_entry` skips `sync_all`, leaves `.tmp` on error and drops unknown keys when the source cannot be read (`files.rs:23-49`); `parse_archive_path` splits on the first `#` (`lister/src/archive.rs:34-43`); `install_sigchld` drops the `Signals::new` error silently (`hub/src/children.rs:82-85`).
  - File(s): `crates/notify/src/popup.rs`, `crates/audio/src/pulse.rs`, `crates/apps-editor/src/files.rs`, `crates/lister/src/archive.rs`, `crates/hub/src/children.rs`

- [x] **Duplicated constants, helpers and per-keystroke work** - `PANEL_HEIGHT = 40` is defined in three crates and dock CSS colors are hard-coded; `file://` percent-encoding exists three times; output-window ids use an unnamed `run_id * 1000 + step_idx`; hub loads `desktop.json` / `settings.json` / `functions.json` in three places; the runner stats all of PATH on the GTK thread and lowercases every entry on each keystroke; `sort_dirs_first` allocates two lowercase strings per comparison. Fix: shared constants in `opusdm_ipc` / `opusdm_ui`, one URI helper, cached lowercase keys (`sort_by_cached_key`), and a single config load.
  - File(s): `crates/panel/src/window.rs:16`, `crates/runner/src/ui.rs:16-18`, `crates/dock/src/main.rs:8`, `crates/dock/src/lib.rs:521-534`, `crates/iconview/src/dnd.rs:48-59`, `crates/iconview/src/thumbs/cache.rs:14-25`, `crates/fsops/src/trash.rs:199`, `crates/hub/src/server/function_run.rs:305`, `crates/hub/src/main.rs:93`, `crates/runner/src/sources.rs:70-84`, `crates/runner/src/search.rs:45-96`, `crates/iconview/src/scan.rs:94-101`

- [x] **Failed inotify `add_watch` is dropped silently** - When watches run out (ENOSPC) live refresh stops with no log line. Fix: log the error and show a hint in the status bar.
  - File(s): `crates/lister/src/watcher.rs:52-54`
