# OpusDM Lister - Manual QA Checklist

Run under a nested X server: `./run-x11.sh` (needs `Xephyr`, `libgtk-4-dev`,
a built workspace). The hub opens its toolbar and the first Lister itself.

## Startup

- [ ] `./run-x11.sh` opens Xephyr with the hub toolbar and one Lister at `$HOME`.
- [ ] Toolbar is a small non-resizable strip; reads `SRC: -   DST: -`.
- [ ] `New Lister` opens another Lister; each is its own process (`pgrep -a opusdm-lister`).
- [ ] Launching `opusdm-lister` directly (no `OPUSDM_HUB_SOCK`) prints guidance and exits 1.
- [ ] Second `opusdm-hub` invocation while one runs: prints "already running", exits 0, raises the toolbar.

## Navigation

- [ ] Lists directories first, then files; name, size, date columns render.
- [ ] Double-click a directory navigates in; title and path bar update.
- [ ] `Parent` goes up; `Root` goes to `/`; `Refresh` re-reads.
- [ ] Typing a valid path + Enter navigates; an invalid path is rejected without navigating.
- [ ] Size header sorts by size; click again reverses; directories still lead.
- [ ] Selecting rows updates `N items, M selected, X` in the status line.
- [ ] Double-click a file opens it with the default application.

## Roles

- [ ] Focus Lister A -> it becomes SOURCE (badge + toolbar SRC).
- [ ] Focus Lister B -> B becomes SOURCE, A becomes DEST.
- [ ] `Source` / `Dest` / `Lock` toggle buttons reflect and drive hub state.
- [ ] `Lock` on A keeps A as SOURCE when B is focused.
- [ ] Clicking the toolbar SRC / DST indicator raises that Lister.

## Operations

- [ ] Select files in SOURCE, `Copy` -> they appear in DEST's directory; toolbar shows a progress row that clears; both Listers refresh.
- [ ] `Move` on the same filesystem is instant; the file leaves SOURCE.
- [ ] `Move` across filesystems copies then deletes.
- [ ] Copy onto an existing name -> conflict dialog; `Skip` keeps the old file; re-run and `Overwrite All` replaces it.
- [ ] `Delete` asks to confirm, then permanently removes; view refreshes.
- [ ] `MakeDir` / `NewFile` create the entry and refresh.
- [ ] `Rename` on a single selection renames in place.
- [ ] `Copy` with no DEST set shows "no DEST Lister set" in the status line, no job.
- [ ] A permission-denied file in a copy: job completes, toolbar row shows "N errors", click opens the error list.

## Drag and drop

- [ ] Drag a selection from Lister A onto Lister B's list -> copied into B's directory.
- [ ] Hold `Shift` while dragging -> moved instead.
- [ ] Dragging onto the same directory -> no duplication (hub rejects self-drop).
- [ ] A file name with spaces transfers correctly.

## Shutdown

- [ ] Close a Lister window -> only that process exits; hub logs it; toolbar stays open.
- [ ] Close a Lister that held SOURCE -> role clears; toolbar SRC shows `-`.
- [ ] Hub `Quit` with a job running -> confirm dialog; on confirm all Listers close and the socket file is gone (`ls $XDG_RUNTIME_DIR/opusdm/`).
- [ ] Hub `Quit` with no jobs -> all Listers close, socket cleaned, process exits.
- [ ] `kill -9` the hub -> every Lister exits within ~1s (reader hits EOF).
