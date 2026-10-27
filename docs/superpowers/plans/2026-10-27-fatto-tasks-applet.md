# Fatto Tasks applet Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A read only `fatto-tasks` applet (workspace and board dropdowns, open cards list, card detail window) plus the three applet system extensions it needs: `opus.http.post`, the `dropdown` widget and `opus.ui.openWindow`.

**Architecture:** The applet talks JSON-RPC 2.0 to `POST <baseUrl>/api/mcp-kanban` with `Authorization: Bearer fatto_...` using `tools/call`. The extensions live in `crates/jscore` (shared HTTP) and `crates/applets` (separate process from the hub). One risky change per commit, in the order of the tasks.

**Tech Stack:** Rust (ureq, gtk4, boa), JavaScript applet.

**Spec:** `docs/superpowers/specs/2026-10-27-fatto-tasks-applet-design.md`

## Global Constraints

- `CLAUDE.md` of the repo applies: no behaviour change of existing code, `err_log!` on every failing early return, lifetime changes explained in the commit body ("who owns this object for the whole run, and what happens when it is gone?"), no push.
- English in code and comments, `-` never the long dash, constants UPPERCASE, a comment on every function, text files end with an empty line, `CHANGES.md` updated (with what was NOT verified).
- `opus.http.get` keeps its exact behaviour; `post` has the same limits (http/https only, 15 s default, 120 s clamp, 8 MB body, 5 redirects).
- API key never in logs, errors or URLs.

## Review Focus

- `post` with an empty body, a non-UTF-8 reply, a 401/500 status (a response, not an error).
- `dropdown` with `items: []`, `selected` out of range, `update` that sets `items` and `selected` together.
- A secondary window closed by the user while a request is in flight (the answer must not touch the dead window, and must log).
- Board with no open cards, workspace with no boards, wrong or missing key.
- A card whose optional fields (assignees, due date, tags, checklists) are missing or null.

---

### Task 1: `opus.http.post`

**Files:**
- Modify: `crates/jscore/src/http.rs` (tests in the same file), `crates/applets/src/runtime.rs` (op table next to `("http.get", op_http_get)`), `crates/applets/src/prelude.js` (`opus.http.post` next to `get`), `crates/hub/src/scripting.rs` (op table and prelude, same as `get`)

**Interfaces:**
- Produces: `pub fn post(req: &HttpRequest) -> Result<HttpResponse, String>`; `HttpRequest` gets `pub body: Option<String>` read from the JSON field `body` (string, optional) by `request_from_json`. JS: `opus.http.post(url, {headers, body, timeoutMs})`.

- [ ] **Step 1: Write failing tests** in `http.rs`: `post_sends_body_and_headers` (local `TcpListener` echoing the received head and body, assert method POST, `Content-Type` and body arrive), `post_404_is_a_response_not_an_error`, `post_refuses_schemes_other_than_http_and_https`, `request_from_json_reads_body`, and `get_still_ignores_body` (a `body` on a get request is not sent).
- [ ] **Step 2: Run** `cargo test -p opusdm-jscore http` - expected: FAIL (`post` missing).
- [ ] **Step 3: Implement** `post` sharing the agent/response code with `get_with_limit` (extract a private `_send` taking the method; do not change `get` behaviour).
- [ ] **Step 4: Wire** the op in applets (`op_http_post`, same worker thread and Promise path as `op_http_get`) and in the hub scripting (`op_http_post`, synchronous). Add runtime tests in `crates/applets/src/runtime/tests.rs`.
- [ ] **Step 5: Run** `cargo test -p opusdm-jscore -p opusdm-applets -p opusdm-hub` and `cargo clippy` - expected: PASS.
- [ ] **Step 6: Commit** `jscore: opus.http.post` (only the changed files).

### Task 2: `dropdown` widget

**Files:**
- Modify: `crates/applets/src/ui_tree.rs` (+ `ui_tree/tests.rs`), `crates/applets/src/widgets.rs` (+ `widgets/tests.rs`), `crates/applets/src/prelude.js` (handler replacement for `onChange` if the prelude lists handler props)

**Interfaces:**
- Produces: `Kind::Dropdown { items: Vec<String>, selected: Option<usize>, on_change: Option<u32> }`; JS node `{ type: "dropdown", id, items: string[], selected?: number, onChange?(index) }`; `opus.ui.update(id, { items, selected })`. A value set by `update` never calls `onChange` (same rule as `entry`/`switch`).

- [ ] **Step 1: Write failing tests**: parse (`items` required array of strings, `selected` out of range is an error naming type, id and prop, absent means none), build (a `gtk::DropDown` over a `StringList`), `onChange` fires with the index on user selection, `update` of `items` and `selected` does not fire `onChange`, `items: []` works.
- [ ] **Step 2: Run** `cargo test -p opusdm-applets ui_tree widgets` - expected: FAIL.
- [ ] **Step 3: Implement** in `ui_tree.rs` and `widgets.rs` following the `Switch`/`List` code paths (`View`, `Change`, `apply_change`).
- [ ] **Step 4: Run** `cargo test -p opusdm-applets` and `cargo clippy` - expected: PASS.
- [ ] **Step 5: Commit** `applets: dropdown widget`.

### Task 3: `opus.ui.openWindow`

**Files:**
- Modify: `crates/applets/src/prelude.js`, `crates/applets/src/runtime.rs` (new `Event` variants for open/update/close of a secondary window), `crates/applets/src/applet_window.rs` and/or `crates/applets/src/host.rs` (owns the secondary windows), tests in `runtime/tests.rs` and a GTK test where the existing ones allow it.

**Interfaces:**
- Produces: JS `opus.ui.openWindow({ title, width, height, tree, onClose })` returning `{ id, update(nodeId, props), close() }`; at most `MAX_SECONDARY_WINDOWS = 8` per applet (an error beyond it); node ids are scoped to their window.
- Consumes: Task 2 not required; uses `widgets::build` and `Dispatch`.

- [ ] **Step 1: Write failing tests**: a secondary window is alive after N main loop iterations and after a refresh of the main tree (rule 5); its handlers fire after N events; `close()` and the window close button call `onClose` once and release the handlers; stopping or reloading the applet closes all its secondary windows; the 9th window is an error; an `update` on a closed window logs with `err_log!` and does not touch the main tree.
- [ ] **Step 2: Run** the new tests - expected: FAIL.
- [ ] **Step 3: Implement.** Ownership: the applet's window object owns its secondary `gtk::Window`s (strong refs in a map by window id); handlers hold `Weak` and a failed upgrade logs with `err_log!`. Handler ids of secondary trees are released on close.
- [ ] **Step 4: Run** `cargo test -p opusdm-applets` and `cargo clippy` - expected: PASS.
- [ ] **Step 5: Run the app** (`run-x11.sh` sandbox) with a tiny test applet that opens a window, closes it, reloads; note what could not be checked.
- [ ] **Step 6: Commit** `applets: opus.ui.openWindow`, body answering the ownership question.

### Task 4: the `fatto-tasks` applet and docs

**Files:**
- Create: `applets/fatto-tasks/applet.json`, `applets/fatto-tasks/main.js`
- Modify: `docs/usage.md` (applet API and widget tables, new applet), `CHANGES.md`

**Interfaces:**
- Consumes: `opus.http.post`, `dropdown`, `opus.ui.openWindow`, `opus.applet.settings` (`baseUrl`, `apiKey`), `opus.applet.open`.

- [ ] **Step 1: Discover the real tool shapes** with a live `tools/list` and `tools/call` (`curl` with the owner's key if provided; otherwise from `crates`-independent fatto source `liwe3/modules/mcpkanban/backend/src/tools.ts`); write the field mapping at the top of `main.js` as comments.
- [ ] **Step 2: Write `main.js`** as small pure functions (`rpc(tool, args)`, `openCards(columns, cards)`, `cardDetailTree(card)`, `loadWorkspaces/Boards/Cards`) plus the UI. Pure functions are tested by a `runtime/tests.rs` case that runs the script with a fake `opus.http.post` (no GTK).
- [ ] **Step 3: Handle states**: no key, 401, network error, empty workspace/board, no open cards; errors go to a label and `console.error` (never include the key).
- [ ] **Step 4: Run** `cargo test -p opusdm-applets` - expected: PASS.
- [ ] **Step 5: Run the real thing** in the sandbox or live session with a real key if the owner provides one; test workspace change, board change, refresh, detail window open/close. What cannot be checked is written plainly in `CHANGES.md`.
- [ ] **Step 6: Commit** `applets: fatto-tasks`.
