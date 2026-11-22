# Desktop icons: same context menu and shortcuts as the Lister

Owner request (2026-11-22): the Desktop icons must have the same context menu
and the same shortcuts as the files / folders of a Lister. Chosen approach: a
shared refactor (not a copy in the hub). Del on the Desktop moves to the
trash, as in the Lister.

## Why the two differ today

Both use `opusdm_iconview::IconField`. The Lister menu
(`crates/lister/src/context_menu.rs`) and its keys (`keys.rs`) are written
on `Rc<Ui>`, which lives in the `opusdm-lister` process. The Desktop lives in
the hub process (`Widgets`, no `Ui`), so it has its own reduced menu
(`crates/hub/src/desktop_menu.rs::fill_icon_menu`) and only the built-in
keys of `IconField`.

## Design

New crate `crates/fileactions` (`opusdm-fileactions`):

- `FileActionHost` trait: what a menu action needs from its host (parent
  window, notify, folder, selection, DEST check, submit job, rename,
  confirm delete, file type, run a file type function, viewer, background,
  inside-archive test, host-only extra items).
- `MenuSink` trait: where items go (the Lister popover, the Desktop box).
- `fill_entry_menu(sink, host, target)`: the menu of one entry, the order
  of today's Lister menu.
- The dialogs used by the menu move into the crate, written on the host:
  permissions, open with, compress, extract, encrypt / decrypt.
- `keys`: the file keys (F3, F5, F6, Del, Shift+Del, F2, Ctrl+C/X/V,
  Ctrl+N, Ctrl+Shift+N) as `FileKey`, with a parity test against the Lister
  key table.

Hosts:

- `ListerHost { ui: Rc<Ui> }`: calls the existing Lister functions. The
  menu closures and dialogs held an `Rc<Ui>` before; they hold the same
  `Rc<Ui>` through the host now (no ownership change). Extra items: Leave
  Out on Desktop, Checksums (folders).
- `DesktopHost { widgets: Rc<Widgets> }`: jobs as `HubCmd::RunJob`, DEST
  folders from `HubEvent::Roles` (kept in `Widgets`), notify = log +
  requester. Extra items (before the shared ones): Open, Put Away, Cut,
  Copy (clipboard), as today. A left-out icon (real file elsewhere) gets no
  Rename / Move / Delete, as today.

## Steps (one commit each, app run in between)

1. Crate + trait + dialogs moved; Lister uses them through `ListerHost`.
   No behaviour change in the Lister.
2. Lister context menu built by `fill_entry_menu`. No behaviour change.
3. Desktop icon menu built by `fill_entry_menu` through `DesktopHost`
   (`HubCmd::SetBackground` added for "Set as Desktop Background").
4. Desktop file keys through `keys::resolve` (Del = trash).

## Not shared on purpose

- Leave Out on Desktop (the file already is on the desktop) and Checksums
  (needs the Lister size column) stay Lister-only.
- The recursive-filter pattern requester of the Lister jobs is not asked on
  the Desktop (no settings there).
