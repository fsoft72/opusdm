# Changes

## 2026-09-01

- Lister layout redesign: top stat bar (D/F/B counts + SOURCE/DEST/lock), button row, file list (Name/Size/Date/User/Group/Access), bottom navigation (home/parent/path).
- model.rs: Row extended with uid, gid, mode fields; format_user/format_group/format_access functions added.
- window.rs: rebuild_window refactored for new layout; add_column expanded for new column types (User, Group, Access with monospace font).
- update_status() now computes directory/file/byte counts (D/F/B format).
- Role-based stat bar coloring: SOURCE=green, DEST=light red; CSS classes toggle in update_status().
- Hub settings: new settings.rs module (load/save ~/.config/opusdm/settings.json); Settings button opens dialog with SOURCE/DEST color picker (preview box clickable for ColorDialog, hex input).
- IPC: ToLister::Welcome extended with source_color/dest_color fields; new ColorUpdate message to push color changes to connected Listers.
- Server: HubCmd::UpdateColors broadcasts ColorUpdate to all registered Listers when settings saved.
- Lister: receives colors from Welcome + ColorUpdate; applies them dynamically via CSS; update_css_for_colors() generates role-based CSS at runtime.
- Settings model restructured for growth: `opusdm_ipc::Settings { colors: ColorSettings }` is now the single struct used as the wire format (Welcome/SettingsUpdate), the on-disk JSON shape, and the hub's in-memory state - one place to extend when a new setting is added, no more per-field IPC messages. `ColorUpdate` renamed `SettingsUpdate`, carries the whole `Settings`. `HubCmd::UpdateColors` renamed `UpdateSettings`.
- ColorSettings expanded to 4 categories x background+text: source_bg/fg, dest_bg/fg, dir_bg/fg, file_bg/fg. Directory/file rows in the Lister's file list now get their own CSS classes (row-dir/row-file) applied per cell in every column.
- Hub settings dialog: 4 sections (SOURCE/DEST/DIRECTORY/FILE), each with Background + Text color rows; scrollable.

## 2026-08-31

- Task 1: workspace scaffold, opusdm-ipc frame module (length-prefixed JSON frames).

- Task 2: opusdm-ipc protocol enums (ToHub/ToLister), socket path resolution, probe_live_hub.
- Task 3: opusdm-fsops CancelToken, JobSpec/Progress, scan().
- Task 4: opusdm-fsops copy_tree (mode/mtime preserved, conflict + cancel), run_job.
- Task 5: opusdm-fsops run_move, run_delete, make_entry. fsops crate complete.
- Task 6: opusdm-hub crate skeleton, apply_role_event() role machine.
- Task 7: opusdm-hub JobEngine (validate, MAX_CONCURRENT=4, map_progress).
- Task 8: opusdm-hub server.rs - accept loop, owner loop, HubEvent/HubCmd bridge.
- Task 9: opusdm-hub startup - acquire_socket, spawn_lister, install_sigchld; headless main.
- Task 11: opusdm-lister crate, Client::connect + reader thread, env guard in main.
- Task 12: opusdm-lister model.rs (pure): rows, format_size, format_mtime, dir_first_cmp.
- Task 16 (partial): hub lib.rs + integration_copy smoke test (passes), run-x11.sh launches opusdm-hub, QA-checklist.md. GUI tasks 10/13/14/15 pending libgtk-4-dev.
- Task 10: opusdm-hub toolbar.rs GTK window (New Lister, SRC/DST indicators, job rows, Quit); main runs the GTK app. gtk4 pinned to 0.10 + v4_10 (0.11 needs rustc 1.92).
- Task 13: opusdm-lister window.rs - ColumnView (BoxedAnyObject rows), path bar, sort buttons, navigation, focus, status line, inbound pump. actions/dnd stubs.
- Task 14: opusdm-lister actions.rs - Copy/Move/Delete/MakeDir/NewFile/Rename, role toggles, conflict dialog, error dialog. Hub now resolves Copy/Move dest_dir from the DEST Lister path (server.rs); resolve_copy_target is a precondition check.
- Task 15: opusdm-lister dnd.rs - text/uri-list drag source + drop target (Shift=Move). RunJob.dest_dir is now Option: None=button Copy/Move (hub resolves), Some=explicit (DnD/Delete/MakeDir/NewFile).
- Task 16: release build verified; GUI smoke (hub spawns Lister at $HOME, single-instance handoff works); integration_copy passes. v1 feature-complete.
- Post-v1: hub toolbar is now a top-docked X11 panel - undecorated, full monitor width, _NET_WM_WINDOW_TYPE_DOCK + _NET_WM_STRUT_PARTIAL (top=42) so Listers tile below it. New crates/hub/src/dock.rs; deps gdk4-x11, x11rb.
- Fix: openbox was re-centering the DOCK panel. Now sets WM_NORMAL_HINTS user-specified position (0,0) + re-asserts geometry on map. Panel sits flush top-left, full width.
- Feature: Lock button disabled unless the Lister holds SOURCE or DEST.
- Feature: multiple DEST Listers. Role model reworked to a per-Lister map (RoleState.source()/dests()); a locked DEST persists, one unlocked DEST is transient. Button Copy/Move fans out to every DEST (one job per destination). Wire: ToLister RoleChanged/Welcome carry dests: Vec<ListerId>.
