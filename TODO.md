# TODO.md - OpusDM roadmap checklist

Progress tracker for the feature gap described in [FEATURES.md](FEATURES.md).
Each `F-xx` ID refers to the matching section of that file, which holds the
"Original" behaviour and the implementation "Spec".

Legend: `- [x]` done, `- [ ]` to do.

---

## 1. Command and function system

- [x] F-01 Function Editor and internal command language (core: data model, placeholders, `functions.json`, hub execution, editor UI, Command Palette)
  - [ ] F-01 follow-up: optional arguments `{x!}` (verify/implement)
  - [ ] F-01 follow-up: function flags (run async, wait, windowed output)
  - [x] F-01 follow-up: per-function hotkey (done through hotkeys.json, F-03)
  - [ ] F-01 follow-up: conflict prompt UI for Copy/Move/Delete steps (currently auto-skip)
  - [ ] F-01 follow-up: more internal commands beyond Copy/Move/Delete/MakeDir/NewFile/Rename
- [ ] F-02 Button Banks + Button Bank Editor
- [x] F-03 Hotkey Editor (local + system-global hotkeys, KeyFinder)
- [x] F-04 User menus in listers
- [x] F-05 Script events (system events only; viewer overrides wait for F-16, Hidden/Revealed for F-38)
- [ ] F-06 Filetypes system (classes, matching, actions, Filetype Creator)

## 2. File operations

- [x] F-07 CopyAs, MoveAs, Duplicate, UPDATE/NEWER/MOVEWHENSAME (`quiet` flag is carried but has no requester to suppress yet)
- [x] F-08 CheckFit (space pre-check; requester only, no status-bar Y/N badge)
- [x] F-09 Protect (chmod requester; chown not implemented, needs privileges)
- [x] F-10 DateStamp (touch)
- [ ] F-11 File comments
- [x] F-12 MakeLink / MakeLinkAs (hard and symbolic)
- [x] F-13 Join / Split
- [ ] F-14 Encrypt / Decrypt
- [x] F-15 Recursive filter for operations (Copy/Move/Delete now; Protect/DateStamp/Comment hook in when they exist)

## 3. Viewers and search

- [x] F-16 Full Reader: Read / AnsiRead / HexRead / SmartRead
- [x] F-17 Show (image slideshow)
- [ ] F-18 Play (sound files) + beep/flash commands
- [ ] F-19 FindFile (search by name across tree)
- [ ] F-20 Search (content search)
- [x] F-21 Advanced Select requester

## 4. Lister capabilities

- [x] F-22 Lister Format Editor (per-directory formats; up/down buttons instead of drag and drop, no Comment column until F-11)
- [x] F-23 Show/Hide name patterns (persistent filter)
- [x] F-24 Directory buffer cache + CacheList + PathHistory
- [ ] F-25 Lister tiling / cascade
- [ ] F-26 Lister Snapshot / window geometry persistence
- [ ] F-27 Dual Lister (split pane in one window)
- [ ] F-28 Icon mode and Icon Action mode listers
- [x] F-29 Extended key selection (type-to-find; always on, the vim single-letter keys were removed; Copy/Move/Delete are now F5/F6/F8)
- [ ] F-30 DiskInfo / DeviceList equivalent
- [x] F-31 Reselect
- [x] F-32 Lister status-bar customization

## 5. Desktop, main window, themes

- [ ] F-33 LeaveOut (icons left out on the desktop)
- [ ] F-34 Program Groups
- [ ] F-35 Full themes (images + sounds + fonts), Build/Load/Save theme
- [ ] F-36 Sound Events
- [ ] F-37 SetAsBackground command
- [ ] F-38 Backdrop / hide system

## 6. Network

- [ ] F-39 FTP / FTPS / SFTP module

## 7. Miscellaneous commands

- [ ] F-40 Confirm (requester step inside functions)
- [ ] F-41 Busy / function running indicator
- [ ] F-42 Internal CLI (command line interpreting internal commands)
- [ ] F-43 ScanDir options (NEW, MODE, CONTAINER)
- [ ] F-44 Root / Parent variants ("Open parent in NEW lister")
- [ ] F-45 GetSizes FORCE
- [ ] F-46 Run (external program from a function, output window)
- [ ] F-47 Print / PrintDir
- [ ] F-48 Assign equivalent (bookmarks/aliases, low priority)
- [ ] F-49 ViewFont (low priority)
- [ ] F-50 Custom panel title format codes

## 8. Amiga-only features

Intentionally out of scope, see section 8 of [FEATURES.md](FEATURES.md). Do not implement.

## 9. Suggested order

Follows section 9 of [FEATURES.md](FEATURES.md):

1. [x] Function system core (F-01, core only, see follow-ups above)
2. [x] Hotkeys + user menus + scripts (F-03, F-04, F-05)
3. [x] File operation variants (F-07, F-08, F-09, F-10, F-12, F-13, F-15, F-31)
4. [x] Lister formats and patterns (F-22, F-23, F-24, F-29, F-32)
5. [ ] Search trio (F-19, F-20, F-21) + viewer upgrades (F-16, F-17)
6. [ ] Desktop features (F-33, F-34, F-37, F-38)
7. [ ] Themes and sounds (F-35, F-36, F-50)
8. [ ] Button banks (F-02)
9. [ ] Dual lister, icon-mode listers, tiling/snapshot, devices view (F-27, F-28, F-25, F-26, F-30)
10. [ ] Network (F-39), Print (F-47), encryption (F-14)
