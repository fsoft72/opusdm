# F-34 Program Groups - design

Status: decisions taken by the implementer on the user's explicit delegation
("take the decisions and write the code"); no separate approval stage.
Part of roadmap step 6 (Desktop features). F-33 and F-37 are done; F-38 follows.

## Goal

A group is a named collection of programs, files or folders shown as one icon
on the desktop. Dropping items on the icon adds them; opening the icon shows a
group window from which every member can be launched.

## Decisions

- Storage: one file per group, `~/.config/opusdm/groups/<id>.json`, shape
  `{"name":"Editors","items":["/usr/bin/gedit"],"x":120.0,"y":80.0}` (`x`, `y`
  optional). `<id>` is a slug of the name (lowercase letters, digits and `-`),
  made unique with a `-2`, `-3`... suffix. Names are unique, ignoring case.
- Desktop icon: the folder icon plus a group badge (a different glyph from the
  left-out arrow), at a position pinned by the group id like a left-out icon.
  The entry carries the virtual path `group:<id>` and `IconEntry.group = true`.
- Group window: a normal top-level window (one per group, re-presented when
  already open) with an `IconField` in grid mode. Double click launches:
  folder (or symlink to a folder) opens a Lister, `.desktop` file launches
  the application, an executable regular file runs (detached, in its own
  folder), anything else opens with the default application.
- Adding members: drop files on the group icon on the desktop, or on an open
  group window. Only absolute paths are accepted; a path already in the group is
  skipped. A member whose file disappears stays and shows as a broken icon.
- Managing: desktop menu "New Group...", group icon menu "Open", "Rename
  Group...", "Delete Group..." (asks for confirmation; the members are never
  touched), group window context menu "Remove from Group".
- Cut, Copy, Rename and Delete of files are not offered on a group icon.

## Scope

In scope: the store (`opusdm_ipc::groups`), `IconEntry.group` and its badge,
merging groups into the desktop listing, positions by path, the group window,
desktop wiring and docs.

Out of scope: adding to a group from the Lister context menu (drag and drop
covers it), reordering members, groups on secondary monitors, nested groups,
icons taken from `.desktop` files.

## Design

1. `opusdm_ipc::groups` (pure): `Group`, `Groups` (create, rename, delete,
   add_items, remove_item, set_position, positions), `slugify`, load and save
   per group file, `group_path` / `group_id` for the virtual path.
2. `opusdm_iconview`: `IconEntry.group`, badge glyph, pinned positions and
   `connect_pinned_moved` also report group entries.
3. `opusdm_hub::leftout`: `DesktopExtras` (left-out items plus groups) replaces
   the bare `LeftOut` as what gets merged into the desktop listing and where
   the pinned positions come from; `launch_action` classifies a member.
4. `crates/hub/src/group_window.rs` (GTK): the group windows.
5. `crates/hub/src/toolbar.rs`: state, menus, drop and activation wiring.

## Error handling

Empty or duplicate group names are refused with a message dialog and nothing is
saved. Unreadable or malformed group files are skipped with a log line and
never deleted. Save failures are logged; the in-memory state stays valid.

## Testing

Unit tests for the store (slug, unique ids, name rules, add and remove, save and
load, malformed file skipped, bad coordinates), the merge of group entries, the
positions map, `launch_action`, and the `IconField` badge and pinned report for
group entries. The GTK wiring is verified by build and clippy; the manual
acceptance (create a group, drop 3 apps, open it, launch one) needs an X11
session.
