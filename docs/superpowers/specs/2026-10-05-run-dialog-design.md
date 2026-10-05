# Run dialog (`opusdm-run`)

Date: 2026-10-05. Status: approved in chat, implemented on branch `opusdm-run`.

## Goal

A key combination opens a small "Run..." window at the top of the screen,
right under the hub bar. The user types a program name (from the OpusDM
application menu) or a command (`bash`); suggestions appear below the input
while typing, and Enter runs the choice.

## Decisions

- **Own process.** A new crate `crates/runner` builds the binary `opusdm-run`
  (deps: `opusdm-ui`, `opusdm-dock` for `apps::resolve` / `apps::launch`,
  `opusdm-ipc`). It starts fast and does not depend on the Lister.
- **Hotkey is a normal hotkey.** The user binds `exec:opusdm-run` in Edit
  Hotkeys (Global ticked for a system-wide key). No fixed key, no new setting.
  The `opusdm-session` script puts its own `bin/` on `PATH`, so the bare name
  resolves in a real session.
- **Toggle.** The application id `org.opusdm.Run` is unique: a second launch
  while the window is open closes it.

## Window

- Undecorated, 600 px wide, centered horizontally at the top of the primary
  monitor, just below the panel (`PANEL_HEIGHT` below the top edge when
  `desktop.json` puts the panel at the top, at the top edge otherwise).
- Input with a search icon and a clear button; suggestion list below it,
  hidden while empty. Up / Down move the selection, Enter runs it, Escape or
  losing the focus closes the window, a click on a row runs it.
- Each row: kind label (shown on the first row of a run of the same kind),
  icon, name, dimmed comment.

## Suggestions

- **Applications**: the apps of the custom menu (`menu.json`, folders
  flattened) when it has any, else every installed visible desktop entry.
- **Commands**: executables found on `PATH`, scanned once at start-up.
- Matching is case-insensitive over the whole typed text. Rank, best first:
  exact name, name prefix, word prefix, substring of the name, then generic
  name, keywords and the command line. Ties: applications before commands,
  then by name. At most 8 rows.
- When the text has a space, commands are also matched on its first word and
  the rest is kept as their arguments (`bash -c ls`).

## Running

- A selected application is launched through `opusdm_dock::apps::launch`.
- A selected command, or the typed text when nothing is suggested, is split
  with `opusdm_ipc::cli::tokenize` (same rules as the `exec:` hotkeys: no
  shell, quotes group words) and started detached, in its own process group,
  with null stdio. `bash -c "a | b"` is the way to use shell syntax.
- A failure to start (program not found, bad quoting) is shown in red under
  the input and the window stays open. Success closes it.

## Tests

Pure logic is unit tested: ranking, flattening of `menu.json`, the `PATH`
scan, command splitting and the detached start. The GTK window is checked by
hand in the Xephyr sandbox.

## Out of scope

Command history, calculator, file search, Tab completion, settings.
