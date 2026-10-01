# TODO.md - OpusDM roadmap checklist

Progress tracker for the feature gap described in [FEATURES.md](FEATURES.md).
Each `F-xx` ID refers to the matching section of that file, which holds the
"Original" behaviour and the implementation "Spec".

Legend: `- [x]` done, `- [ ]` to do.

---

## 1. To do now

Work on these in order. Decide what comes next only when all of them are done.


## 2. To do later

To be prioritised after section 1 is finished.

- [ ] F-35 Full themes (images + sounds + fonts), Build/Load/Save theme (import/export)
- [ ] F-30 DiskInfo / DeviceList equivalent (`devices://`)
- [ ] F-18 Play (sound files) + beep/flash commands
- [ ] F-36 Sound Events
- [ ] F-50 Custom panel title format codes
- [ ] F-28 Icon mode and Icon Action mode listers
- [ ] Minor commands (F-40..F-49):

## 3. Not classified yet

Left over from the previous list and not mentioned in the latest prioritisation.

- [ ] F-11 File comments
- [ ] F-01 follow-up: optional arguments `{x!}` (verify/implement)
- [ ] F-01 follow-up: function flags (run async, wait, windowed output)
- [ ] F-01 follow-up: more internal commands beyond Copy/Move/Delete/MakeDir/NewFile/Rename

## 4. Will NOT be done

Decided out of scope. Do not implement.

- F-27 Dual Lister (split pane in one window)
- F-25 Lister tiling / cascade
- F-26 Lister Snapshot / window geometry persistence
- F-47 Print / PrintDir

Amiga-only features are also out of scope, see section 8 of [FEATURES.md](FEATURES.md).

## 5. Completed

- [x] F-49 ViewFont (low priority) (system fonts only; no font file preview)
- [x] F-48 Assign equivalent (bookmarks/aliases, low priority) (bookmarks usable as name: in the path bar; local folders only)
- [x] F-46 Run (external program from a function, output window) (internal command; output window per run)
- [x] F-45 GetSizes FORCE (size cache; also reused on reload)
- [x] F-44 Root / Parent variants ("Open parent in NEW lister") (palette commands)
- [x] F-43 ScanDir options (NEW, MODE, CONTAINER) (options on ChangeDir; mode and select do not apply to a NEW Lister)
- [x] F-42 Internal CLI (command line interpreting internal commands) (palette prompt; no history navigation)
- [x] F-41 Busy / function running indicator (per Lister, for runs it started)
- [x] F-40 Confirm (requester step inside functions) (palette-less: it is a function step; headless runs fail the step)
- [x] F-01 Function Editor and internal command language (core: data model, placeholders, `functions.json`, hub execution, editor UI, Command Palette)
  - [x] F-01 follow-up: per-function hotkey (done through hotkeys.json, F-03)
  - [x] F-01 follow-up: conflict prompt for Copy/Move steps (Delete never conflicts; runs with no Lister to ask still skip)
- [x] F-02 Button Banks + Button Bank Editor (open under the mouse, Start Menu variant and the Lister toolbar as a bank are not implemented)
- [x] F-03 Hotkey Editor (local + system-global hotkeys, KeyFinder)
- [x] F-39 FTP / FTPS / SFTP (browse, download, upload, remote Copy/Move/Delete/New Folder/New File with cancel; no remote rename, keep-alive, logging or raw commands; the Lister UI was not exercised by hand)
- [x] F-04 User menus in listers
- [x] F-05 Script events (system events only; viewer overrides wait for F-16, Hidden/Revealed for F-38)
- [x] F-06 Filetypes system (matching rules, double click and Open With actions, editor, Filetype Creator; the `drop` action is stored but not wired, no MIME sniffing: unmatched files use xdg-open)
- [x] F-07 CopyAs, MoveAs, Duplicate, UPDATE/NEWER/MOVEWHENSAME (`quiet` flag is carried but has no requester to suppress yet)
- [x] F-08 CheckFit (space pre-check; requester only, no status-bar Y/N badge)
- [x] F-09 Protect (chmod requester; chown not implemented, needs privileges)
- [x] F-10 DateStamp (touch)
- [x] F-12 MakeLink / MakeLinkAs (hard and symbolic)
- [x] F-13 Join / Split
- [x] F-14 Encrypt / Decrypt (palette only; no internal commands for functions yet; the password is not zeroized in memory)
- [x] F-40 Confirm (requester step inside functions)\n- [x] F-15 Recursive filter for operations (Copy/Move/Delete now; Protect/DateStamp/Comment hook in when they exist)
- [x] F-16 Full Reader: Read / AnsiRead / HexRead / SmartRead
- [x] F-17 Show (image slideshow)
- [x] F-19 FindFile (search by name across tree)
- [x] F-20 Search (content search)
- [x] F-21 Advanced Select requester
- [x] F-22 Lister Format Editor (per-directory formats; up/down buttons instead of drag and drop, no Comment column until F-11)
- [x] F-23 Show/Hide name patterns (persistent filter)
- [x] F-24 Directory buffer cache + CacheList + PathHistory
- [x] F-29 Extended key selection (type-to-find; always on, the vim single-letter keys were removed; Copy/Move are now F5/F6 and Delete is Del)
- [x] F-31 Reselect
- [x] F-32 Lister status-bar customization
- [x] F-33 LeaveOut (icons left out on the desktop; position saved only for left-out icons, primary monitor only, no drag and drop entry point yet)
- [x] F-34 Program Groups (groups are created from the desktop menu and filled by drag and drop; no Lister menu entry, primary monitor only)
- [x] F-37 SetAsBackground command (palette use needs a saved function with `SetBackground {f}`; the filetype action waits for F-06)
- [x] F-38 Backdrop / hide system (Hide, Reveal and Toggle Hide commands plus a desktop menu button; the clock and AppIcon hide methods are not implemented)
