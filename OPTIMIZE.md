# Optimization and cleanup notes

Items judged not worth the risk right now (CLAUDE.md rule 8). Each says what
and why it was kept.

## Panel

- `crates/panel/src/wl_windows.rs`: the native request worker (`NATIVE_JOBS`, `start_native_worker`, `with_native`) duplicates the Wayfire one (`WAYFIRE_JOBS`, `start_wayfire_worker`, `with_wayfire`) almost line for line. Kept on purpose (2026-11-25, stage 2 task 11): merging them would touch the Wayfire worker, which the owner's Wayfire session runs (CLAUDE.md rule 1). A shared `worker(name, label)` helper is possible once a Wayfire session can be exercised after the change.
