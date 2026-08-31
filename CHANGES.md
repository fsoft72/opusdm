# Changes

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
