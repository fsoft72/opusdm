# Optimization TODO

> Generated on 2026-10-07. Items sorted by importance.
>
> Overall the codebase is healthy: no critical problems found. Configuration is saved atomically (`opusdm_ipc::settings::save_json`), the hub state is split into handlers, and production `unwrap` calls are rare (the large counts in `server.rs`, `copy.rs` and others are almost all in test modules).

## Critical

None found.

## High

- [ ] **Verify the snap restore with a physical mouse** - the restore cancels Openbox's move loop by sending Escape through XTEST and starts a new move with `_NET_WM_MOVERESIZE`. It was checked only with an XTEST-driven button, which is a different input device from a real mouse, and an Escape sent after Openbox has already ended its loop would reach the focused application. Check it on the real session, and if it misbehaves switch to restoring the size at button release.
  - File(s): `crates/snap/src/x11.rs` (`Display::restore`)
- [ ] **Extract the shared X11 plumbing into one GTK-free crate** - seven places open their own connection with `x11rb::connect(None)` and three of them (`ui/x11.rs`, `panel/ewmh.rs`, `snap/x11.rs`) each implement their own `send_to_root`, atom interning and `_NET_MOVERESIZE_WINDOW` constants. A bug fix or a new EWMH helper has to be made three times. A small `opusdm-x11` crate (connection, atom cache, EWMH messages, frame extents) would be used by all of them; `opusdm-ui` cannot be reused by `opusdm-snap` because it pulls in GTK.
  - File(s): `crates/ui/src/x11.rs`, `crates/panel/src/ewmh.rs`, `crates/snap/src/x11.rs`, `crates/ui/src/screen.rs`, `crates/hub/src/global_keys.rs`, `crates/panel/src/systray.rs`, `crates/shot/src/capture.rs`

## Medium

- [ ] **Read the per-monitor work area from the docks' struts** - the snap area is the monitor intersected with `_NET_WORKAREA`, which is the union over all monitors, so a top panel on one monitor also shrinks the zones of the others. Reading `_NET_WM_STRUT_PARTIAL` of dock windows gives the real area of each monitor.
  - File(s): `crates/snap/src/x11.rs` (`Display::environment`, `workarea`)
- [ ] **Replace the snap polling with events** - the watcher wakes 20 times a second even when idle, and while a button is down it does five or six X round trips every 16 ms. Listening for button press and release with XInput2 raw events would wake it only during a drag, and the window-type and size-hint reads could be cached per window.
  - File(s): `crates/snap/src/watcher.rs`, `crates/snap/src/x11.rs`
- [ ] **Turn the live snap check into a repeatable test** - `watcher` and `overlay` have no automated test; the live check (Xephyr, Openbox, picom, XTEST drags) was done with throwaway examples. Keep it as an `#[ignore]` integration test plus a script that starts the nested server, so a regression in zones, preview or restore is caught.
  - File(s): `crates/snap/` (new `tests/live.rs`), `scripts/`
- [ ] **Share the watcher skeleton of the devices, audio and network crates** - the three `watch.rs` files each carry their own 30 s retry loop, 300 ms coalescing and `thread::sleep`, and the three `backend.rs` files repeat the same `Backend` plus `FakeBackend` shape. A small shared helper (retrying connect, coalescing burst drain) removes about 100 duplicated lines and keeps the three behaving the same.
  - File(s): `crates/devices/src/watch.rs`, `crates/audio/src/watch.rs`, `crates/network/src/watch.rs`
- [x] **Stop panicking on poisoned mutexes** - 6 `lock().unwrap()` in each of the devices and audio backends, 4 in network, 5 in `lister/archive.rs`, 2 in `panel/sni.rs` and 2 in `hub/global_keys.rs`: after any panic while a lock is held, every later call panics too. `opusdm-ui/src/x11.rs` already uses `unwrap_or_else(|e| e.into_inner())`; use the same everywhere.
  - File(s): `crates/devices/src/backend.rs`, `crates/audio/src/backend.rs`, `crates/network/src/backend.rs`, `crates/lister/src/archive.rs`, `crates/panel/src/sni.rs`, `crates/hub/src/global_keys.rs`
- [x] **Log the failures of cleanup after an error** - `let _ = remove_file(...)` / `remove_dir_all(...)` after a failed copy, compress, encrypt, split or archive operation hides a partial or temporary file that stays on disk. Log the failed cleanup with the path so it can be found.
  - File(s): `crates/fsops/src/crypt.rs:194,198`, `crates/fsops/src/compress.rs:175-187`, `crates/fsops/src/copy.rs:95,468,630`, `crates/fsops/src/joinsplit.rs:131,211`, `crates/lister/src/archive.rs:366`, `crates/lister/src/remote.rs:85`
- [ ] **Split the very long GTK builder functions** - `build_desktop_menu` (272 lines), `open_rules_dialog` (257), `add_column` (239), `open_compare_dialog` (184) and `open_command_palette` (178) build, wire and handle everything in one function, which makes them hard to read and impossible to test. Extract the sections (rows, buttons, handlers) into small functions or widgets.
  - File(s): `crates/hub/src/desktop_menu.rs`, `crates/lister/src/batch_rename.rs`, `crates/lister/src/columns.rs`, `crates/lister/src/tools.rs`, `crates/lister/src/palette.rs`
- [ ] **Move the inline tests out of the biggest files and break up `lister_msg`** - `hub/src/server.rs` is 2920 lines of which about 1600 are tests, and `Hub::lister_msg` is a 206-line match. Moving the tests to `server/tests.rs` (the module already has a `server/` folder) and extracting one handler per `ToHub` variant, as `apply_settings` already does, makes both easier to navigate. `functions.rs` (1410 lines) has the same shape.
  - File(s): `crates/hub/src/server.rs`, `crates/hub/src/functions.rs`
- [ ] **Clear the clippy warnings so `-D warnings` can pass** - `cargo clippy --workspace` reports about 40 warnings, and `-D warnings` already fails on 4 errors in `opusdm-ipc` alone: 13 very complex types (for example `Rc<dyn Fn(Vec<(i32, i32, i32, i32)>)>` in `ui/screen.rs`, which wants a `MonitorRect` alias), 7 large enum variants to box (`ToHub` and `ToLister` in `ipc/proto.rs:150,443`, `lister/client.rs:54`, `hub/server/persist.rs:10`, and `HubEvent`, `OwnerMsg`, `OutMsg` in `hub/server.rs:39,228,257`), 5 functions with too many arguments (`lister/build.rs:21`, `lister/model.rs:43`, `panel/lib.rs:77`, `hub/functions.rs:686`, `hub/desktop_windows.rs:121`) and small ones like `manual_is_multiple_of` in `ipc/filetypes.rs:199`.
  - File(s): `crates/ipc/src/proto.rs`, `crates/hub/src/server.rs`, `crates/hub/src/server/persist.rs`, `crates/lister/src/client.rs`, `crates/ui/src/screen.rs`, `crates/lister/src/build.rs`, `crates/lister/src/model.rs`, `crates/panel/src/lib.rs`, `crates/hub/src/functions.rs`, `crates/hub/src/desktop_windows.rs`, `crates/ipc/src/filetypes.rs`

## Low / Nice to have

- [ ] **Replace the remaining production `unwrap` and `expect` calls with errors** - `lister/columns.rs` has 9, `lister/archive.rs` 5 and `hub/server.rs` 5 outside tests; each should fail with a message or fall back instead of panicking the UI or the hub.
  - File(s): `crates/lister/src/columns.rs`, `crates/lister/src/archive.rs`, `crates/hub/src/server.rs`
- [ ] **Use one version for the whole workspace** - the version is repeated in 25 `Cargo.toml` files plus `opusdm_ui::VERSION`, and every release means editing all of them by hand (as the 0.7.0 bump did). Put it in `[workspace.package]` with `version.workspace = true` and build `VERSION` from `env!("CARGO_PKG_VERSION")`.
  - File(s): `Cargo.toml`, `crates/*/Cargo.toml`, `crates/ui/src/version.rs`
- [ ] **Finish moving the JSON files onto `load_json`** - 11 callers already use `opusdm_ipc::settings::load_json`, but `ipc/menu.rs:50` and a few others still parse by hand, and the 35 `load_*` functions each repeat the path plus load plus save trio. A small `ConfigFile<T>` helper would give one place for the error messages.
  - File(s): `crates/ipc/src/menu.rs`, `crates/ipc/src/bookmarks.rs`, `crates/ipc/src/quickapps.rs`, `crates/ipc/src/filetypes.rs`, `crates/ipc/src/format.rs`
- [ ] **Remove the dead code behind `#[allow(dead_code)]`** - the allow hides an unused item that should be deleted or used.
  - File(s): `crates/lister/src/tools.rs:269`
- [ ] **Keep the snap watcher thread responsive during a restore** - `Display::restore` sleeps 120 ms on the watcher thread while Openbox cancels the move, so the preview and state machine freeze for that time. Making the restore a step of the state machine (cancel, wait for the next samples, then resize) removes the sleep. Also, `watcher::apply` takes 7 parameters and would read better with a small context struct.
  - File(s): `crates/snap/src/x11.rs`, `crates/snap/src/watcher.rs`
- [ ] **Round the corners of the snap preview** - the preview is a plain rectangle; KDE's has rounded corners. The X Shape extension (already used for the click-through region) can clip the bounding region, at the cost of one mask per size change.
  - File(s): `crates/snap/src/overlay.rs`
