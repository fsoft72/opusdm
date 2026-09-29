# OpusDM job engine base (F-15, F-07) - design

Status: draft, pending user review 2026-09-29. Sub-project A of item 3 of the
suggested order in `TODO.md` ("File operation variants"). Item 3 is split into
four sub-projects, each with its own spec, plan and implementation:

- **A (this spec)**: F-15 recursive filter, F-07 Copy/Move variants and flags.
- B: F-09 Protect (`Chmod`) and F-10 DateStamp (`Touch`), reusing the F-15 filter.
- C: F-12 MakeLink and F-13 Join/Split.
- D: F-08 CheckFit and F-31 Reselect (lister side, no job).

## Goals

- Extend the hub job engine with a `JobOptions` struct carried next to `Op`,
  so that flags combine without multiplying `Op` variants.
- **F-15**: an `include_glob` applied during the recursive scan; a
  `recursive_filter` flag in `settings.json` makes job submission ask for the
  pattern.
- **F-07**: new operations `CopyAs`, `MoveAs`, `Duplicate`, and the flags
  `update`, `newer`, `move_when_same`, `quiet` on Copy/Move.
- Flags are reachable from the Function Editor (parameters of the internal
  Copy/Move commands, plus new internal commands) and from the Command
  Palette. The existing Copy/Move buttons keep their current behaviour.

## Non-goals

- Global defaults for `update`/`newer` applied to the toolbar buttons.
- A conflict prompt UI for Function steps (still auto-skip, see F-01 follow-ups).
- Protect/Touch/Link/Join/Split (sub-projects B and C). They will reuse
  `JobOptions.include_glob` but are not part of this spec.

## Data model (`opusdm_ipc`, `opusdm_fsops`)

- `Op` gains three variants:
  - `CopyAs { new_names: Vec<String> }` and `MoveAs { new_names: Vec<String> }`:
    one new name per source, in the same order as `srcs`. Lengths must match,
    otherwise the job fails fast with a descriptive error. Names may not
    contain `/`.
  - `Duplicate`: copy each source into its own parent directory. The new name
    is `name.copy`, then `name.copy2`, `name.copy3`, ... the first one that is
    free. `dest_dir` is ignored.
- New `JobOptions { update: bool, newer: bool, move_when_same: bool, quiet: bool,
  include_glob: Option<String> }`, `Default` = everything off,
  `#[serde(default)]` on every field, so existing IPC messages and stored
  `functions.json` files still deserialize.
- `JobSpec` and the hub's job-submission message carry `options: JobOptions`.
  A missing `options` on the wire means `JobOptions::default()`.

## Behaviour (`opusdm_fsops`)

- **Filter** (`scan.rs`): `scan` takes the optional glob. Only regular files
  and symlinks are matched (against the file name, not the full path);
  directories are always traversed. A directory that ends up with no
  surviving entries is not created at the destination. Excluded files are not
  counted in `Scanning` totals.
- **`update`**: copy a file only if an entry with that name already exists at
  the destination.
- **`newer`**: copy only if the destination is missing or has an older mtime.
- Both are decided in the work pass before a `Conflict` event would be
  emitted; a file excluded this way counts as `skipped`. When both are set,
  both must hold.
- **`move_when_same`**: on `Copy`, if source and destination share `st_dev`,
  the job is run as a Move (rename fast path). Different devices keep Copy.
- **`quiet`**: suppresses warning requesters (e.g. the `CopyAs` name-count
  warning). It never suppresses conflict prompts.
- **`CopyAs`/`MoveAs`**: each source is mapped onto its new name at the
  destination, going through the same conflict resolution as Copy/Move.
- **Duplicate** collision search runs at plan time so two sources never pick
  the same new name.

## UI and integration

- **Function Editor**: `ipc::function::schema::params_for` exposes `update`,
  `newer`, `move_when_same` (checkboxes) and `include_glob` (text) as optional
  parameters of Copy and Move, and `include_glob` on Delete. New internal
  commands `CopyAs`, `MoveAs`, `Duplicate` are added to `InternalCmd`, with a
  `new_names` parameter for the "As" pair. `hub/src/functions.rs` maps the
  parameters onto `JobOptions`.
- **Command Palette**: "Copy As...", "Move As...", "Duplicate" and
  "Copy (newer only)". The "As" commands open a naming dialog with a live
  preview, reusing the Batch Renamer preview widget
  (`crates/lister/src/batch_rename.rs`).
- **Recursive filter setting**: `settings.json` gets `recursive_filter: bool`
  (default false). When true, submitting a recursive Copy/Move/Delete from a
  Lister first opens a pattern requester and puts the result in
  `include_glob`; cancelling the requester cancels the job. Sub-project B
  will hook Protect/Touch in the same way.

## Error handling

- Name-count mismatch, invalid name (`/`, empty) and an invalid glob are
  rejected before any file is touched, with a message naming the offending
  value.
- Per-file errors keep flowing through `Progress::Done.errors` as today.

## Testing

Unit tests in `opusdm_fsops`, one per acceptance criterion:

- `newer` over an older/newer pair copies only the newer file;
- `update` copies only files whose name already exists at the destination;
- filtered recursive copy of `*.rs` skips every other file and creates no
  empty directories;
- `Duplicate` of three files produces `name.copy` style names without
  collisions;
- `move_when_same` renames on the same device and copies across devices
  (device check behind a small seam so it is testable);
- `CopyAs` with mismatched name count fails before touching the filesystem.

`opusdm_ipc`: a JSON compatibility test (old `Op`/`JobSpec` payloads without
`options` still parse), plus round-trips for the new variants and for
`functions.json` with the new parameters.

Hub: a test that `recursive_filter` on makes a Lister-submitted job wait for
the pattern and that cancelling drops the job.
