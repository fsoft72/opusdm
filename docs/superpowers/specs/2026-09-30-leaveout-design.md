# F-33 LeaveOut - design

Status: approved in conversation, pending written-spec review.
Part of roadmap step 6 (Desktop features). F-37 is done; F-34 and F-38 follow,
each with its own spec, plan and implementation cycle.

## Goal

Let the user leave any file or folder out on the desktop as a permanent
shortcut icon with an arrow badge, remember it (and where the user dragged it)
across restarts, and put it away again.

## Decisions taken

- Positions: stored per path, only for left-out icons. Icons of the Desktop
  folder stay auto-slotted as today.
- Triggers: Lister context menu, internal function commands, and the desktop
  icon context menu. No new drag and drop behavior.
- A left-out entry whose target no longer exists stays visible as a broken
  icon and is removed with Put Away (an unmounted disk must not lose the
  entry).

## Scope

In scope:
- `leftout.json` store and its pure operations.
- `IconEntry.left_out` flag, the badge overlay, and per-path pinned positions
  in `IconField` with a "pinned icons moved" callback.
- Hub desktop wiring: merged entry list, persistence, "Put Away" menu entry.
- `ToHub::LeaveOut`, `HubEvent::LeftOutAdd` / `HubEvent::LeftOutRemove`.
- Lister menu entry "Leave Out on Desktop".
- Internal commands `LeaveOut` and `PutAway`.

Out of scope:
- Saving positions of Desktop folder icons.
- Drag and drop onto the desktop with a modifier key.
- Filetype actions (F-06), left-out icons on secondary monitors.

## Facts about the current code

- The desktop `IconField` runs in `LayoutMode::Free` on the primary monitor
  only (`crates/hub/src/toolbar.rs`). Free positions are held in memory,
  keyed by entry index, and `IconField::set_entries` clears them. The desktop
  watcher and "Rearrange icons" both call `set_entries`.
- `IconField::refresh_entries` remaps free positions by path but nothing is
  persisted.
- `docs/usage.md` section 3.12 says the icon layout lives in a `.opusdm/`
  directory inside the desktop folder. The code has no such directory; the
  sentence is corrected as part of this work.
- The desktop icon menu is built by `fill_icon_menu` in `toolbar.rs`.
- Function steps emit `FnEvent`s that `Hub::function_event` maps to messages;
  `SetBackground` (F-37) is the template for a step that acts on the hub.

## Design

### 1. Store (`crates/ipc/src/leftout.rs`, new)

- File: `~/.config/opusdm/leftout.json`:
  `{"items":[{"path":"/etc/fstab","x":120.0,"y":80.0}]}`; `x` and `y` are
  optional.
- Types `LeftOut { items: Vec<LeftOutItem> }` and
  `LeftOutItem { path: PathBuf, x: Option<f64>, y: Option<f64> }`.
- Pure operations: `add(&mut self, paths: &[PathBuf])` (skips paths already
  present, keeps insertion order), `remove(&mut self, paths: &[PathBuf])`,
  `set_position(&mut self, path: &Path, x: f64, y: f64)` (no-op for an unknown
  path), `positions(&self) -> HashMap<PathBuf, (f64, f64)>`.
- `load()` / `save()` follow the other config files in `opusdm_ipc`: a missing
  or malformed file gives an empty store, save creates the directory.
  Coordinates are clamped to be non-negative and finite when loaded.

### 2. `IconField` (`crates/iconview`)

- `IconEntry` gets `left_out: bool` (default `false` at every existing
  constructor). `IconItem` draws the arrow badge as an overlay in the corner
  of the icon box when it is set. The badge does not change the item size.
- New `IconField::set_pinned_positions(&self, HashMap<PathBuf, (f64, f64)>)`.
  `set_entries` and `refresh_entries` apply these positions, by path, to the
  entries that have one, in addition to their existing behavior. Entries
  without a pinned position keep auto-slotting.
- New `IconField::connect_pinned_moved(f: impl Fn(Vec<(PathBuf, (f64, f64))>))`,
  called at the end of a reposition drag with the new position of every moved
  entry that has `left_out == true`. Moving other icons behaves as before.

### 3. Hub desktop (`crates/hub`, GTK side)

- New module `leftout.rs`: owns the in-memory `LeftOut`, saves it after every
  change, and builds the merged entry list for the desktop field:
  Desktop folder entries plus one entry per left-out item. An entry present
  in both (same path) appears once with `left_out = true`. A left-out path
  that no longer exists becomes an `IconKind::Broken` entry named after the
  file name. `IconEntry`s for existing paths come from a new
  `opusdm_iconview::scan::entry_for_path(path)` (uses `symlink_metadata`).
- Every place that fills the desktop field (initial load, watcher reload,
  "Rearrange icons", desktop directory change) uses the merged list and calls
  `set_pinned_positions` first.
- `wire_primary_icon_field` connects `connect_pinned_moved` to
  `LeftOut::set_position` + save.
- Activating a broken left-out entry does nothing except log
  `left-out target missing: <path>`.
- `fill_icon_menu`: for entries with `left_out == true`, add "Put Away" below
  "Open". It removes the item from the store, saves, and refreshes the field.
  The file itself is never touched.
- New protocol pieces: `ToHub::LeaveOut { paths: Vec<PathBuf> }` (from a
  Lister), `HubEvent::LeftOutAdd(Vec<PathBuf>)`,
  `HubEvent::LeftOutRemove(Vec<PathBuf>)`; the GTK side applies them to the
  store, saves and refreshes the field.

### 4. Lister and functions

- Lister context menu (`crates/lister/src/context_menu.rs`): "Leave Out on
  Desktop" for files and folders. It sends `ToHub::LeaveOut` with the row's
  path, or with the whole selection when the clicked row is part of it.
- `InternalCmd::LeaveOut` (label "Leave Out") and `InternalCmd::PutAway`
  (label "Put Away"): no parameters, act on the SOURCE selection. The
  executor emits `FnEvent::LeaveOut { paths }` / `FnEvent::PutAway { paths }`;
  `Hub::function_event` handles them for every requester (like
  `SetBackground`) and sends the matching `HubEvent`. An empty selection
  fails the step with `Leave Out: nothing selected` / `Put Away: nothing
  selected`. Both are added to the Function Editor list.

## Error handling

- A path that is not absolute or does not exist when leaving out fails fast
  with a message that includes the path; nothing is stored. Paths inside the
  config directory are allowed. Save failures are logged with the path and the
  in-memory state stays valid.
- Non-finite or negative coordinates in `leftout.json` are dropped (the entry
  loses its position, not its place).

## Testing

- `leftout.rs`: add skips duplicates and keeps order; remove; set_position on
  known and unknown paths; JSON roundtrip; malformed file gives empty; bad
  coordinates dropped on load.
- `IconField` (uses the existing GTK test setup): pinned positions survive
  `set_entries` and `refresh_entries` by path; entries without a pinned
  position stay auto-slotted; `connect_pinned_moved` reports only left-out
  entries after a drag.
- Hub: merged list has no duplicate for a path in both sources and marks it
  `left_out`; a missing target yields a `Broken` entry; `LeaveOut` /
  `PutAway` executor steps (selection required, event emitted, failure emits
  nothing); server test that `ToHub::LeaveOut` emits `LeftOutAdd`.
- Manual acceptance: leave out `/etc/fstab` from a Lister, drag it, restart
  the hub: the icon is there with the badge, at the dragged position. "Put
  Away" removes it and leaves the file alone. Unmount or rename the target:
  the icon becomes broken and can be put away.

## Documentation

`CHANGES.md`, `TODO.md` (F-33), the F-33 section of `FEATURES.md`
(`leftout.json` shape, per-path positions), and `docs/usage.md`: a new
section for LeaveOut, and section 3.12 corrected (no `.opusdm/` layout
directory exists).
