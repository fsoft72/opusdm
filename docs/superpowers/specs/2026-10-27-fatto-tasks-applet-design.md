# Fatto Tasks applet and applet system extensions - design

Date: 2026-10-27. Status: draft, waiting for the owner's approval.

## Intent

The owner wants to open a small window and see the **tasks still open on one
board** of their fatto instance (`fatto.os3.services`), without opening the web
app. The applet is read only. The owner picks a workspace, then a board, sees
the cards that are not in DONE, and clicks one to read its detail.

To build it, the applet system (`crates/applets`, see
`2026-10-26-applets-design.md`) and the shared script core (`crates/jscore`)
need three small extensions. They are useful to later applets too.

## Goals

- New applet `applets/fatto-tasks/` (`applet.json` + `main.js`), loaded like
  `site-news` and `weather-clock`.
- Settings: `baseUrl` (string, default `https://fatto.os3.services`) and
  `apiKey` (string, `fatto_...`, stored in plain text in the applet state file,
  as already accepted for string settings).
- Main window: a workspace dropdown, a board dropdown, a refresh button and a
  list of the open cards of the board. Changing the workspace reloads the board
  dropdown and the list. The last chosen workspace and board are remembered
  (builder's call how: a hidden setting or the state file).
- Open = card not in the DONE column. DONE is always the last column of a board
  and `done_at` is null outside DONE; the filter uses the column, not a name.
- Click on a card opens a **detail window** (read only): code (e.g. FATTO-12),
  title, column, assignees, due date, tags, description, checklists, and an
  "Open in fatto" button that opens the card in the browser. Fields that fatto
  does not return are simply not shown. (The exact field list is a guess of the
  design and is confirmed against the real `kanban_card_get` answer.)
- Errors (no key, 401, network, bad JSON) are shown in the window and logged
  with `err_log!`, never a blank window.
- Three extensions, below.

## Non-goals

- No writing: no moving cards, comments, timers, creating cards.
- No live updates (no SSE stream), no automatic refresh in v1 (refresh button
  only). A periodic refresh can come later.
- No change to the fatto server. API keys authenticate only
  `POST /api/mcp-kanban` (JSON-RPC 2.0, MCP protocol `2025-06-18`), not the
  normal REST routes (fatto spec `2026-10-22-mcp-kanban-design.md`). The applet
  uses that endpoint with `tools/call`: `kanban_workspace_list`,
  `kanban_board_list`, `kanban_column_list` (or `kanban_board_get_full`),
  `kanban_card_list`, `kanban_card_get`. The builder checks the real tool
  arguments and result shapes with `tools/list` and a live call.
- No Wayland-specific work (applets are X11 only in v1).

## Anti-goals (a failure even if it technically works)

- The extensions change the behaviour of existing applets, of the Lister
  scripts' `opus.http.get`, or of the hub, the Lister or the global hotkeys.
- A detail window that stays alive, or whose handlers keep running, after it was
  closed; or that closes when the main window refreshes.
- The API key in the log, in an error message shown in the window, or in a URL.
- A silent failure: any early return on a failure path logs.

## The three extensions

1. **`opus.http.post(url, {headers, body, timeoutMs})`.** Same result shape as
   `get` (`{status, ok, headers, body}`), same limits (http and https only, 15 s
   default timeout, clamped at 120 s, body at most 8 MB, up to 5 redirects). `body`
   is a string sent as is; the script sets `Content-Type`. Lives in
   `crates/jscore/src/http.rs` next to `get`, so the Lister scripts get it too
   (synchronous there, a Promise in applets, exactly like `get`). Existing `get`
   behaviour must not change. Tests like the existing `get` ones, plus the body
   and header being sent.
2. **Widget `dropdown`.** Props: `items` (array of strings), `selected` (index,
   -1 or absent for none), `onChange(index)`. `opus.ui.update(id, {items,
   selected})` changes it; as for `entry` and `switch`, a value set by `update`
   does not call `onChange`. Implemented in `ui_tree.rs` and `widgets.rs` with
   the same validation style (every error names type, id and prop) and tests.
3. **`opus.ui.openWindow({ title, width, height, tree })`.** Opens a secondary
   window of the applet with its own widget tree (same widgets, handlers run in
   the same Boa context of the applet). Returns a handle with `id`,
   `update(id, props)` and `close()`; also an `onClose` option. Closing it with
   the window button releases its handlers. Closing or reloading or disabling
   the applet closes its secondary windows. A limit on open secondary windows
   per applet (suggested 8) and an error beyond it. The ids of the secondary
   tree are separate from the main tree, so `opus.ui.update` of the main window
   is unaffected.

## Constraints

- All of `CLAUDE.md` applies. `opusdm-applets` is a separate process from the
  hub, so Lister, hub and hotkeys are not touched, but `jscore` is shared:
  `opus.http.get` and the Lister scripts must be checked after change.
- **Lifetime (rule 2).** `openWindow` is a lifetime change. The commit body
  answers "who owns this window for the whole run, and what happens when it is
  gone?". Suggested answer to verify: the applet instance owns the window
  objects; a window removes its handlers when closed; a `Weak` upgrade that
  fails logs with `err_log!`. A regression test: a secondary window is alive
  after N main loop iterations and after a main window refresh, and its
  handlers fire after N events (rule 5).
- One risky change per commit, in this order, each testable on its own:
  1. `opus.http.post` (jscore), 2. `dropdown`, 3. `openWindow`, 4. the applet.
- Run the real thing before saying done (rule 4): the applets process in the
  sandbox (`run-x11.sh`) or the live session, with a real fatto key if the
  owner gives one or can run it. What cannot be checked goes in `CHANGES.md`.
- Docs: `docs/usage.md` applet section (API table, widget table), `CHANGES.md`.
- Text and code in English, `-` instead of the long dash, constants in
  UPPERCASE, comments on every function.

## Left to the builder

- JSON-RPC envelope details (`initialize` is not needed for stateless calls:
  verify), pagination or limits of `kanban_card_list`, mapping column ids to
  "is DONE".
- Layout and style of the main and detail windows, the empty and error states.
- How the remembered workspace/board is stored.
- Sorting of the list (suggested: board order, column by column).
- Where the secondary window is placed and whether it is resizable.

## Risks

- `openWindow` ownership and the handler registry in `runtime.rs`
  (handler ids, `{ "handler": id }` replacement): a secondary window needs its
  own handler scope or a shared one cleaned on close.
- Boa promise handling for `post` in the applet runtime must match `get`.
- The API key ends up in `~/.local/state/opusdm/applets/fatto-tasks.json` in
  plain text (accepted earlier for string settings).
