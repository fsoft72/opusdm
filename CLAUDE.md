# OpusDM - rules for changes

OpusDM is PRODUCTION software, used every day as the desktop of the owner. A
regression in the Lister, the hub or the global hotkeys breaks the whole
session. Refactors, optimizations and "fixes" must never change behaviour that
works.

## Incident that must not happen again

Commit `5be25d9` ("stop closures keeping the Ui alive") turned every Lister
handler into a `Weak<Ui>`, but nothing owned the `Rc<Ui>` any more. After the
first listing the window was dropped: Enter in the path bar, double click on a
folder and all the global hotkeys silently did nothing (the hub waited for a
`QuerySelection` that the dead Lister never answered). Unit tests passed, the
build passed, clippy passed. The bug was found by the user, a day later.

## Rules

1. **Behaviour first.** An optimization, a refactor or a "robustness" change
   must keep every observable behaviour identical. If unsure, do not do it.
   Do not "improve" code that was not asked about.
2. **Lifetime and ownership changes are high risk** (`Weak`, `Rc`/`Arc`
   cycles, dropping handles, moving work to threads or workers, cancel tokens,
   channels, `glib::spawn_future_local` loops that `break` on failure). Before
   committing one, answer in writing in the commit body: *who owns this object
   for the whole run, and what happens when it is gone?* A `Weak::upgrade()`
   that fails must never be a silent `return`: log it with `err_log!`, so a
   dead object is visible.
3. **Silent failure is forbidden.** Any early return on a failure path in a
   handler, loop or worker must log (see `opusdm_ipc::err_log!`). A feature
   that "does nothing" with an empty log is the worst kind of bug here.
4. **Run the real thing before saying "done".** Unit tests are not enough for
   GUI, hub and IPC code. After changing `crates/lister`, `crates/hub`,
   `crates/ipc`, `crates/iconview`, `crates/ui` or the panel/dock, check the
   app in the running session (`scripts/live-update.sh --all`, ask the owner
   when it needs sudo) or in the sandbox (`run-x11.sh`), and test at least:
   - Lister: Enter in the path bar, double click on a folder, Backspace /
     parent button, Alt+Left/Right, F-keys.
   - Global hotkeys: Ctrl+backslash (terminal), Ctrl+Shift+space (run),
     Ctrl+Print (shot), Alt+1 / Alt+2 (desktops).
   - Hub: open a second Lister, SOURCE/DEST roles, a Copy job, a function run.
   If a check cannot be done, say so plainly in the final message and in
   `CHANGES.md`. Never write "fixed" or "works" for something not exercised.
5. **Add a regression test for every lifetime or wiring change** where
   possible (for example: the window is still alive after the first listing and
   after `main context` iterations; a handler still fires after N events). If
   a GTK test is impossible, add a startup self-check or a `dlog!` trace and
   explain how it was verified by hand.
6. **One risky change per commit, small and revertable.** Never batch many
   "OPTIMIZE.md" items in a merge without exercising the app between them.
   Parallel agent branches must be merged one at a time, with the app run in
   between, not all at the end.
7. **Do not trust a green CI.** `cargo test`, `cargo clippy` and `cargo build`
   cannot see "the handler is connected but its target is gone". Review the
   diff for ownership and event flow, not only for compile errors.
8. **When a performance or cleanup item is not clearly worth the risk,
   skip it** and note it in `OPTIMIZE.md` instead of doing it.

## Project conventions

- Keep `CHANGES.md` updated for every change (what was NOT verified too).
- Commit only the files you changed; never push.
