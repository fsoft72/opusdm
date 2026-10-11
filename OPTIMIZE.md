# Optimization TODO

> Generated on 2026-10-11. Items sorted by importance.

## Critical

- [x] **Archive file open blocks the GTK thread** - `activate_row` calls `extract_single_file_temp` (spawns `unzip`/`tar`/`7z` and waits on `.output()`) directly on the main loop, so opening a file inside a large or compressed archive freezes the whole Lister; move it to a worker thread with an `async_channel` like `navigate_from` already does for listings.
  - File(s): `crates/lister/src/window.rs`, `crates/lister/src/archive.rs`

## High

- [x] **Shared `apply_theme()` copied into 9 binaries** - player, runner, shot, viewer, apps-editor, input-config, screen, terminal and polkit each repeat the same "load settings, apply theme mode, build CssProvider from `chrome_css` + `ui_look_css` + local CSS" body; add `opusdm_ui::install_app_css(app_id, extra_css)` so a theming change lands in one place.
  - File(s): `crates/{player,runner,shot,viewer,apps-editor,input-config,screen,terminal,polkit}/src/main.rs`, `crates/ui/src/`

- [x] **`x11_window_id` duplicated 5 times** - the same surface -> `X11Surface` -> xid downcast lives in hub, panel, dock, runner and polkit; move it into `opusdm_ui` (next to `with_x11`), taking `&impl IsA<gtk::Native>` so both `Window` and `ApplicationWindow` callers use it.
  - File(s): `crates/hub/src/desktop.rs`, `crates/panel/src/window.rs`, `crates/dock/src/window.rs`, `crates/runner/src/ui.rs`, `crates/polkit/src/dialog.rs`

- [x] **`Hub` is a 40+ field god struct** - connections, roles, settings, bindings, jobs, confirms, output windows and five function-run maps (`pending_functions`, `selection_waiters`, `function_requesters`, `function_cancels`, `next_run_id`) all sit on one struct; group the function-run state into a `FunctionRuns` type (owned by `server/function_run.rs`) and jobs/replies/conflicts into a `Jobs` type so each can be tested without building a full hub.
  - Done: `FunctionRuns` (`crates/hub/src/server/function_runs.rs`, 5 unit tests). Not done: the `Jobs` grouping (jobs, replies, conflict owners, confirms share `next_req` with function prompts; left for a later pass).
  - File(s): `crates/hub/src/server.rs`, `crates/hub/src/server/function_run.rs`

- [x] **Two separate image viewers** - the Lister's "Show" mode (picture display, prev/next, mark-for-delete) overlaps with the standalone `opusdm-viewer` crate (gallery, zoom, slideshow, trash-first delete); decide which one owns image viewing and make the other delegate, otherwise every image feature has to be built and fixed twice.
  - File(s): `crates/lister/src/viewer.rs`, `crates/lister/src/slideshow.rs`, `crates/lister/src/viewer_mode.rs`, `crates/viewer/src/`

## Medium

- [x] **Archive listing re-runs the external tool on every navigation** - `navigate_from` skips the dir cache for archive paths, so each step into a sub-folder of an archive re-runs `zipinfo`/`tar -t`/`7z l` over the whole file; cache the parsed `Vec<ArchiveItem>` keyed by archive path + mtime and filter per prefix.
  - File(s): `crates/lister/src/nav_cache.rs`, `crates/lister/src/archive.rs`

- [x] **Lister hardcodes `7z` while fsops resolves the binary** - `fsops::compress` uses `seven_z_tool()` (7z / 7za / 7zz) but `lister/src/archive.rs` calls `Command::new("7z")` in three places, so systems with only `7zz` can create 7z archives but not browse or extract them; reuse the fsops resolver.
  - File(s): `crates/lister/src/archive.rs`, `crates/fsops/src/compress.rs`

- [x] **Archive preview ignores the tool exit status** - `archive_listing` returns stdout even when `unzip -l` / `tar -tf` fails, so a corrupt archive shows an empty preview instead of the error; check `status.success()` and show stderr.
  - File(s): `crates/lister/src/preview.rs`

- [ ] **PDF `RenderCache` rebuilt for every page** - `render_page` creates a fresh `RenderCache` on each call although the page worker already keeps the document open; keep the cache next to the open `Pdf` in the worker so fonts and images are not re-decoded on every page turn.
  - File(s): `crates/viewer/src/pdf.rs`, `crates/viewer/src/pdf_pages.rs`

- [ ] **Lister editors read config straight from disk** - the Function, Hotkey and Filetype editors call `load_functions()` / `load_bindings()` themselves while the hub holds the authoritative `current_functions` / `current_bindings`; an editor opened right after another Lister saves (persist thread still writing) can start from stale data. Fetch the current copy from the hub instead.
  - File(s): `crates/lister/src/function_editor.rs`, `crates/lister/src/hotkey_editor.rs`, `crates/lister/src/filetypes_ui.rs`

- [ ] **`layer_spec` rebuilt in 6 places** - hub, panel (x2), dock, runner and polkit each assemble a `LayerSpec` by hand; add a small set of constructors (`LayerSpec::desktop(rect)`, `::bar(place)`, `::overlay()`) in `opusdm_ui::layer` so layer, anchor and keyboard-mode choices stay consistent.
  - File(s): `crates/hub/src/desktop.rs`, `crates/panel/src/window.rs`, `crates/panel/src/device_popup.rs`, `crates/dock/src/window.rs`, `crates/runner/src/ui.rs`, `crates/polkit/src/dialog.rs`

- [ ] **Very long UI builder functions** - `theme-config::build_window` (366 lines), `lister::show_context_menu` (249), `lister::actions::attach` (162), `theme-config::editor::open` (165) and `hub::toolbar::apply_event` (144) mix widget creation, signal wiring and business logic; split them into per-section builders like `lister/src/build.rs` already does with its `*Widgets` structs.
  - File(s): `crates/theme-config/src/main.rs`, `crates/lister/src/context_menu.rs`, `crates/lister/src/actions.rs`, `crates/theme-config/src/editor/mod.rs`, `crates/hub/src/toolbar.rs`

- [ ] **Three size formatters with different rules** - `lister::model::format_size_as` (1024 base, "1.5K"), `devices::model::human_size` (1000 base, "1.5 GB") plus ad hoc math in `lister/src/gauge.rs` and `panel/src/widgets/sysmon.rs`; move one formatter with a `SizeFormat` argument into `opusdm_ipc` so the panel, devices popup and Lister agree.
  - File(s): `crates/lister/src/model.rs`, `crates/devices/src/model.rs`, `crates/lister/src/gauge.rs`, `crates/panel/src/widgets/sysmon.rs`

## Low / Nice to have

- [ ] **Identical forwarder threads in `owner_loop`** - the job-progress and function-event forwarders are the same 15-line "recv, wrap in `OwnerMsg`, send" thread; extract `spawn_forwarder(name, rx, wrap)` to shorten `owner_loop` (122 lines).
  - File(s): `crates/hub/src/server.rs`

- [ ] **Repeated "send to connection" boilerplate** - `lister_msg` and friends repeat `if let Some(c) = self.conns.get_mut(&cid) { let _ = c.tx.send(...) }`; a `Hub::send_to(cid, msg)` helper would cut noise and give one spot to log dead connections.
  - File(s): `crates/hub/src/server.rs`

- [ ] **`eprintln!` bypassing `err_log!`** - a few non-example call sites print directly (`hotkey_editor.rs`, `filetypes_ui.rs`, `network/src/watch.rs`, `terminal/src/main.rs` outside the usage message), skipping the shared `[tag:pid]` prefix the rest of the workspace uses.
  - File(s): `crates/lister/src/hotkey_editor.rs`, `crates/lister/src/filetypes_ui.rs`, `crates/network/src/watch.rs`

- [ ] **Hardcoded accent colors outside the theme** - `#3584e4` and other literals in `viewer/src/main.rs`, `lister/src/viewer.rs`, `shot/src/editor.rs` and `hub/src/desktop_windows.rs` ignore the theme accent; use the theme's CSS variables (`@accent_color` / chrome tokens) so dark and custom themes match.
  - File(s): `crates/viewer/src/main.rs`, `crates/lister/src/viewer.rs`, `crates/shot/src/editor.rs`, `crates/hub/src/desktop_windows.rs`

- [ ] **419-line single GTK test** - `iconview::field::tests::gtk_widget_behavior` packs a dozen unrelated scenarios into one test, so the first failure hides the rest; split per scenario behind a shared `gtk_ready()` guard.
  - File(s): `crates/iconview/src/field.rs`

- [ ] **Duplicated `show_message` / `show_error` in panel centers** - audio, devices, network and battery centers each implement the same status-message helper; pull it into a shared center base widget in `crates/panel/src/`.
  - File(s): `crates/panel/src/audio_center.rs`, `crates/panel/src/devices_center.rs`, `crates/panel/src/network_center.rs`, `crates/panel/src/battery_center.rs`
