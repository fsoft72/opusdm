# OpusDM Function Editor (F-01) - design

Status: draft, pending user review 2026-09-28. First feature of the Function
system (`FEATURES.md` section 1: F-01..F-06). Introduces the internal
`Function`/`FnStep` data model, an argument-expansion pass, an editor UI, and
hub-side execution, wired into the Command Palette. Foundation for F-02
(Button Banks), F-03 (Hotkey Editor), F-04 (User menus), F-05 (Script events)
and F-06 (Filetypes), which will attach to `FunctionId`s created here but are
out of scope for this spec.

## Goals

- A `Function` is a named, ordered list of steps. Each step is either an
  internal OpusDM command (initially: Copy, Move, Delete, MakeDir, NewFile,
  Rename - the operations the hub's `JobEngine` already executes) or an
  external command (argv + optional args), with placeholder codes
  `{f} {F} {o} {O} {df} {dF} {do} {dO}` and the `{x!}` optional-argument
  suffix, matching the Amiga argument-code syntax documented in FEATURES.md.
- A dedicated editor window: add/remove/reorder steps, per-step parameter
  form generated from that internal command's schema (required parameters
  shown first and visually distinguished, optional parameters below),
  a **Test** button that really runs the function-as-edited against the
  active lister's current selection/DEST, and a **Save** button that is
  blocked while any step has an unfilled required parameter.
- Functions are persisted (`functions.json`) and runnable from the Command
  Palette (a dynamic "Function" section appended to the existing static
  list). Running one from the palette uses the same execution path as Test.

## Non-goals (deferred to later features per FEATURES.md order)

- Hotkey assignment (local or global) for a function - F-03's job. F-01
  ships functions with no hotkey field; F-03 will reference `FunctionId`s.
- Button banks, user menus, script-event hooks, filetype actions (F-02,
  F-04, F-05, F-06) - all consume `FunctionId`s but are separate specs.
- Parameter schemas for the rest of the existing palette commands (only the
  six fs job commands above get schemas in F-01; other internal commands
  stay palette-only, non-configurable, until a later pass extends the
  schema table).
- `Confirm` step type (F-40) and `Busy`/progress UI beyond per-step
  log lines (F-41) - noted here because they'll attach to the same
  `FnStep` enum later, but not built now.

## Data model (`crates/ipc/src/function.rs`, new module in `opusdm_ipc`)

```rust
pub type FunctionId = String; // uuid v4 string, generated on creation

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Function {
    pub id: FunctionId,
    pub name: String,
    pub steps: Vec<FnStep>,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum FnStep {
    Internal { cmd: InternalCmd, args: InternalArgs },
    External { argv: Vec<String>, cwd_source: bool },
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum InternalCmd { Copy, Move, Delete, MakeDir, NewFile, Rename }

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct InternalArgs(pub std::collections::BTreeMap<String, String>);
```

`InternalArgs` is a flat string map keyed by `ParamSpec::key`; the editor
converts typed widgets (text/bool/path-template) to/from strings, keeping
the wire format simple and forward-compatible with new params.

### Parameter schema (`crates/ipc/src/function/schema.rs`)

```rust
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub required: bool,
    pub kind: ParamKind,
}

pub enum ParamKind { Text, Bool, PathTemplate }

pub fn params_for(cmd: InternalCmd) -> &'static [ParamSpec];
```

Initial schemas (all fields optional unless noted - the six commands act on
the current SOURCE/DEST selection implicitly, so "required" here means
required *configuration*, not required selection):

| `InternalCmd` | params |
|---|---|
| `Copy` | `overwrite: Bool` (opt), `new_name: PathTemplate` (opt, supports `{o}` style rename-on-copy) |
| `Move` | `overwrite: Bool` (opt) |
| `Delete` | `permanent: Bool` (opt, default false = use trash if the fs layer supports it, else hard delete) |
| `MakeDir` | `name: Text` (**required**) |
| `NewFile` | `name: Text` (**required**) |
| `Rename` | `new_name: PathTemplate` (**required**) |

`External` steps have no `ParamSpec` list; validation instead requires
`argv` to be non-empty (the "command" field itself is the one required
field, enforced directly in the editor rather than via the schema table).

### Persistence

`crates/ipc/src/settings.rs` already provides `load_json<T: DeserializeOwned
+ Default>` / `save_json<T: Serialize>` against `config_dir()`
(`$XDG_CONFIG_HOME/opusdm`, fallback `~/.config/opusdm`), used today for
`settings.json` and `desktop.json`. Add:

```rust
pub fn functions_path() -> PathBuf { config_dir().join("functions.json") }
pub fn load_functions() -> Vec<Function> { load_json(&functions_path()) }
pub fn save_functions(fns: &[Function]) { save_json(&functions_path(), &fns) }
```

Stored as a plain `Vec<Function>` (order = editor/palette display order).
`Function` derives `#[serde(default)]`-friendly fields only (no new required
fields added later without a default), matching the existing convention so
old configs never fail to parse.

## Argument expansion and execution (hub)

### New IPC messages (`crates/ipc/src/proto.rs`)

```rust
// Lister -> Hub
ToHub::RunFunction { id: FunctionId, from_lister: ListerId }
ToHub::RunFunctionAdHoc { function: Function, from_lister: ListerId } // Test button, unsaved edits
ToHub::SetFunctions(Vec<Function>)   // editor Save, mirrors SetDesktopSettings

// Hub -> Lister
ToLister::QuerySelection            // hub asks a lister for its current state
FromLister::Selection { paths: Vec<PathBuf>, cwd: PathBuf }

// Hub -> Lister (progress/output, streamed per step)
ToLister::FunctionOutput { step_idx: usize, line: String }
ToLister::FunctionDone { step_idx: usize, exit_code: Option<i32> }
ToLister::FunctionFailed { step_idx: usize, error: String }
```

`QuerySelection`/`Selection` is new plumbing: today only `cwd` propagates
via `PathChanged`; selection does not. The hub already tracks `Roles
{ source, dests, source_locked }` (`crates/lister/src/client.rs:11`) so it
knows *which* listers to query - it sends `QuerySelection` to the SOURCE
lister (`from_lister`) and to the current DEST lister (`roles.dests`
first entry), awaiting both `Selection` replies (with a short timeout -
if a DEST lister doesn't answer, `{df}`-class placeholders expand empty,
matching the `{x!}` optional-argument behavior) before expanding args.

### Expansion pass

Given `SourceSel { paths, cwd }` and `DestSel { paths, cwd }`:

- `{f}` -> SOURCE selected paths (absolute), joined space-separated and
  shell-quoted per entry when substituted into an `External` argv string;
  for `Internal` steps the raw `Vec<PathBuf>` is passed directly (no
  quoting concerns, it's not a shell command line).
- `{F}` -> SOURCE selected paths, uppercase-flag variant per FEATURES.md
  ("names only" vs "names with paths" - exact Amiga `f`/`F`/`o`/`O`
  semantics will be nailed down against `DOpus5.guide` during
  implementation; this spec fixes the four-placeholder shape, not the
  precise lower/upper distinction, since the guide's exact wording wasn't
  re-verified for this design pass).
- `{o}`/`{O}` -> same as `{f}`/`{F}` per the guide's "names only" pairing.
- `{df}`/`{dF}`/`{do}`/`{dO}` -> same four variants from `DestSel`.
- `{x!}` (any of the above with a trailing `!`) -> expands to empty string
  instead of failing when the underlying selection is empty.
- Unresolvable non-optional placeholder (e.g. `{df}` with no DEST lister
  open at all) -> step fails fast with a descriptive error naming the
  step index and the missing placeholder; function execution stops.

### Step execution

Hub executes steps in order, stopping at the first failure (fail fast, per
project-wide error handling standard):

- `Internal`: expanded args + selection feed into the existing `JobEngine`
  (`crates/hub/src/jobs.rs`) via the same job-spec shape `RunJob` already
  uses, reusing `opusdm_fsops` under the hood - no new fs code.
- `External`: `std::process::Command::new(argv[0]).args(&argv[1..])
  .current_dir(cwd_source ? source.cwd : dest.cwd)`, stdout/stderr piped
  and streamed back line-by-line as `FunctionOutput`, exit status reported
  as `FunctionDone`. This is new: existing spawns in `desktop_ops.rs` and
  `toolbar.rs` are fire-and-forget with no captured output.

`RunFunction` and `RunFunctionAdHoc` share one internal `execute(function,
from_lister)` implementation; the ad-hoc variant just skips the
`functions.json` lookup and runs the passed-in (possibly unsaved) function
directly - this is what the editor's Test button uses.

## Editor UI (`crates/lister/src/function_editor.rs`)

New GTK4 window, structurally modeled on `crates/desktop-config/src/
menu_ui.rs` (the closest existing precedent for an ordered, editable list):

- Top: function name `gtk::Entry`.
- Step list: one row per `FnStep`, showing a summary label (`"Internal:
  Copy"` / `"External: notify-send done {o}"`) plus three icon buttons
  (`go-up-symbolic`, `go-down-symbolic`, `list-remove-symbolic`), same
  `move_up`/`move_down`/`remove_at` pattern as `menu_ui.rs:194`. No native
  drag-and-drop reorder (none exists elsewhere in the codebase either).
- "Add Step" button opens a small chooser: Internal (dropdown over
  `InternalCmd` variants) or External (argv text entry). Choosing Internal
  immediately expands a parameter form below, built from `params_for(cmd)`:
  a "Required" section (params where `required: true`, label suffixed
  with `*` and styled, e.g. bold/red accent consistent with existing
  requester error styling) rendered above a collapsible "Optional"
  section. Each `ParamKind` maps to one widget: `Text`/`PathTemplate` ->
  `gtk::Entry`, `Bool` -> `gtk::CheckButton`.
- Bottom: log panel (`gtk::TextView`, read-only, monospace) that receives
  `FunctionOutput`/`FunctionDone`/`FunctionFailed` events while the editor
  window is open, for the Test button.
- **Test** button: enabled only when validation (below) passes on the
  in-memory edit state; on click sends `ToHub::RunFunctionAdHoc` for the
  function as currently edited (not yet saved) and streams results into
  the log panel.
- **Save** button: runs the same validation; if any step has an unfilled
  required param (or an `External` step with empty `argv`), Save stays
  disabled and the offending step row gets an inline error marker. When
  valid, sends `ToHub::SetFunctions` with the full updated list and closes
  or stays open per existing editor-window convention in this codebase
  (match whatever `menu_ui.rs`'s own editor does on save - to confirm
  against that file during implementation).

Validation function (`validate(&Function) -> Vec<ValidationError>`) lives in
`opusdm_ipc::function` itself so the same check can run both client-side
(editor, to gate Save) and defensively on the hub before executing
`SetFunctions`/`RunFunction` (never trust a stale/hand-edited
`functions.json`).

## Palette integration (`crates/lister/src/palette.rs`)

- `CommandId` gains one dynamic variant: `CommandId::RunFunction(FunctionId)`.
- `execute_command` gets one new arm: on `RunFunction(id)`, send
  `ToHub::RunFunction { id, from_lister: my_lister_id }`.
- The palette's entry list changes from purely `&'static [CommandItem]` to
  that static slice plus a `Vec<CommandItem>` built at palette-open time
  from `load_functions()` (or a cached copy pushed live by the hub via
  `SetFunctions` broadcast, mirroring how `desktop.json` changes propagate
  today) - each `Function` becomes one `CommandItem { id:
  CommandId::RunFunction(f.id.clone()), title: &f.name, category:
  "Function", shortcut: None }`. `title`/category being owned `String`
  rather than `&'static str` requires widening `CommandItem`'s field types
  (or introducing an owned variant) - a small, mechanical change to
  `palette.rs`.

## Testing

- Unit tests in `opusdm_ipc::function`: serde round-trip for `Function`/
  `FnStep`/`InternalArgs`; `validate()` catches missing required params on
  every `InternalCmd` and empty-argv `External` steps; placeholder
  expansion against mock `SourceSel`/`DestSel` (including `{x!}` on empty
  selection, and a missing-DEST failure case).
- Manual acceptance walkthrough (GTK4 UI and hub execution aren't unit
  tested elsewhere in this codebase per existing convention - to confirm
  no exception exists for this crate during implementation):
  1. Open the editor, create function "Copy and notify": step 1 =
     `Internal Copy` (no required params), step 2 = `External` with argv
     `["notify-send", "done", "{o}"]`.
  2. Attempt Save with step 2's argv field empty -> Save stays disabled.
  3. Fill the argv field, Save enables, save succeeds.
  4. With 2 files selected in a lister and a DEST lister open, click
     **Test** -> both files are copied into DEST and a notification
     "done <name>" appears; the log panel shows per-step output/exit
     codes.
  5. Restart the app, open the Command Palette -> the function appears
     under a "Function" category; running it from there produces the same
     result as Test.

## Open questions to resolve during implementation

- Exact Amiga `{f}` vs `{F}` vs `{o}` vs `{O}` semantics (this spec assumes
  f/o are a "with path" / "name only" pair duplicated across two letters
  per FEATURES.md's phrasing, but the precise mapping should be checked
  against `DOpus5.guide` directly rather than inferred).
- Whether `menu_ui.rs`'s own editor window closes-on-save or stays open;
  the Function Editor should match that convention rather than invent a
  new one.
