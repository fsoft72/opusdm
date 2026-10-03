# FEATURES.md - Directory Opus 5 feature gap for OpusDM

This document lists the features of the original **Directory Opus 5** (as
documented in `DOpus5.guide`, v58.3) that are **missing or only partially
implemented** in OpusDM, together with implementation specifications detailed
enough to hand to an LLM (or a human) as work items.

Sources used:

- Original behaviour: `DOpus5.guide` (AmigaGuide node names are quoted for
  reference).
- Current OpusDM behaviour: `docs/usage.md`, `docs/dock.md`, `CHANGES.md`.

How to read this document:

- Each feature has an ID (`F-xx`), an **Original** section (what DOpus 5 did),
  and a **Spec** section (how it should map onto OpusDM's architecture).
- OpusDM architecture reminders for implementers:
  - `opusdm-hub` (crate `crates/hub`): background coordinator, runs all
    filesystem jobs asynchronously (progress, conflict handling, cancellation),
    owns the panel/dock/desktop, talks IPC over a Unix socket.
  - `opusdm-lister` (crate `crates/lister`): GTK4 file windows; commands are
    dispatched through the Command Palette table (`palette.rs`) and the key
    table (`keys.rs`); lister -> hub jobs go through `ToHub::RunJob`.
  - `opusdm-ipc` (crate `crates/ipc`): shared config/message types; settings
    live in `~/.config/opusdm/*.json` and are pushed live to all listers.
  - Shared widgets in `crates/ui` (e.g. `opusdm_ui::requester`).
- Anything marked **N/A (Amiga-only)** is listed for completeness and should
  NOT be implemented.

---

## 1. Command and function system

### F-01 Function Editor and internal command language
**Original** (`Function Editor`): every button, menu item, hotkey, filetype
action or script is a *function*: an ordered list of commands. Each command
can be an internal Opus command (Copy, Rename, ...), an external program, or
a script. Argument codes inject context: `{f}`/`{F}` selected entries with
paths, `{o}`/`{O}` names only, `{df}`/`{dF}`/`{do}`/`{dO}` the same from the
DEST lister, `!` makes an argument optional (`{df!}`). Functions have flags
(run async, wait, windowed output, ...) and can carry their own hotkey.

**Spec**:
- New module `opusdm_ipc::function`: `Function { name, hotkey: Option<String>,
  steps: Vec<FnStep> }`, `FnStep::{Internal { cmd, args }, External { argv,
  cwd_source: bool }}`. Persist to `~/.config/opusdm/functions.json`.
- Implement an argument-expansion pass resolving `{f} {F} {o} {O} {df} {dF}
  {do} {dO}` against the current SOURCE selection and DEST selection; `{x!}`
  expands to empty instead of prompting.
- New editor window (reuse requester style): list of steps, add/remove/reorder,
  per-step type picker, hotkey capture field.
- Functions must be runnable from the Command Palette, from hotkeys (F-03),
  from button banks (F-02) and from filetype actions (F-06).
- Acceptance: define a function "copy selection to DEST then run
  `notify-send done {o}`" and run it from palette and hotkey.

### F-02 Button Banks (custom button windows) + Button Bank Editor
**Original** (`Button Bank Editor`, `Button Editor`, `Buttons - *`,
`LoadButtons`, `CloseButtons`): free-floating windows containing user buttons
(text or images). Each button carries up to three functions (left / right /
middle click). Banks can be saved/loaded, hidden, iconified, toggled,
re-shaped, opened under the mouse; a *Start Menu* variant pops up as a menu.
The lister toolbar itself is a button bank and is editable
(`Lister - Edit Toolbar`).

**Spec**:
- New crate `crates/buttonbank`: undecorated GTK4 window rendering a grid of
  buttons from `~/.config/opusdm/buttons/<name>.json`
  (`ButtonBank { name, buttons: Vec<Btn> }`, `Btn { label, icon: Option<path>,
  functions: { left, right, middle } -> function ids from F-01 }`).
- Commands: open/close/hide/toggle bank by name, open-under-mouse.
- Editor: add/insert/delete/copy/paste buttons, drag-reorder, edit button
  (opens F-01 editor), bank appearance (borderless, orientation, visible on all desktops).
- Hub owns bank windows (like it owns panel/dock) so they survive lister
  restarts; IPC messages `OpenBank/CloseBank/ToggleBank`.
- Acceptance: create a bank with 3 buttons bound to functions, save, restart
  hub, bank restores; middle-click runs the alternate function.

### F-03 Hotkey Editor (incl. system-global hotkeys)
**Original** (`Hotkey Editor`, `Project - KeyFinder`): bind any function to a
key; optional **system-global** flag makes it work even when no Opus window
is focused. KeyFinder locates which function owns a key combination.

**Spec**:
- Extend `settings.json`/new `hotkeys.json`: `{ key: "Ctrl+Alt+U",
  function: <id>, global: bool }`.
- Local hotkeys: merge into lister `keys.rs` dispatch.
- Global hotkeys (X11 session): hub grabs keys via XRecord/XInput2
  (project already runs on X11, see `run-x11.sh`); document that Wayland is
  unsupported for global grabs.
- KeyFinder: small requester "press a key combo" -> shows matching binding.
- Acceptance: global hotkey opens a new lister while a non-OpusDM window has
  focus.

### F-04 User menus in listers
**Original** (`UserMenu`, `User1..User4`, `Menu Editor`): user-editable
hierarchical menus attached to the lister toolbar/menu button, entries run
functions.

**Spec**:
- `~/.config/opusdm/usermenu.json`: tree of `{ label, function }` entries
  (reuse `opusdm_ipc::menu::MenuConfig` pattern from the panel app menu).
- Add a "Menu" drop-down to the lister action toolbar rendering the tree.
- Editor: reuse the existing Menu tab UI from `opusdm-settings`
  (`crates/desktop-config/src/menu_ui.rs`) generalized to functions instead of
  `.desktop` apps.
- Acceptance: create nested user menu, run entries, edit live without restart.

### F-05 Script events (event hooks)
**Original** (`Scripts Editor`): attach functions to system events: open/close
lister, open/close button bank, startup, shutdown, hide/reveal, double-click
on desktop background, middle/right double-click, plus viewer/print override
hooks (Read, HexRead, AnsiRead, SmartRead, Show, Play, Print, IconInfo).

**Spec**:
- `scripts.json`: `{ event: <enum>, function: <id> }`.
- Hub emits events: `Startup`, `Shutdown`, `ListerOpened`, `ListerClosed`,
  `DesktopDoubleClick`, `DesktopMiddleDoubleClick`, `DesktopRightDoubleClick`,
  `Hidden`, `Revealed`.
- Viewer overrides: before opening Quick Look for text/hex/image, check
  override hook and run external command with `{f}` instead.
- Acceptance: "open lister" hook opens a second lister at a fixed path;
  Read override replaces Quick Look with `gedit {f}`.

### F-06 Filetypes system
**Original** (`File Types`, `File Type Editor`, `File Class Editor`,
`Filetype Creator`, `Filetype Finder`, `CreateFiletype`, `DoubleClick`,
`DragNDrop`, `User1-4`): user-defined filetype classes matched by name
pattern, content pattern ("Match", "Match Bits", "Search For" at offsets),
size, date. Each type defines actions: double-click, drag&drop target,
context ("Open With") menu entries, user commands. Unknown files trigger
Filetype Finder (or SmartRead fallback - see `Environment - Miscellaneous`
"Filetype Sniffer").

**Spec**:
- `filetypes.json`: `FileType { name, match: MatchRule, actions: { dblclick:
  fn_id, drop: fn_id, menu: Vec<fn_id> } }`,
  `MatchRule::{ Glob, ContentBytes { offset, bytes }, ContentText { needle,
  range }, SizeRange, DateRange, And(Vec), Or(Vec) }`.
- Matching order: user types first, then MIME sniffing (`xdg` default
  fallback keeps current behaviour).
- Wire double-click in `lister` to: enter dir / enter archive (existing) ->
  filetype action -> xdg-open fallback.
- Right-click on a file shows filetype menu entries ("Open With..." list).
- Filetype Creator wizard: pick a file, auto-derive a Glob/ContentBytes rule.
- Acceptance: define type "PNG image" with dblclick -> `feh {f}` and a menu
  entry; double-click and context menu behave accordingly.

---

## 2. File operations

### F-07 Copy/Move variants: CopyAs, MoveAs, Duplicate, UPDATE/NEWER/MOVEWHENSAME
**Original** (`CopyAs`, `MoveAs`, `Duplicate`, `Copy` flags): copy/move with
per-entry new names (wildcards allowed), duplicate in place, `UPDATE` (copy
only if name exists at dest), `NEWER` (copy only if missing or newer),
`MOVEWHENSAME` (move instead of copy when src and dest share a device),
`QUIET` to suppress warning requesters.

**Spec**:
- Extend `opusdm_ipc` job op enum with `CopyAs { new_names }`,
  `MoveAs { new_names }`, `Duplicate`, and flags `update`, `newer`,
  `move_when_same`, `quiet` on Copy/Move.
- Hub `fsops`: implement filters before enqueueing each file (skip when
  UPDATE/NEWER conditions fail); `move_when_same` compares `st_dev`.
- UI: palette commands "Copy As...", "Move As...", "Duplicate"; batch naming
  dialog reusing the Batch Renamer preview widget.
- Acceptance: NEWER copy over an older/newer pair copies only the newer file;
  duplicate of 3 files prompts/creates `name.copy` style names.

### F-08 CheckFit (space pre-check)
**Original** (`CheckFit`): shows bytes needed vs. free space at DEST and the
percentage that fits; GetSizes integration shows a Y/N fit indicator in the
status bar.

**Spec**:
- Hub: sum selection sizes (reuse recursive sizer), `statvfs` on dest, return
  `{ needed, free, fits }`.
- Lister: requester with the three numbers; optional status-bar Y/N badge
  next to the selection counter when a DEST exists.
- Acceptance: selecting 2 GB with 1 GB free shows `fits = false`, 50%.

### F-09 Protect (chmod/chown requester)
**Original** (`Protect`, `Protectionbits`): edit protection bits of selected
entries, optional recursion into directories.

**Spec**:
- Palette "Change Permissions...": requester with rwx checkboxes per class,
  numeric field, recursive toggle (with pattern filter when F-15 exists).
- Hub job op `Chmod { mode, recurse }`.
- Acceptance: multi-file chmod 0644 applied recursively with filter `*.txt`.

### F-10 DateStamp (touch)
**Original** (`DateStamp`): set modification date/time of selected entries,
"current time" shortcut, optional recursion.

**Spec**: palette "Set Timestamp...": datetime entry + "Now" + "apply to
contents" toggle; hub job using `utimensat`. Acceptance: set fixed date on 2
files; Date column updates via inotify refresh.

### F-11 File comments
**Original** (`Comment`): attach a <= 79 char comment to files/dirs, shown in
the lister; recursive application possible.

**Spec**:
- Store comments as xattrs (`user.comment`) with filename-keyed fallback in
  `.opusdm/comments.json` inside the directory (xattrs don't survive all
  filesystems).
- New optional lister column "Comment"; palette "Edit Comment...".
- Acceptance: comment survives reload; appears in column and in Quick Look
  header.

### F-12 MakeLink / MakeLinkAs
**Original** (`MakeLink`, `MakeLinkAs`): create hard links in DEST (same
volume), optional new names.

**Spec**: extend to symbolic links too (Unix-natural). Palette "Make Link
(hard/symbolic)..."; hub ops `Link { kind, new_name? }`; render symlinks with
the existing `@` indicator, bold hard links optional. Acceptance: symlink in
DEST points at SOURCE file; hard link shares inode (`ls -i`).

### F-13 Join / Split
**Original** (`Join`, `Split`): concatenate files in a chosen order; split a
file into `CHUNK`-KB parts named `stem.001`, `stem.002`, ...

**Spec**: palette "Join Files..." (ordered list editor, async hub job) and
"Split File..." (size field in KiB/MiB, stem field). Acceptance: split a 10
MB file into 4 MiB chunks, join them back, `cmp` with original succeeds.

### F-14 Encrypt / Decrypt
**Original** (`Encrypt`): password-encrypt selected files into DEST; decrypt
with checkbox or `-password`.

**Spec**: use a modern AEAD (age or AES-256-GCM via a Rust crate), never the
original XOR scheme. Palette "Encrypt..." / "Decrypt..." with password
requester; output to DEST with suffix `.enc`. Acceptance: round-trip
encrypt->decrypt yields identical bytes; wrong password fails cleanly.

### F-15 Recursive Filter for operations
**Original** (`Settings - Filter`): global toggle; when on, any recursive
operation (copy/move/delete/protect/comment/datestamp) first asks for a file
pattern and only matching files are affected.

**Spec**: `settings.json` flag `recursive_filter`; when true, hub job
submission pops a pattern requester and attaches `include_glob` to the job;
fsops applies it during directory traversal. Acceptance: filtered recursive
copy of `*.rs` skips other files.

---

## 3. Viewers and search

### F-16 Full Reader: Read / AnsiRead / HexRead / SmartRead
**Original** (`Read`, `AnsiRead`, `HexRead`, `SmartRead`): internal viewer
window with text, ANSI (escape-art) and hex modes; SmartRead picks the mode
by sniffing; multiple selected files are shown in sequence (ESC aborts);
search inside the viewer; `WAIT` flag blocks subsequent commands.

**Spec**: extend Quick Look (it already does text/hex/images) with:
- ANSI escape rendering mode (CP437 charset + colour codes) for `.ans` files.
- In-viewer search bar (next/prev, highlight).
- "Open all selected in sequence" mode with next-file key.
- SmartRead auto-mode selection (already partially present; formalize).
Acceptance: SmartRead on an `.ans` shows colour art; searching jumps between
matches; 3 selected files page through in one window.

### F-17 Show (image slideshow)
**Original** (`Show`): picture viewer with next-image keys, mark-for-deletion
while browsing (Delete), print key; delete-marked files after the last image.

**Spec**: slideshow mode in Quick Look for image selections: `Left/Right`
navigate, `Del` marks for deletion, on close a confirmation requester offers
to delete marked files. Acceptance: mark 2 of 5 images, confirm, files
deleted.

### F-18 Play (sound files) + sound feedback commands
**Original** (`Play`, `Beep`, `Alarm`, `Flash`): internal audio player with
status requester, volume, WAIT; Beep/Alarm/Flash used as function-end
signals.

**Spec**: play audio through `pw-play`/`paplay` subprocess (no new audio
dependency); palette "Play" for audio files; internal commands
`beep`/`flash` (bell / window flash) for use in functions. Acceptance: a
function ending in `beep` beeps.

### F-19 FindFile (search by name across tree)
**Original** (`FindFile`): recursive filename search (full pattern matching)
from selected directories or devices; on match, offer to enter the directory
with matches selected.

**Spec**: palette "Find Files...": pattern + start dir; hub streams matches;
result dialog offers "Open containing folder and select" per match.
Acceptance: find `*.log` under `/var`, jump to a hit with it selected.

### F-20 Search (content search)
**Original** (`Search`): search file contents for a string inside selected
files/dirs (recursive, filterable); on hit, open the viewer positioned at the
match.

**Spec**: palette "Search in Files...": text + glob + start dir; background
grep-like scan (reuse walker); results list -> open in Quick Look at first
match (needs F-16 in-viewer search). Acceptance: find "TODO" in `crates/`,
open hit scrolled to match.

### F-21 Advanced Select requester
**Original** (`Select`): beyond glob selection: match by name, date range,
size range, protection bits, with include/exclude and files-only/dirs-only
switches.

**Spec**: extend the current `+`/`-` pattern selection with a full requester:
name glob, size min/max, mtime range, type (file/dir), include/exclude mode.
Acceptance: select files > 1 MB modified in the last 7 days, dirs excluded.

---

## 4. Lister capabilities

### F-22 Lister Format Editor (per-directory formats)
**Original** (`Lister Format`, `Configure`, `Lister - Edit`): choose which
columns display and their order (drag & drop), entry separation (dirs first /
files first / mixed), reverse sort, show/hide filename patterns baked into
the format, free-space gauge toggle, **Save per directory** and **Inherit**
(sub-dirs inherit parent format), reset to defaults.

**Spec**:
- Format model `ListerFormat { columns: Vec<Column>, separate, reverse,
  show_glob, hide_glob, gauge: bool }`; columns available: Name, Size, Date,
  User, Group, Access, Comment (F-11).
- Editor dialog with column add/remove/reorder; "Use" applies, "Save" stores
  per-path in `~/.config/opusdm/formats.json`; inherit flag walks up parents.
- Free-space gauge widget in the header stat bar (`statvfs`).
Acceptance: save a format for `/tmp` without User/Group columns; entering
`/tmp` applies it, `/var` keeps the default.

### F-23 Show/Hide name patterns (persistent filter)
**Original** (`Set Show`, `Set Hide`, `Lister - Filter`): persistent
show-pattern/hide-pattern per lister (distinct from the live fuzzy filter,
which OpusDM already has); wildcard syntax incl. negation.

**Spec**: add `show_glob`/`hide_glob` to the lister state (part of F-22
format); palette commands "Set Show Pattern..." / "Set Hide Pattern...".
Acceptance: hide `*.o` persists across reload and directory re-entry.

### F-24 Directory buffer cache + CacheList + PathHistory
**Original** (`CacheList`, `PathHistory`, `FreeCaches`): visited directories
stay buffered; CacheList lists all buffers across listers for instant jumps;
per-lister PathHistory menu jumps back; FreeCaches frees memory.

**Spec**:
- Lister keeps an LRU cache of recent dir listings (N configurable, default
  20) to make back/forward instant.
- Path history drop-down on the path bar (back/forward buttons).
- Hub-wide CacheList dialog listing every lister's cached paths -> focus that
  lister/path. "Free caches" command clears them.
Acceptance: revisit a 10k-file dir instantly from history; CacheList jump
focuses the right window.

### F-25 Lister tiling / cascade
**Original** (`Lister - Tile`): arrange all open listers tiled
(horizontal/vertical) or cascaded over the screen.

**Spec**: hub commands `TileListers { orientation }` / `CascadeListers` using
X11 `XMoveResizeWindow` (Wayland unsupported - document). Palette entries.
Acceptance: 3 listers tile side by side without overlap.

### F-26 Lister Snapshot / window geometry persistence
**Original** (`Lister - Snapshot`, `UnSnapshot`, `Lock Position`,
`Settings - Save Layout`): remember lister size/position; lock position;
save the whole layout (open listers + paths) as the default startup set.

**Spec**:
- Persist per-lister geometry keyed by path in `layout.json`; "Save Layout"
  stores all open listers+paths and restores on hub start.
- "Lock position" makes the window ignore moves.
Acceptance: save layout with 2 listers, restart, both reopen at same
path/geometry.

### F-27 Dual Lister (split pane in one window)
**Original** (`Lister - Dual Lister`, `Set Dual`): one lister window split
into two independent panels (own path/scroll); active panel = SOURCE, other =
default DEST.

**Spec**: optional `gtk::Paned` mode inside `opusdm-lister` hosting two file
views sharing the toolbar; focus click switches active panel; palette "Toggle
Dual Pane". Acceptance: copy in dual mode targets the inactive panel.

### F-28 Icon mode and Icon Action mode listers
**Original** (`Lister - View As`, `Set Mode`): listers can display as icons
(Workbench style) or "icon action" (icons + toolbar/functions), with
"Show All" pseudo-icons for iconless files.

**Spec**: add view-mode switch (Name / Icons) to the lister reusing
`opusdm_iconview` widgets already used on the desktop; icon mode supports
selection, drag&drop and all toolbar ops. Acceptance: switch a lister to icon
mode, copy via drag to another lister.

### F-29 Extended key selection (type-to-find)
**Original** (`Environment - Miscellaneous`): typing letters over the list
opens a small incremental string gadget to jump to a filename prefix.

**Spec**: typeahead overlay in the lister: printable keys open a mini entry,
live-jump selection to the longest matching prefix, Esc/timeout dismisses.
The vim single-letter keys were removed, so it is always on. Acceptance: typing `car` selects
`Cargo.toml`.

### F-30 DiskInfo / DeviceList equivalent
**Original** (`DiskInfo`, `DeviceList`): device/volume list view; disk info
(space used/free, errors).

**Spec**: virtual location `devices://` listing mounts (`/proc/mounts` +
`statvfs`: fs type, size, free) openable in any lister; "Disk Info" palette
command shows a requester for the current filesystem. Acceptance: open
`devices://`, double-click a mount to navigate into it.

### F-31 Reselect
**Original** (`Reselect`): re-select the entries the previous command
deselected.

**Spec**: keep the last selection set per lister; palette "Reselect"
(`Ctrl+Shift+A`). Acceptance: copy (which clears selection), reselect, same
set is selected again.

### F-32 Lister status-bar customization
**Original** (`Environment - Lister Display` "Status Text",
`Lister File Count`): status bar content defined by a format string with
info codes.

**Spec**: `settings.json` `status_format` with codes (`%d` dirs, `%f` files,
`%b` bytes, `%s` selected, `%p` path); header stat bar renders it.
Acceptance: `%f files, %b` renders accordingly.

---

## 5. Desktop, main window, themes

### F-33 LeaveOut (icons left out on the desktop/main window)
**Original** (`LeaveOut`, `Icons - Leave Out`/`Put Away`): drop permanent
shortcut icons for any file/dir onto the main window/desktop; arrow badge
marks them.

**Spec**: left-out entries are stored in `~/.config/opusdm/leftout.json`
(`{"items":[{"path":...,"x":...,"y":...}]}`, position optional and saved per
path once dragged), merged by the hub into the desktop icon field with an arrow
badge. Entry points are the Lister context menu "Leave Out on Desktop", the
`Leave Out` / `Put Away` function steps, and "Put Away" in the desktop icon
menu. Acceptance: leave out `/etc/fstab`, restart hub, icon persists.

### F-34 Program Groups
**Original** (`Program Group`, `Icons - New Group`): group icons collect
related programs/files; dropping icons onto the group adds them; opening
shows a group window.

**Spec**: a group is one JSON file, `~/.config/opusdm/groups/<id>.json`
(`{"name":...,"items":[paths],"x":...,"y":...}`), shown as a folder icon with a
group badge whose position is saved once dragged. Opening it shows a group
window; double click on a member launches it (folder in a Lister, `.desktop`
application, executable, default application). "New Group..." is in the desktop
menu; dropping files on the group icon or on its window adds them. Acceptance:
create group, drop 3 apps, open group, launch from it.

### F-35 Full themes (images + sounds + fonts), Build/Load/Save theme
**Original** (`LoadTheme`, `SaveTheme`, `BuildTheme`, `ConvertTheme`,
`Environment - Backgrounds`): a theme bundles colours, fonts, background
pictures (desktop / lister windows / requesters, with tile/center/stretch
and random-pick from a pattern) and sounds; BuildTheme packs a theme for
distribution.

**Spec**:
- Extend the current colour presets into a `ThemePackage` (zip under
  `~/.config/opusdm/themes/`): `theme.json` (colours - reuse existing keys),
  optional `backgrounds/{desktop,lister,requester}.*`, `sounds/*`, `fonts`.
- Lister/requester background images via a CSS provider; desktop background
  already exists - reuse `DesktopSettings`.
- Settings UI: import/export theme package (BuildTheme equivalent).
Acceptance: export current settings as a theme zip, wipe config, import,
colours+background restored.

**Update 2026-10-02**: the theme is now the only source of the look (colours,
appearance, fonts, Lister / requester pictures, icons, sounds, bundled
wallpapers), edited only in the Theme Editor. The desktop background, light /
dark, icon grid and behaviour are user preferences in the Settings window; a
theme can bundle wallpapers in `wallpapers/`, offered by "From theme...". See
`docs/superpowers/specs/2026-10-02-theme-single-source-design.md`.

### F-36 Sound Events
**Original** (`Environment - Sound Events`): assign a sound file + volume to
each script event (F-05), with test button and exclusive startup/shutdown
sounds.

**Spec**: `sounds.json`: `{ event, file, volume }`; hub plays via subprocess
on each emitted event; settings UI tab with file picker, volume slider, Test.
Acceptance: assign a wav to "lister opened", hear it.

### F-37 SetAsBackground command
**Original** (`SetAsBackground`): install any picture as desktop/lister/
requester background with tile/center/stretch options; designed for filetype
context menus ("set this image as background").

**Spec**: internal command `SetBackground` (params `path`, optional `fit`)
updating `Settings.theme.background`, plus a Lister context menu entry on image
files; the filetype action arrives with F-06. Acceptance: right-click a PNG ->
set as background applies live.

### F-38 Backdrop / hide system
**Original** (`Project - Backdrop`, `Hide`, `Reveal`,
`Environment - Hide Method`): the whole environment hides to a clock, an
AppIcon, a hotkey-only state, etc.; Reveal brings it back.

**Spec**: internal commands `Hide`, `Reveal` and `Toggle Hide` (usable from
any function, user menu entry or global hotkey) and a "Hide All" / "Reveal"
button in the desktop menu. Hidden means every Lister minimized, panel, dock and
group windows hidden; the desktop stays as the backdrop. Only windows the hub
minimized are restored on reveal. Acceptance: a global hotkey bound to a
`Toggle Hide` function toggles hidden and revealed with all windows restored.

---

## 6. Network

### F-39 FTP / FTPS / SFTP module
**Original** (`FTPConnect`, `FTPAddressBook`, `FTPAdd`, `FTPCommand`,
`FTPOptions`, `FTPQuit`, `FTPSetVar`): remote sites appear as lister paths;
address book with per-site protocol (FTP, explicit FTPS with optional cert
verification, SFTP with known-hosts file), passive mode, keep-alive, logging;
URL syntax `ftp://`, `ftps://`, `sftp://` typed into the path bar; raw
command send; global quit.

**Spec**:
- New crate `crates/ftp` implementing a virtual filesystem adapter behind the
  same "archive-like" path abstraction the lister already uses for archives:
  `sftp://user@host/path` etc. (Rust crates: `suppaftp` for FTP/FTPS, `ssh2`
  for SFTP).
- Address book `~/.config/opusdm/ftp.json` (host, port, user, protocol,
  passive, TLS verify); known-hosts file for SFTP fingerprints with
  first-connect approval requester.
- Copy from remote lister to local DEST streams through a hub job (like
  archive extraction does today).
- Acceptance: connect to a local test SFTP server, browse, copy a file to a
  local DEST lister.

---

## 7. Miscellaneous commands

| ID | Command(s) | Notes / Spec summary |
|---|---|---|
| F-40 | `Confirm` | Requester step usable inside functions (F-01): aborts the function unless confirmed. |
| F-41 | `Busy` / progress | Per-function busy indicator; hub already has job progress - expose "function running" state in the lister. |
| F-42 | Internal `CLI` | The terminal drawer runs a shell; DOpus5's CLI instead interprets *internal* commands. Spec: palette "Command Line" opens a prompt that parses internal commands + functions with `{...}` codes (prefix `!` for shell). |
| F-43 | `ScanDir` options | Navigate command with NEW (new lister), MODE (name/icon), CONTAINER (open parent, select child). Spec: extend path-bar/palette "Open..." accordingly. |
| F-44 | `Root` / `Parent` variants | "Open parent in NEW lister" command. |
| F-45 | `GetSizes` FORCE | Re-scan even cached directory sizes (clear cached sizes). |
| F-46 | `Run` | Run an external program asynchronously from a function, with output window option. |
| F-47 | Print / PrintDir | Print selected text files or the current listing. Spec: format requester (margins, header/footer with title/date/page) -> `lp` or PDF-to-file via GTK print dialog. |
| F-48 | `Assign` | Amiga assigns have no Linux equivalent; nearest analogue: **bookmarks/aliases** (`name -> path`) usable in the path bar (`foo:file.txt`). Optional, low priority. |
| F-49 | `ViewFont` | Font previewer. Spec: tiny dialog previewing system fonts via Pango; low priority. |
| F-50 | Custom screen/panel title | Format codes in the panel title (`%t` time, CPU, mem...). Spec: panel label format string in settings, refreshed on a timer. |

---

## 8. Amiga-only features - intentionally out of scope

These exist in DOpus 5 but have no meaningful Linux/GTK mapping. Do not
implement; listed to make the gap analysis complete.

- `AddIcon`, `FixIcons`, `IconInfo`, `Icons - Snapshot/UnSnapshot`,
  `Settings - Create Icons`: all revolve around Amiga `.info` icon files.
- `Format` / `Format Requester` / `Diskcopy`: track-device floppy formatting
  and raw disk copying (use system tools instead).
- `Environment - Display` (screen modes, custom public screens, palette
  remapping, pen precision), `Environment - Palette`: Amiga chipset/screen
  concepts.
- `Environment - WB Emulation`, Workbench position sync (`SYNCWBTOOPUS`...):
  Workbench interop.
- NewIcons settings, icon borders/labels bits, `dd_CurrentX` fixes: `.info`
  internals.
- `LoadFonts` (Amiga font prefs), `AnsiRead` as separate binary (covered by
  F-16 instead).
- ARexx port: replaced conceptually by the Unix-socket IPC hub + internal
  CLI (F-42). If scripting is desired later, expose the hub socket protocol
  as the public API.

---

## 9. Suggested implementation order

Grouped by dependency and value:

1. **Function system core** (F-01) - foundation for F-02, F-03, F-04, F-05,
   F-06, F-40, F-46.
2. **Hotkeys + user menus + scripts** (F-03, F-04, F-05) - cheap once F-01
   exists.
3. **File operation variants** (F-07, F-08, F-09, F-10, F-12, F-13, F-15,
   F-31) - hub/fsops extensions, high daily-use value.
4. **Lister formats & patterns** (F-22, F-23, F-24, F-29, F-32) - per-lister
   state, independent of the function system.
5. **Search trio** (F-19, F-20, F-21) + viewer upgrades (F-16, F-17).
6. **Desktop features** (F-33, F-34, F-37, F-38).
7. **Themes & sounds** (F-35, F-36, F-50).
8. **Button banks** (F-02) - sizable UI work, needs F-01.
9. **Dual lister & icon-mode listers** (F-27, F-28), tiling/snapshot
   (F-25, F-26), devices view (F-30).
10. **Network** (F-39), **Print** (F-47), encryption (F-14) - independent,
    schedulable any time.
