# File operation variants B, C, D (F-09, F-10, F-12, F-13, F-08, F-31) - design

Status: written 2026-09-29 while the user was away, decisions taken by the
implementer. Continues `2026-09-29-job-engine-base-design.md` (sub-project A).
The plan is the last section of this file.

## Sub-project B: Protect and DateStamp (F-09, F-10)

- `Op::Chmod { mode: u32, recurse: bool }` and `Op::Touch { unix_secs: i64, recurse: bool }`.
- `mode` is the 0o777 permission set (no setuid/setgid/sticky, no chown: chown
  needs privileges the file manager does not have; not implemented).
- Walker (`opusdm_fsops::attrs`): visits the selected entries; with `recurse`
  walks trees children first (a directory is changed after its content, so a
  mode without `x` cannot lock the walk out). Symlinks are never followed.
- Chmod on a directory adds `x` wherever `r` is set (the `X` of chmod), so a
  recursive 0644 keeps directories traversable. Symlinks are skipped by Chmod
  (their mode is meaningless on Linux). Touch changes a symlink itself.
- `include_glob` (F-15) affects files and symlinks only. With a filter,
  directories are never changed.
- Touch sets access and modification time to `unix_secs`.
- Time entry uses local time, `YYYY-MM-DD HH:MM:SS`, parsed with `libc::mktime`
  in `opusdm_ipc::timefmt`.
- UI: palette "Change Permissions..." (rwx grid for owner/group/other, octal
  entry kept in sync, "Apply recursively") and "Set Timestamp..." (entry,
  "Now" button, "Apply to contents"). Both go through `job_submit::submit_job`
  so the recursive filter prompt applies when `recurse` is on.
- Function Editor commands for these two are a follow-up, not part of this work.

## Sub-project C: Links, Join, Split (F-12, F-13)

- `Op::Link { symbolic: bool }` and `Op::LinkAs { symbolic: bool, new_names }`
  create links in `dest_dir` pointing at each source (absolute target for
  symbolic links). Hard links to directories are refused with a clear error;
  a hard link across devices reports the OS error. An existing target is an
  error for that entry, never overwritten.
- `Op::Join { target: String }`: concatenate `srcs`, in the given order, into
  `dest_dir/target` (must not exist). `Op::Split { chunk_bytes: u64 }`: split
  the single source into `stem.001`, `stem.002`, ... inside `dest_dir`
  (`stem` = the full file name; parts numbered from 001, width grows past 999).
  Parts must not exist. Both stream through a buffer and honour cancel.
- UI: "Make Link..." (choose hard or symbolic, into DEST), "Join Files..." (the
  selection in its current order, target name entry), "Split File..." (size
  entry in KiB/MiB, one selected file, into DEST).
- Function Editor support is a follow-up.

## Sub-project D: CheckFit and Reselect (F-08, F-31)

- **CheckFit**: `opusdm_fsops::checkfit::{needed_bytes, free_bytes, CheckFit}`.
  `needed_bytes` sums the selection recursively (symlinks count zero, not
  followed); `free_bytes` uses `statvfs` (`f_bavail * f_frsize`). `CheckFit {
  needed, free, fits, percent }`, `percent` = share that fits, capped at 100.
  Run by the Lister on a worker thread (no hub round trip needed: the Lister
  already knows the DEST path from the role state). Palette "Check Fit
  (Source -> Dest)..." shows a requester with the three numbers. The optional
  status bar badge is left out: it would add work to every selection change.
- **Reselect**: each Lister remembers the names of the selection that was
  active when it last got cleared by a job it started (Copy/Move/Delete...) or
  by Deselect All. Palette "Reselect" and `Ctrl+Shift+A` restore them (names
  that still exist). Kept in `Ui` state, no IPC.

## Testing

Unit and integration tests per module (attrs, links, join/split, checkfit,
timefmt, mode helpers, reselect bookkeeping). GTK dialogs are covered through
their pure helpers.

## Plan

1. B: ipc ops + `timefmt`; fsops `attrs`; dialogs; docs; commit.
2. C: ipc ops; fsops `links`, `joinsplit`; dialogs; docs; commit.
3. D: fsops `checkfit`; Lister dialog and reselect state; docs; commit.
