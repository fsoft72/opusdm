# JavaScript scripting (Boa): design as built

Date: 2026-10-15. Branch: `scripting`.

## Goal

Let the user write scripts in JavaScript that drive OpusDM: run the internal
commands, read and move the Listers, work with files, ask questions. The
user asked for:

- a window that lists the scripts;
- selecting, editing and deleting a script;
- an editor with JavaScript syntax colouring;
- the internal commands, the OpusDM objects and the Listers exposed to the
  scripts.

Decisions taken with the user:

| Question | Choice |
|---|---|
| Where the engine runs | In the **hub**, on the Function executor's thread |
| Storage and launch | One `.js` file per script in `~/.config/opusdm/scripts`; a new **Run Script** internal command, so scripts run from buttons, hotkeys, menus, Script Events, the palette and the Command Line |
| Editor | `gtk::TextView` + the syntect highlighter of the viewer (no new system library) |
| API shape | Directory Opus like, **synchronous**: a call returns when the hub is done |

The engine is [Boa](https://boajs.dev) (`boa_engine` 0.22, default features
off: no `Temporal`, no `Intl`). It is pure Rust, so it adds no system
dependency.

## Pieces

| Crate / file | Role |
|---|---|
| `ipc/src/scripts.rs` | Storage: `scripts_dir`, `list`, `load`, `save` (written aside then renamed), `delete`, `rename`, `validate_name`, `description` (leading `//` lines), `template`, and `API` (the reference shown by the editor) |
| `ipc/src/function.rs` | `InternalCmd::Script` ("Run Script"), parameters `script` (`ParamKind::Script`, required) and `args` (template, optional) |
| `ipc/src/funcref.rs` | `FuncRef::Script { name }`, reference string `script:<name>` |
| `hub/src/functions.rs` | The per-step `match` is now `run_step`, shared by the step loop and by scripts; new `FnEvent::ScriptQuery` |
| `hub/src/scripting.rs` | The engine: `run_script`, the native `__opusHost(op, payload)`, the dispatch of every call |
| `hub/src/scripting/prelude.js` | The `opus` object, written in JavaScript on top of `__opusHost` |
| `hub/src/server/function_run.rs` | `script_query` answers `ScriptQuery`; `only_scripts` lets a script-only run start without a SOURCE Lister |
| `highlight/` (new crate) | The syntect highlighter, moved out of the viewer, with the dark and light palettes |
| `ui/src/line_numbers.rs` | The line number gutter, moved out of the viewer |
| `ui/src/reference_picker.rs` | New **Script** kind: a drop-down of the saved scripts |
| `lister/src/scripts_ui.rs` | The Scripts window (`opusdm-lister --scripts`) |
| `lister/src/script_editor.rs` | The editor window |
| `lister/src/function_editor.rs` | The `Script` parameter is a drop-down of the scripts |

## How a script runs

```text
Lister / tool window ──ToHub::RunFunction──▶ hub ──spawn_executor──▶ executor thread
                                                                    │ run_step(Script)
                                                                    ▼
                                                     scripting::run_script
                                                     Boa Context + prelude.js
                                                                    │ opus.command(...)
                                                                    ▼
                                                     functions::run_step (same code as a Function step)
                                                                    │ FnEvent::Job / Confirm / ScriptQuery ...
                                                                    ▼
                                                     hub main loop answers, the script goes on
```

1. A `Script` step loads `<name>.js` (or, from the editor's Run, the unsaved
   text passed in the hidden `code` argument), expands `args` with the
   placeholders (`{f}`, `{o}`, ...) and runs it in a fresh Boa `Context`.
2. The prelude defines `opus` and `console`. Every `opus` call goes through
   `__opusHost(op, payload)`: payload and answer are JSON
   (`JsValue::to_json` / `from_json`), a failure becomes a thrown `Error`
   that the script can catch.
3. Commands run through `functions::run_step` on the script's SOURCE and
   DEST, so they behave exactly as Function steps (job engine, progress
   window, conflict prompts, Lister refresh).
4. What only the hub knows is asked with `FnEvent::ScriptQuery` and a reply
   channel (like `Confirm`): `Listers`, `Functions`, `GoTo { lister, path }`,
   `Refresh { path }`. The script waits up to 10 s for the answer.
5. A `ChangeDir` done by the script moves the SOURCE folder of the next
   steps of the Function too.
6. `run_jobs()` runs after the script, so `async` code settles before the
   step ends.

### Guards

- Each running script has a `Host` on a thread-local stack; a native call
  copies what it needs out of it, so a command that runs another script
  never meets a held borrow.
- Scripts nest at most `MAX_DEPTH` (8) deep.
- A cancelled run (its Lister closed) fails at the next `opus` call;
  `opus.sleep` checks every 50 ms.
- `opus.fs.read` refuses files over 64 MiB.
- A run with only `Script` steps starts without a SOURCE Lister, in `$HOME`
  with an empty selection; every other run still needs one.

### Known limit

- Boa 0.22 reports a stale position for `ReferenceError: x is not defined`
  (the last call made, or `?:?`). The editor finds the first use of `x`
  instead; other errors carry a correct `name.js:line:col`.
- A script stuck in an endless loop that makes no `opus` call cannot be
  stopped (Boa has no interrupt); it only blocks its own executor thread.

## The `opus` API

All calls are synchronous. Relative paths are relative to the SOURCE folder,
`~` is the home folder.

### Listers

| Member | What it is |
|---|---|
| `opus.source`, `opus.dest` | The SOURCE / DEST Lister or `null`: `path`, `selected` (full paths), `selectedNames`, `selectedFiles`, `selectedDirs`, `role` |
| `opus.listers` | Every open Lister: `id`, `role` (`source`, `dest`, `none`), `path` (the selection is known for SOURCE and DEST only) |
| `lister.go(dir, {mode})` | Show a folder; on the SOURCE it is a `ChangeDir` (it also moves the folder of the next commands), on another Lister a `GoTo` |
| `lister.files(pattern)` | `opus.fs.list` of its folder |
| `lister.refresh()` | Read its folder again |

### Commands

| Member | What it does |
|---|---|
| `opus.command(name, params)` | Run an internal command, by name (`CopyAs`), label (`Copy As`) or alias. `params` keys must be the command's parameters; `true` turns a checkbox on, `false` / `null` leave it off, numbers and lists become text. `files` replaces the SOURCE selection for this command, `to` the DEST folder |
| `opus.commands` | Every command: `name`, `label`, `params [{key, label, required, kind}]` |
| `opus.runFunction(nameOrId)` | Run a saved Function's steps on the script's SOURCE / DEST |
| `opus.functions`, `opus.scripts` | The saved Functions (`id`, `name`) and scripts (`name`, `description`) |
| `opus.runScript(name, args)` | Run another script |
| `opus.run(commandLine, {wait, window})` | The `Run` command |
| `opus.exec(program, args, {cwd, input})` | Run and capture: `{code, stdout, stderr}` |
| `opus.shell(script, options)` | `bash -c`, captured |
| `opus.newLister`, `setBackground`, `leaveOut`, `putAway`, `hide`, `reveal`, `toggleHide`, `desktop.next/prev`, `bank.open/close/toggle/hide` | Shortcuts for the matching internal commands |

### User, files, script

| Member | What it does |
|---|---|
| `opus.log(...)`, `console.log/info/warn/error` | One output line (objects as JSON) |
| `opus.confirm(message)` | Yes / no question, `true` for yes (a "no" does not fail the script) |
| `opus.sleep(ms)` | Wait |
| `opus.fs.read/write/append/list/stat/exists/isFile/isDir/mkdir` | Files; `list` and `stat` give `{name, path, isFile, isDir, isLink, size, modified, mode}` |
| `opus.path.join/dirname/basename/extname/normalize` | Path text helpers |
| `opus.args`, `opus.scriptName`, `opus.home`, `opus.version`, `opus.env(name)`, `opus.refresh(dir)` | Script context |

## User interface

### Scripts window (`opusdm-lister --scripts`)

Opened from the palette (**Scripts (JavaScript)...**, category Functions) or
the Settings window (Advanced tab). A tool process linked to the hub without
being a Lister, like Manage Functions.

- Filter field, list of scripts (name and description), output pane.
- **New...** (asks the name, writes the template, opens the editor),
  **Edit...** (or double click), **Run**, **Rename...** (refused while the
  script has unsaved changes in its editor), **Duplicate** (`name copy`,
  `name copy 2`, ...), **Delete** (asks; closes its editor),
  **Show Folder** (opens a Lister on the scripts folder), **Close** (closes
  the editors too, each asking about unsaved changes).
- The folder is watched (`gio::FileMonitor`): scripts added or changed by
  hand show up.
- Without a hub the scripts can still be edited; Run is disabled.

### Editor

- Code view: monospace, syntect JavaScript colouring (recoloured 120 ms after
  the last keystroke, dark or light palette after the GTK theme), line
  number gutter, `Ln / Col` in the toolbar, `*` in the title while unsaved.
- Keys: **Ctrl+S** save, **F5** / **Ctrl+Enter** run, **Tab** / **Shift+Tab**
  indent (4 spaces, whole lines for a multi-line selection), **Enter** keeps
  the indentation (one more level after `{ [ (`, and `{|}` opens on three
  lines), **Ctrl+/** comments lines in and out, **Ctrl+Z** undo (GTK).
- **Run** runs the text as it is (no need to save), with the Arguments
  field as `opus.args`; the output and errors (red) go to the pane below,
  and the line of an error is marked and gets the cursor.
- Reference panel with a filter: the `opus` API and one
  `opus.command(...)` entry per internal command with its parameters;
  double click inserts the snippet at the cursor.

### Using a script elsewhere

- Function Editor: add the **Run Script** internal step, pick the script,
  optional Arguments.
- Hotkeys, User Menu, Script Events, Button Banks, File Types: the
  reference picker has a **Script** kind (`script:<name>`).
- Command Line: `Script script=name args='{f}'`.

## Tests

- `ipc/scripts.rs`: names, description, save / list / load / rename /
  delete, template.
- `ipc/funcref.rs`: `script:` round trip.
- `hub/scripting/tests.rs` (17): log, context and arguments, thrown and
  syntax errors, commands and wrong parameters, Copy with `files` / `to`,
  `source.go`, Listers query and `dest.go`, files, `exec`, `confirm`,
  `runFunction`, `commands`, cancel, nesting limit, `async`.
- `hub/server/tests.rs` (2): a script from a tool window with no Lister; a
  script asking the hub for the Listers.
- `lister/script_editor.rs` (5): error positions, undefined names,
  indentation, command snippets, reference entries.
- `ui/reference_picker.rs`: Script kind.

Checked by eye on Xvfb with the real hub: the list, the editor (colours,
gutter, indentation, comment toggle), a run listing the SOURCE Lister's
files, a failing run marking the right line.
