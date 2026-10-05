# Inline commands: bash and exec wherever a Function can be used - design

Status: draft, pending user review 2026-10-05. Extends the Function system
(F-01) and its bindings (F-03 hotkeys, F-04 user menu, F-05 script events,
plus button banks and file types): wherever the user can pick a saved
Function, they can also type a `bash` command or an `exec` program line,
without first creating a Function for it.

## Goals

- In **Edit Hotkeys**, a row can run a saved Function, a bash command or an
  executable. A global hotkey that opens a terminal (`exec:kitty`) is the
  motivating case.
- The same choice is offered in every place that references a Function.
- Existing files keep loading and saving unchanged. No schema or protocol
  change.

## Non-goals

- A new function step type or a new `ToHub` message.
- Running an inline command from the Command Palette (it only lists saved
  Functions).
- Per-command working directory or environment settings.
- Phase 2 consumers are specified here but implemented after phase 1.

## Design

### Reference format

Every place that stores a `FunctionId` keeps a string. A reserved prefix
turns that string into an inline command; a string without the prefix is a
saved Function id, as today (saved ids are `fn-<hex>-<hex>`, so they can
never start with a prefix).

| Reference | Meaning | Output |
|---|---|---|
| `fn-...` (no prefix) | saved Function | as defined by the Function |
| `bash:<cmd>` | `bash -c "<cmd>"` | none |
| `bash+out:<cmd>` | `bash -c "<cmd>"` | shown in a window |
| `exec:<line>` | start the program directly, no shell | none |

`exec` has no output option: it exists for graphical programs, and a command
whose output matters is a `bash+out:` one.

### `opusdm_ipc::funcref` (new module)

```rust
pub enum FuncRef {
    Saved(FunctionId),
    Bash { cmd: String, show_output: bool },
    Exec { line: String },
}
pub fn parse(reference: &str) -> FuncRef;      // total: no prefix => Saved
pub fn format(r: &FuncRef) -> String;          // inverse of parse
pub fn to_steps(r: &FuncRef) -> Result<Vec<FnStep>, String>;   // inline only
pub fn problem(r: &FuncRef) -> Option<String>; // why it cannot run, if so
```

- `to_steps` builds a single `FnStep::Internal { cmd: Run, .. }` with the
  existing `command` and `window` params, so the hub runs it with the code
  that already runs a Function's Run step (no new executor).
  - `Bash`: `command` is `bash -c <quoted cmd>`, quoted with
    `funcref::quote` (single quotes, `\` and `'` escaped). `window` is `show_output`. The command line is
    `bash -c <script> opusdm {f}`, with the `Run` arg `script` set: the hub
    keeps the script (third word) unexpanded and the SOURCE selection
    arrives as `"$@"`. Placeholders are therefore not substituted inside a
    bash script (found in review: pasting file names into a script is code
    injection, and an empty `{o}` silently changes what the script does).
    `to_steps_with(r, wait)` adds `wait` for hooks the hub must finish.
  - `Exec`: `command` is `line` as typed (the Run step already splits it with
    quote handling), `window` is off. The argv is split with the same `cli::tokenize` the Run step uses;
    the first element is the program.
- `problem`:
  - `Bash`: empty or blank command.
  - `Exec`: blank line, unbalanced quotes (`cli::tokenize` fails), or
    a program that is neither an absolute path to an executable file nor found
    in `PATH`.
  - `Saved`: never (existence is checked against the function list, as now).
- No new dependency: the `Run` step already splits its command with `cli::tokenize`, and validation must split exactly as execution does.

### Hub

`run_function_by_id` (`crates/hub/src/server/function_run.rs`) calls
`funcref::parse` first. `Saved` follows the current path. An inline
reference whose `problem` is `Some` fails through `function_failed` with that
text; otherwise it calls `start_function_run` with `to_steps`. Every trigger
(local hotkey, global X11 hotkey, user menu, script event, bank button, file
type) already goes through this function, so the hub needs no other change.

Working directory: the SOURCE Lister's folder when there is a SOURCE, else
`$HOME` (the existing `start_function_run` behavior for a run with no
SOURCE). A global hotkey pressed with no Lister open therefore starts the
program in `$HOME`.

### Validation (ipc)

`validate_hotkeys`, `validate_menu`, `validate_scripts` and `prune` take the
set of known saved ids. An inline reference counts as known when
`problem` is `None`; otherwise it yields a new error per consumer that
carries the `problem` text (`HotkeyInvalidCommand { index, reason }`,
`MenuInvalidCommand { path, reason }`, `ScriptInvalidCommand { index,
reason }`). `sanitize` keeps inline references untouched (it never drops a
reference for a missing program: the program may be installed later).
Validation of a missing `exec` program is therefore an editor error that
blocks Save, but a hub that loads a file by hand-edit still keeps the entry
and reports the failure when it runs.

### UI: shared reference picker (`lister/src/binding_ui.rs`)

`function_dropdown` and `selected_function` are replaced by one widget that
owns a reference string:

- A kind selector: **Function / Bash / Exec**.
- Function: the existing drop-down of saved Functions.
- Bash: a text entry and a **Show output** check box.
- Exec: a text entry and a **Browse...** button. Browse opens
  `gtk::FileDialog` and puts the chosen path in the entry, shell-quoted when
  it contains spaces, keeping any arguments already typed after the program.
- It reports changes through a callback with the new reference string and
  keeps its own state, so switching kind does not lose what was typed for the
  other kinds during the life of the row. A reference loaded from disk selects
  the matching kind.

Edit Hotkeys uses it in phase 1; the rows' error labels are unchanged, with
the new error variants added. "Find Key" reports `Runs bash: <cmd>` or
`Runs: <line>` for inline references. The `(missing function)` text stays for
dangling saved ids.

### Phases

1. **Phase 1**: `funcref` module and tests, hub resolution, validation and new
   error variants, shared picker, Edit Hotkeys, Find Key, global hotkeys.
2. **Phase 2**: the same picker in the user menu editor, the script events
   editor, the button bank editor (three mouse buttons) and the file types
   editor (dblclick, drop, menu). Places that show a Function name (the
   file types context menu, user menu entries) show the label of the inline
   reference: its command text, shortened, with the user menu entry's own
   label where the node has one.

### Security

An inline command is a user-authored string from the user's own config, the
same trust level as a saved Function with a Run step. File names are never
pasted into a script: they reach it only as `"$@"`. `bash -c` receives the command as a single
quoted argument (no second interpretation by the hub), and `exec` never
invokes a shell.

## Testing

- `funcref`: parse/format round trip for all three kinds including an id with
  `:` in the command; `to_steps` for bash (quoting of `'`, `"`, `$`, spaces),
  exec (quoted argument with spaces) and `show_output`; `problem` for empty
  bash, blank exec, unbalanced quotes, missing program, absolute non-executable
  path, program found in `PATH`.
- `bindings`: validation accepts a good inline reference and reports the new
  error for a bad one; `prune` keeps valid inline entries; `sanitize` keeps
  inline hotkeys; a file with only saved ids loads and saves byte-identical.
- Hub: `run_function_by_id` with `bash:echo hi` and `exec:true` starts a run
  and with `exec:opusdm-nope` fails with the `problem` text (run against the
  existing executor tests in `functions.rs`).
- UI logic that does not need GTK (kind detection from a loaded reference,
  quoting of a browsed path) is a plain function and unit tested; the widget
  itself is checked by hand in the Xephyr sandbox: a global `Ctrl+Alt+t` bound
  to `exec:<terminal>` opens a terminal with no Lister open.

## Files touched (phase 1)

`crates/ipc/Cargo.toml`, `crates/ipc/src/funcref.rs` (new), `lib.rs`,
`bindings.rs`, `crates/hub/src/server/function_run.rs`,
`crates/lister/src/binding_ui.rs`, `hotkey_editor.rs`, `CHANGES.md`,
`docs/usage.md`, `FEATURES.md`.
