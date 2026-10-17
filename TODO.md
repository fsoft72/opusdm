# TODO items


- [x]  (done, needs a manual check in the desktop session) Select files / dirs and drag and drop them inside a folder in the same lister

  It shouuld be possible to select multiple files / folders and then drag and drop them
  into a folder in the same lister (without DEST)
  While dragging, the folder that can become the target should be highlighted

- [x] (done, checked in Xephyr; see CHANGES.md 2026-10-17) Opening a big tar.bz2 archive in lister has no feedback

  When trying to open a big tar.bz2 archive in lister, there is no feedback and the user
  doesn't know anything is happening.
  Create a new feature inside lister: a small progress window INSIDE the lister (centered
  vertically) with a message (that can change while operations proceed), progress bar (if possible) and a "Cancel" button to abort the task.
  The small progress window cannot be moved (borderless) and it moves along with the lister.
  When the small window is shown, all other buttons and event on the litser are muted


- [ ] Reload must not cancel a navigation to another folder

  `Ui::reload` calls `navigate`, which bumps the generation and cancels the previous read. If a watcher event,
  an `InvalidatePath` or a job-done reload fires while a slow folder B is loading, the result for B is dropped and the
  Lister stays in A with no message. With back/forward the history position has already moved, so the buttons can
  disagree with the folder on screen. Fix idea: a reload of the current folder must not supersede a pending navigation.

- [ ] Closing a Lister kills the programs of its `Run ... wait` steps

  Since `9bc12cd` `FunctionRuns::forget_connection` cancels the run and `wait_or_kill` kills the waited child
  (before, the run only stopped at its next step). An editor started with wait dies when the Lister that started the
  function is closed. Decide if this is wanted; if not, only stop the steps that have not started.

- [x] (done, checked in Xephyr; see CHANGES.md 2026-10-17; Reload is Ctrl+R) Failed thumbnails are not retried by a reload (F5)

  Since `4c24cc5` a failed thumbnail is retried only when the file mtime or the icon size bucket changes, or after
  leaving and re-entering the folder. The Lister uses only `refresh_entries`, so F5 does not clear the failed set.
  A missing or busy `ffmpeg` / `pdftoppm` leaves the generic icon until then. Fix idea: F5 clears the failed set.

- [x] (done, checked in Xephyr; see CHANGES.md 2026-10-17) An edited file can show its old thumbnail

  If a file is edited while its thumbnail request is pending, `begin()` refuses a new request and the old result is
  applied by path. The stale thumbnail stays until the next mtime change. Narrow race, low priority.

- [x] (done; the GTK-only branch cannot be exercised here, see CHANGES.md 2026-10-17) Markdown pictures in formats only GTK reads show `[image: alt]`

  Since `91135e0` the viewer decodes Markdown pictures with gdk-pixbuf on a worker. Formats that
  `gdk::Texture::from_filename` read but gdk-pixbuf does not (avif, jxl, heif) now show the alt text.

- [x] (done, checked in Xephyr; see CHANGES.md 2026-10-17) Run window: selection jumps to the first row when the PATH scan ends

  `crates/runner/src/ui.rs` calls `refresh()` when the scan finishes and text is already typed; it resets the
  selection to row 0, also when the user picked another row with the arrow keys. Keep the selected entry.

- [ ] Panel and dock size with a single GDK monitor and a bigger X screen

  `fit_to_screen` in `crates/ui/src/screen.rs` gives the only monitor the root window size. With `xrandr --fb`
  larger than the only active output, or a root that did not shrink after a monitor was removed, panel and dock get the
  wrong width or height. Does not happen when the framebuffer equals the monitor.

- [x] (done, unit tests only; see CHANGES.md 2026-10-17) network-config: an unrelated job failure cancels "Save then close"

  `crates/network-config/src/closing.rs:44`, called on any `Done::Failed`, clears `close_after_save`. If the user
  chose Save while closing and another job (for example a List) fails first, the save works but the window stays open.
  Only a failure of the save itself should clear the flag.

- [ ] Hub: one fit-check worker serves every Lister

  `64616dd` runs `CheckFit` on a single serial worker: a big size scan blocks the checks of all other Listers, and a
  panic of the worker leaves the requester with only "worker is gone". Consider one worker per Lister or a small pool.

- [ ] Archive scratch folders can be deleted too early or leak

  `keep_scratch(dests)` in `crates/lister/src/actions.rs:260` counts DEST Listers when the extraction ends, the hub
  reads them again at submit. A DEST added in between: the folder is deleted after the first `JobDone` while the second
  job still reads it. A DEST removed, or a remote DEST (`RemoteTransfer`, no `JobDone`): the folder stays until quit.

- [ ] Mixed hub / Lister versions: unknown messages close the window without a log

  `OutputLines` and `JobDone.tag` are new variants (protocol 2). An older Lister that cannot decode one used to end
  silently; it now logs the read error but still closes the window. Consider ignoring an unknown variant, and bump
  `PROTOCOL_VERSION` when a variant is added so the mismatch warning is accurate.

- [ ] Viewer: its `State` is kept alive only by a cycle of strong handlers

  `crates/viewer` `_wire_keys` and the other strong handlers keep `State` alive. It works, but if someone converts
  them to `Weak` the viewer dies like the Lister did. Give `State` an explicit owner (as `StartState.ui` in the Lister).

- [x] (audited 2026-10-17, no code changed; follow-ups below) Weak captures of the Lister dialogs were not audited

  `find_dialog.rs`, `scripts_ui.rs`, `script_editor.rs`: check who owns each `Weak` target for the whole feature.
  Result:
  - `find_dialog.rs`: `Dialog` is owned by the window's `connect_close_request` handler (the one strong clone), the
    `Dialog` does not own the window, no cycle. The weak handlers can upgrade for the whole life of the window. OK.
  - `scripts_ui.rs`: `Host` is owned by the Scripts window's `connect_destroy` handler (`keep`). `Editor`s are owned by
    `Host.editors` and removed by `editor_closed`. OK while the Scripts window is open.
  - `script_editor.rs`: `Editor.host` is `Weak<Host>` and every handler holds `Weak<Editor>`. When the Scripts window
    closes, its `close_request` closes the editors; an editor with unsaved changes asks Save / Discard / Cancel and,
    on Cancel, stays open while the Scripts window (and `Host.editors`) goes away. Checked in Xephyr: the editor still
    works (typing, Save, Ctrl+S wrote the file), so something else keeps it alive, but no explicit owner was found.
    See the follow-up below.
  - All three files return silently when a `Weak::upgrade()` fails (`if let Some(..) = weak.upgrade()`), against rule
    2 of `CLAUDE.md`. See the follow-up below.

- [ ] Script Editor: no explicit owner once the Scripts window is closed

  After Close on the Scripts window and Cancel on an unsaved editor, `Host` and `Host.editors` are gone and every
  handler of the editor holds a `Weak<Editor>`. It works today (checked in Xephyr), but by an owner nobody chose. Give
  the open editors an explicit owner for the whole run (or keep `Host` alive while an editor is open). Lifetime change:
  needs the owner's go-ahead and a check in the running session.

- [ ] Silent `Weak::upgrade()` failures in `find_dialog.rs`, `scripts_ui.rs`, `script_editor.rs`

  Each handler does `if let Some(x) = weak.upgrade()` with no `else`: a dead target is invisible in the log. Add
  `err_log!` on the failure path (logging only, no lifetime change).

- [ ] Icon view: the spacing grows after a watcher refresh

  In a folder in icon view, a change on disk that triggers `on_directory_changed` (e.g. `chmod` of a file) re-lays out
  the icons with a wider horizontal spacing; Ctrl+R puts the normal spacing back. Seen in Xephyr on master `0756545`
  as well, so it is older than the 2026-10-17 thumbnail fixes.

- [ ] Icon view: clicking an icon does not take the keyboard focus from the path bar

  After Enter in the path bar, a click on an icon (icon view) leaves the focus in the entry, so Backspace and Alt+Left
  edit the path text instead of going to the parent / back. The list view takes the focus. Seen on master `0756545` too.

- [ ] Flaky test: `opusdm_polkit::helper::tests::a_wrong_password_ends_in_an_error_and_failure`

  Failed once in `cargo test --workspace` with `Text file busy` (ETXTBSY: a test helper script is executed while
  another test thread still has a file open for writing). Passed on the next run.

- [ ] Path bar: typing fast while a folder is suggested can leave suggested text in the path

  Reported by the owner (it happens in daily use) and seen in Xephyr on 2026-10-17: typing a long path quickly in the
  path bar (a second Lister on `/ramdisk`, the path of a scratch folder typed with XTest at about 20 keys a second)
  ended with a text that looked right at both ends, but Enter did not go there and the bar went back to `/ramdisk`;
  the same path typed at about 8 keys a second, with Delete before Enter, worked. The look-ahead is
  `wire_path_completion` in `crates/lister/src/build.rs`: each keystroke starts a `pathcomplete::suggest` on a worker
  and the answer is inserted as selected text when the entry still holds the typed text with the cursor at its end.
  The `typing` flag is set on key press and cleared on key release, which can come out of order when keys overlap.
  To find: which interleaving of keystrokes, worker answers and the IM leaves suggested text that is not selected (and
  so is not replaced by the next character). Fix with a regression test on the decision logic (pull it out of the
  GTK handlers) and a check in Xephyr typing at full speed.
