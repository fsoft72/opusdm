# F-01 Function Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user build a named, ordered list of steps (internal fs commands or external/bash commands with argument placeholders), edit it in a GTK4 window with live validation, test-run it for real, save it, and run it again from the Command Palette.

**Architecture:** A new pure data/logic module `opusdm_ipc::function` (model, per-command parameter schema, validation, `{f}{F}{o}{O}{df}{dF}{do}{dO}` placeholder expansion) is shared by the hub and the lister. The Lister owns persistence (`functions.json`, loaded/saved directly - no hub round trip needed for storage, matching how a small local JSON file is otherwise handled) and the editor UI. Running a function is a Lister -> Hub request: the hub asks the SOURCE and current DEST Lister for their selection+cwd (new `QuerySelection`/`SelectionReply` messages, modeled on the existing `ConflictPrompt`/`ConflictReply` pending-request pattern), then executes the steps on a dedicated thread - internal steps reuse `opusdm_fsops::run_job` (same engine Copy/Move/Delete/MakeDir/NewFile already use) or a direct `std::fs::rename`; external steps spawn a subprocess with captured stdout/stderr - streaming `FunctionOutput`/`FunctionStepDone`/`FunctionFailed`/`FunctionDone` back to the requesting Lister, exactly like `JobProgress`/`JobDone` already stream back for file jobs.

**Tech Stack:** Rust, GTK4 (`gtk4-rs`), existing `opusdm_ipc`/`opusdm_fsops`/`opusdm_hub`/`opusdm_lister`/`opusdm_ui` crates. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-28-function-editor-design.md`

## Global Constraints

- No new crate dependencies (no `uuid` - `FunctionId` is a locally-generated string using a timestamp + atomic counter, matching the "don't introduce new tools without strong justification" project rule).
- `InternalCmd` scope for this plan is exactly `{Copy, Move, Delete, MakeDir, NewFile, Rename}` - the six operations the hub can already perform (five via `opusdm_fsops::run_job`'s `Op`, Rename via direct `std::fs::rename`, matching how `palette.rs::CommandId::Rename` already does it today).
- Parameter schemas only declare fields the executor actually consumes (`MakeDir.name`, `NewFile.name`, `Rename.new_name`, all required). No unused optional flags (`overwrite`, permanent-delete, etc.) - those need `Op`/`JobSpec` changes that are out of scope; adding an unwired checkbox to the UI would be dead code (YAGNI, project rule).
- Every text file this plan creates or edits ends with a trailing empty line (project rule).
- Constants (e.g. any new `SCREAMING_SNAKE_CASE` items) use `_`-separated uppercase, matching `MAX_CONCURRENT`, `AUTO_REFRESH_DEBOUNCE_RANGE_MS` elsewhere in this codebase.
- Public functions/methods get a doc comment (`///`); this matches the existing convention throughout `opusdm_ipc`/`opusdm_hub`/`opusdm_lister` and is required by the project's global rules.
- Exit conditions go first, one-linerd where possible (`if !x { return; }`), matching the project rule and the codebase's existing style (see `fail_fast`-style early returns in `server.rs::lister_run_job`).
- Commit after each task, with a plain, non-AI-attributed message (per this session's git instructions) - the plan's own step lists show a commit as the last step of every task.

## Review Focus

- **A Function step targets a path that already exists at DEST** (e.g. the acceptance-test Copy step, run twice). `opusdm_fsops::run_job` blocks on `Progress::Conflict { reply, .. }` waiting for a resolution that, without a UI conflict prompt wired into Function execution (out of scope per the spec's non-goals), would otherwise never come - deadlocking the executor thread forever. Task 6 auto-resolves every Function-driven conflict as `Resolution::Skip` and logs it, so the function still finishes.
- **No DEST Lister is open when a step needs one** (`{df}`/`{do}`, or an `Internal` `Copy`/`Move`/`Rename`-into-DEST-style step with no dest). Task 1's `expand_text` returns `ExpandError::MissingDest` (or empty string for the `!`-suffixed optional form); Task 6's executor surfaces that as a `FunctionFailed` event naming the step, not a silent no-op or panic.
- **An `External` step's command is not on `$PATH` or is not executable.** `Command::spawn()` returns `Err`; Task 6 turns that into `FnEvent::Failed` with the OS error message and the step index, matching the "fail fast, include context" project error-handling rule - not a swallowed error.
- **Save is clicked with a required parameter still blank**, or with zero steps. Task 1's `validate()` and Task 8's Save-button wiring must agree on what "invalid" means (same function, not two parallel checks that can drift) - Task 8 calls `opusdm_ipc::function::validate` directly rather than re-implementing the rule in GTK code.
- **The function referenced by a saved Command Palette entry was deleted or renamed on disk between palette-open and palette-select** (functions.json is re-read at open time, per the design's "dynamic section" choice - Task 9's `execute_command` arm re-reads it again at execution time and must not panic on an id that no longer resolves).

---

## Task 1: `opusdm_ipc::function` - data model, schema, validation

**Files:**
- Create: `crates/ipc/src/function.rs`
- Modify: `crates/ipc/src/lib.rs` (add `pub mod function;` and re-export `pub use function::{Function, FnStep, InternalCmd, InternalArgs, FunctionId};` next to the existing `pub use` lines for `proto`/`desktop`)

**Interfaces:**
- Produces: `pub type FunctionId = String;`, `pub struct Function { pub id: FunctionId, pub name: String, pub steps: Vec<FnStep> }`, `pub fn Function::new(name: impl Into<String>) -> Function`, `pub enum FnStep { Internal { cmd: InternalCmd, args: InternalArgs }, External { argv: Vec<String>, cwd_source: bool } }`, `pub enum InternalCmd { Copy, Move, Delete, MakeDir, NewFile, Rename }` with `impl InternalCmd { pub fn label(self) -> &'static str }`, `pub struct InternalArgs(pub BTreeMap<String, String>)` with `pub fn get(&self, key: &str) -> Option<&str>`, `pub mod schema { pub enum ParamKind { Text, Bool, PathTemplate }; pub struct ParamSpec { pub key: &'static str, pub label: &'static str, pub required: bool, pub kind: ParamKind }; pub fn params_for(cmd: InternalCmd) -> &'static [ParamSpec] }`, `pub struct ValidationError { pub step_idx: usize, pub message: String }`, `pub fn validate(function: &Function) -> Vec<ValidationError>`.

- [ ] **Step 1: Write the failing tests for the data model and schema**

```rust
// crates/ipc/src/function.rs (bottom of file, #[cfg(test)] mod tests)
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn new_function_gets_a_non_empty_unique_id() {
        let a = Function::new("Copy and notify");
        let b = Function::new("Copy and notify");
        assert!(!a.id.is_empty());
        assert_ne!(a.id, b.id);
        assert_eq!(a.name, "Copy and notify");
        assert!(a.steps.is_empty());
    }

    #[test]
    fn function_serde_roundtrips() {
        let f = Function {
            id: "fn-1".into(),
            name: "Copy and notify".into(),
            steps: vec![
                FnStep::Internal { cmd: InternalCmd::Copy, args: InternalArgs::default() },
                FnStep::External { argv: vec!["notify-send".into(), "done".into(), "{o}".into()], cwd_source: true },
            ],
        };
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<Function>(&json).unwrap(), f);
    }

    #[test]
    fn makedir_and_newfile_and_rename_require_their_name_param() {
        for cmd in [InternalCmd::MakeDir, InternalCmd::NewFile] {
            let specs = schema::params_for(cmd);
            assert!(specs.iter().any(|p| p.key == "name" && p.required));
        }
        let specs = schema::params_for(InternalCmd::Rename);
        assert!(specs.iter().any(|p| p.key == "new_name" && p.required));
    }

    #[test]
    fn copy_move_delete_have_no_configurable_params() {
        for cmd in [InternalCmd::Copy, InternalCmd::Move, InternalCmd::Delete] {
            assert!(schema::params_for(cmd).is_empty());
        }
    }

    #[test]
    fn validate_flags_missing_required_internal_param() {
        let f = Function {
            id: "fn-1".into(),
            name: "n".into(),
            steps: vec![FnStep::Internal { cmd: InternalCmd::MakeDir, args: InternalArgs::default() }],
        };
        let errors = validate(&f);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].step_idx, 0);
    }

    #[test]
    fn validate_accepts_internal_step_with_required_param_filled() {
        let mut args = BTreeMap::new();
        args.insert("name".to_string(), "new_dir".to_string());
        let f = Function {
            id: "fn-1".into(),
            name: "n".into(),
            steps: vec![FnStep::Internal { cmd: InternalCmd::MakeDir, args: InternalArgs(args) }],
        };
        assert!(validate(&f).is_empty());
    }

    #[test]
    fn validate_flags_external_step_with_empty_argv() {
        let f = Function {
            id: "fn-1".into(),
            name: "n".into(),
            steps: vec![FnStep::External { argv: vec![], cwd_source: true }],
        };
        assert_eq!(validate(&f).len(), 1);
    }

    #[test]
    fn validate_accepts_a_step_free_function() {
        let f = Function { id: "fn-1".into(), name: "n".into(), steps: vec![] };
        assert!(validate(&f).is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc function:: -- --nocapture`
Expected: FAIL to compile - `function` module, `Function`, `FnStep`, `InternalCmd`, `InternalArgs`, `schema`, `validate` do not exist yet.

- [ ] **Step 3: Implement the data model, schema and validation**

```rust
// crates/ipc/src/function.rs
//! Function system data model: a `Function` is a named, ordered list of
//! `FnStep`s (an internal OpusDM command or an external/bash command). See
//! `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Locally-unique identifier for a saved [`Function`].
pub type FunctionId = String;

/// Generate a `FunctionId` unique within this process's lifetime: a
/// nanosecond timestamp plus a monotonic counter, so two functions created
/// in the same nanosecond still get distinct ids.
fn new_function_id() -> FunctionId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("fn-{nanos:x}-{seq:x}")
}

/// A named, ordered list of steps a user can run from the Command Palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub id: FunctionId,
    pub name: String,
    pub steps: Vec<FnStep>,
}

impl Function {
    /// A new, empty function with a fresh id.
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: new_function_id(), name: name.into(), steps: Vec::new() }
    }
}

/// One step of a [`Function`]: either an OpusDM-internal command or an
/// external program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FnStep {
    /// Run an internal command; `args` holds the values for
    /// `schema::params_for(cmd)`.
    Internal { cmd: InternalCmd, args: InternalArgs },
    /// Run `argv[0]` with the rest as arguments. `cwd_source` selects the
    /// working directory: `true` = the SOURCE Lister's cwd, `false` = the
    /// DEST Lister's cwd (falling back to SOURCE if there is no DEST).
    External { argv: Vec<String>, cwd_source: bool },
}

/// The internal commands a Function step can run - the operations the hub's
/// job engine (and, for `Rename`, a direct `std::fs::rename`) already
/// perform today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InternalCmd {
    Copy,
    Move,
    Delete,
    MakeDir,
    NewFile,
    Rename,
}

impl InternalCmd {
    /// Human-readable label, used in the editor and in error/log messages.
    pub fn label(self) -> &'static str {
        match self {
            InternalCmd::Copy => "Copy",
            InternalCmd::Move => "Move",
            InternalCmd::Delete => "Delete",
            InternalCmd::MakeDir => "Make Directory",
            InternalCmd::NewFile => "New File",
            InternalCmd::Rename => "Rename",
        }
    }
}

/// Values for an `Internal` step's parameters, keyed by
/// `schema::ParamSpec::key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InternalArgs(pub BTreeMap<String, String>);

impl InternalArgs {
    /// The stored value for `key`, if any.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

/// Per-`InternalCmd` parameter schema: which fields the editor must show,
/// and which are required before a `Function` can be saved.
pub mod schema {
    use super::InternalCmd;

    /// How a parameter's value should be edited and expanded.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ParamKind {
        /// Plain text, no placeholder expansion.
        Text,
        /// Checkbox.
        Bool,
        /// Text that may contain `{f}`/`{o}`/... placeholders, expanded at
        /// run time.
        PathTemplate,
    }

    /// One configurable parameter of an [`InternalCmd`].
    #[derive(Debug, Clone, Copy)]
    pub struct ParamSpec {
        pub key: &'static str,
        pub label: &'static str,
        pub required: bool,
        pub kind: ParamKind,
    }

    /// The parameters the editor must offer for `cmd`, required ones first.
    pub fn params_for(cmd: InternalCmd) -> &'static [ParamSpec] {
        match cmd {
            InternalCmd::Copy | InternalCmd::Move | InternalCmd::Delete => &[],
            InternalCmd::MakeDir => &[ParamSpec {
                key: "name",
                label: "Directory name",
                required: true,
                kind: ParamKind::Text,
            }],
            InternalCmd::NewFile => &[ParamSpec {
                key: "name",
                label: "File name",
                required: true,
                kind: ParamKind::Text,
            }],
            InternalCmd::Rename => &[ParamSpec {
                key: "new_name",
                label: "New name",
                required: true,
                kind: ParamKind::PathTemplate,
            }],
        }
    }
}

/// One reason a `Function` cannot be saved yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub step_idx: usize,
    pub message: String,
}

/// Every required-parameter and empty-command problem in `function`, in
/// step order. Empty means the function can be saved.
pub fn validate(function: &Function) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for (step_idx, step) in function.steps.iter().enumerate() {
        match step {
            FnStep::Internal { cmd, args } => {
                for p in schema::params_for(*cmd) {
                    let filled = args.get(p.key).map(|v| !v.trim().is_empty()).unwrap_or(false);
                    if p.required && !filled {
                        errors.push(ValidationError {
                            step_idx,
                            message: format!("{}: missing required parameter '{}'", cmd.label(), p.label),
                        });
                    }
                }
            }
            FnStep::External { argv, .. } => {
                let has_command = argv.first().map(|s| !s.trim().is_empty()).unwrap_or(false);
                if !has_command {
                    errors.push(ValidationError {
                        step_idx,
                        message: "External step: command is required".to_string(),
                    });
                }
            }
        }
    }
    errors
}
```

(Leave the `PathBuf` import and the rest of the file below `validate` for Steps 5-9, which extend this same file with `SelectionCtx`/`expand_text` and the persistence helpers - do not create a second file.)

- [ ] **Step 4: Wire the module into the crate and run the tests**

```rust
// crates/ipc/src/lib.rs - add near the other `pub mod` / `pub use` lines
pub mod function;
pub use function::{Function, FnStep, FunctionId, InternalArgs, InternalCmd};
```

Run: `cargo test -p opusdm-ipc function::`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/function.rs crates/ipc/src/lib.rs
git commit -m "feat(ipc): add Function data model, parameter schema, validation"
```

---

## Task 2: `opusdm_ipc::function` - argument placeholder expansion

**Files:**
- Modify: `crates/ipc/src/function.rs` (append `SelectionCtx`, `ExpandError`, `expand_text`)

**Interfaces:**
- Consumes: nothing new from Task 1 beyond what's already in the file.
- Produces: `pub struct SelectionCtx { pub paths: Vec<PathBuf>, pub cwd: PathBuf }`, `pub enum ExpandError { MissingDest(String) }`, `pub fn expand_text(text: &str, source: &SelectionCtx, dest: Option<&SelectionCtx>) -> Result<String, ExpandError>`. Task 6 (hub executor) and Task 8 (editor Test button, indirectly via the hub) both call `expand_text`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/ipc/src/function.rs, inside #[cfg(test)] mod tests, add:
    fn ctx(paths: &[&str], cwd: &str) -> SelectionCtx {
        SelectionCtx { paths: paths.iter().map(PathBuf::from).collect(), cwd: PathBuf::from(cwd) }
    }

    #[test]
    fn f_expands_to_shell_quoted_absolute_paths() {
        let src = ctx(&["/home/x/a.txt", "/home/x/b c.txt"], "/home/x");
        let out = expand_text("cp {f} /dst", &src, None).unwrap();
        assert_eq!(out, "cp /home/x/a.txt '/home/x/b c.txt' /dst");
    }

    #[test]
    fn o_expands_to_shell_quoted_basenames() {
        let src = ctx(&["/home/x/a.txt", "/home/x/b c.txt"], "/home/x");
        let out = expand_text("notify-send done {o}", &src, None).unwrap();
        assert_eq!(out, "notify-send done a.txt 'b c.txt'");
    }

    #[test]
    fn uppercase_variants_behave_like_their_lowercase_pair() {
        let src = ctx(&["/home/x/a.txt"], "/home/x");
        assert_eq!(expand_text("{F}", &src, None).unwrap(), expand_text("{f}", &src, None).unwrap());
        assert_eq!(expand_text("{O}", &src, None).unwrap(), expand_text("{o}", &src, None).unwrap());
    }

    #[test]
    fn df_and_do_pull_from_dest() {
        let src = ctx(&["/home/x/a.txt"], "/home/x");
        let dst = ctx(&["/home/y/z.txt"], "/home/y");
        assert_eq!(expand_text("{df}", &src, Some(&dst)).unwrap(), "/home/y/z.txt");
        assert_eq!(expand_text("{do}", &src, Some(&dst)).unwrap(), "z.txt");
    }

    #[test]
    fn missing_dest_fails_a_required_dest_placeholder() {
        let src = ctx(&["/home/x/a.txt"], "/home/x");
        let err = expand_text("{df}", &src, None).unwrap_err();
        assert!(matches!(err, ExpandError::MissingDest(t) if t == "{df}"));
    }

    #[test]
    fn optional_dest_placeholder_expands_empty_without_a_dest() {
        let src = ctx(&["/home/x/a.txt"], "/home/x");
        assert_eq!(expand_text("[{df!}]", &src, None).unwrap(), "[]");
    }

    #[test]
    fn empty_selection_expands_to_empty_string_even_without_bang() {
        let src = ctx(&[], "/home/x");
        assert_eq!(expand_text("[{f}]", &src, None).unwrap(), "[]");
    }

    #[test]
    fn text_with_no_placeholders_is_unchanged() {
        let src = ctx(&["/home/x/a.txt"], "/home/x");
        assert_eq!(expand_text("notify-send hello", &src, None).unwrap(), "notify-send hello");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc function:: -- --nocapture`
Expected: FAIL to compile - `SelectionCtx`, `ExpandError`, `expand_text` do not exist yet.

- [ ] **Step 3: Implement expansion**

```rust
// crates/ipc/src/function.rs, appended after `validate`

/// A Lister's current selection and working directory, as reported to the
/// hub for placeholder expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionCtx {
    pub paths: Vec<PathBuf>,
    pub cwd: PathBuf,
}

/// Why [`expand_text`] could not fully expand its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandError {
    /// A non-optional `{df}`/`{dF}`/`{do}`/`{dO}` placeholder had no DEST
    /// selection to draw from. Carries the exact token (e.g. `"{df}"`).
    MissingDest(String),
}

/// Shell-quote `s` if it contains anything that would otherwise need
/// escaping; a plain path segment is left untouched for readability.
fn shell_quote(s: &str) -> String {
    let plain = s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Space-joined, shell-quoted absolute paths - the `{f}`/`{F}`/`{df}`/`{dF}`
/// expansion.
fn joined_paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| shell_quote(&p.display().to_string())).collect::<Vec<_>>().join(" ")
}

/// Space-joined, shell-quoted file names - the `{o}`/`{O}`/`{do}`/`{dO}`
/// expansion.
fn joined_names(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            shell_quote(&name)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Substitute every `{f}` `{F}` `{o}` `{O}` `{df}` `{dF}` `{do}` `{dO}`
/// (each also accepted with a trailing `!`, e.g. `{df!}`) in `text`.
/// `f`/`F` expand to full paths, `o`/`O` to names only (v1 does not
/// distinguish the lower/upper-case pair further - see the design doc's
/// open question); the `d`-prefixed codes draw from `dest` instead of
/// `source`. A `!` suffix makes a missing `dest` expand to an empty string
/// instead of failing.
pub fn expand_text(text: &str, source: &SelectionCtx, dest: Option<&SelectionCtx>) -> Result<String, ExpandError> {
    let mut out = text.to_string();
    for code in ["df", "dF", "do", "dO", "f", "F", "o", "O"] {
        for optional in [true, false] {
            let token = if optional { format!("{{{code}!}}") } else { format!("{{{code}}}") };
            if !out.contains(&token) {
                continue;
            }
            let is_dest = code.starts_with('d');
            let is_path = code.ends_with('f') || code.ends_with('F');
            let ctx = if is_dest {
                match dest {
                    Some(d) => Some(d),
                    None if optional => None,
                    None => return Err(ExpandError::MissingDest(token)),
                }
            } else {
                Some(source)
            };
            let replacement = match ctx {
                Some(c) if is_path => joined_paths(&c.paths),
                Some(c) => joined_names(&c.paths),
                None => String::new(),
            };
            out = out.replace(&token, &replacement);
        }
    }
    Ok(out)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-ipc function::`
Expected: PASS (16 tests total in this file).

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/function.rs
git commit -m "feat(ipc): add Function argument placeholder expansion"
```

---

## Task 3: `opusdm_ipc::function` - persistence (`functions.json`)

**Files:**
- Modify: `crates/ipc/src/function.rs` (append `functions_path`, `load_functions`, `save_functions`)

**Interfaces:**
- Consumes: `crate::settings::{config_dir, load_json, save_json}` (existing, `crates/ipc/src/settings.rs:20,45,67`).
- Produces: `pub fn functions_path() -> PathBuf`, `pub fn load_functions() -> Vec<Function>`, `pub fn save_functions(functions: &[Function]) -> std::io::Result<()>`. Task 8 (editor Save button) and Task 9 (palette dynamic entries) call these.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/ipc/src/function.rs, inside #[cfg(test)] mod tests, add:
    #[test]
    fn functions_path_is_under_config_dir() {
        assert_eq!(functions_path(), crate::settings::config_dir().join("functions.json"));
    }

    #[test]
    fn load_functions_missing_file_is_empty() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        assert!(load_functions().is_empty());
    }

    #[test]
    fn save_and_load_functions_roundtrips() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let f = Function::new("Copy and notify");
        save_functions(&[f.clone()]).unwrap();
        let loaded = load_functions();
        assert_eq!(loaded, vec![f]);
    }
```

`XDG_CONFIG_HOME` is process-global mutable state shared with every other test in this crate that touches `config_dir()`; these three tests must run serialized. Add a `static ENV_LOCK: std::sync::Mutex<()>` (mirroring `crates/hub/src/server.rs`'s `ENV_LOCK` pattern) and take `let _guard = ENV_LOCK.lock().unwrap();` as the first line of each of the two `XDG_CONFIG_HOME`-setting tests above.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc function:: -- --nocapture`
Expected: FAIL to compile - `functions_path`, `load_functions`, `save_functions` do not exist.

- [ ] **Step 3: Implement persistence**

```rust
// crates/ipc/src/function.rs, appended after expand_text

/// Path to `functions.json` under [`crate::settings::config_dir`].
pub fn functions_path() -> PathBuf {
    crate::settings::config_dir().join("functions.json")
}

/// Load every saved [`Function`], in on-disk order. A missing or malformed
/// file yields an empty list (same clean-first-run behavior as
/// [`crate::settings::load_json`]).
pub fn load_functions() -> Vec<Function> {
    crate::settings::load_json(&functions_path(), "opusdm", "functions")
}

/// Save the full list of functions, replacing whatever was there.
pub fn save_functions(functions: &[Function]) -> std::io::Result<()> {
    crate::settings::save_json(&functions_path(), &functions.to_vec())
}
```

Check `crate::settings::load_json`/`save_json` are `pub` (they are - `crates/ipc/src/settings.rs:45,67`) and that `settings` is a public module from `lib.rs` (it already is, since `settings.rs` is used the same way elsewhere).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-ipc function::`
Expected: PASS (19 tests total in this file).

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/function.rs
git commit -m "feat(ipc): add functions.json persistence"
```

---

## Task 4: IPC wire messages (`RunFunction`, `QuerySelection`, `SelectionReply`, `Function*` events)

**Files:**
- Modify: `crates/ipc/src/proto.rs`

**Interfaces:**
- Consumes: `crate::function::FnStep` (Task 1).
- Produces: `ToHub::RunFunction { steps: Vec<FnStep> }`, `ToHub::SelectionReply { paths: Vec<PathBuf>, cwd: PathBuf }`, `ToLister::QuerySelection`, `ToLister::FunctionOutput { step_idx: usize, line: String }`, `ToLister::FunctionStepDone { step_idx: usize, exit_code: Option<i32> }`, `ToLister::FunctionFailed { step_idx: usize, error: String }`, `ToLister::FunctionDone`. Task 5 (hub dispatch) and Task 7/10 (lister dispatch) match on these.

- [ ] **Step 1: Write the failing roundtrip test**

```rust
// crates/ipc/src/proto.rs, inside #[cfg(test)] mod tests, extend
// every_tohub_variant_roundtrips() and every_tolister_variant_roundtrips():
        roundtrip_tohub(ToHub::RunFunction {
            steps: vec![crate::function::FnStep::External {
                argv: vec!["notify-send".into(), "done".into()],
                cwd_source: true,
            }],
        });
        roundtrip_tohub(ToHub::SelectionReply { paths: vec!["/a".into()], cwd: "/home/x".into() });
// ... and, in every_tolister_variant_roundtrips():
        roundtrip_tolister(ToLister::QuerySelection);
        roundtrip_tolister(ToLister::FunctionOutput { step_idx: 0, line: "hello".into() });
        roundtrip_tolister(ToLister::FunctionStepDone { step_idx: 0, exit_code: Some(0) });
        roundtrip_tolister(ToLister::FunctionFailed { step_idx: 1, error: "boom".into() });
        roundtrip_tolister(ToLister::FunctionDone);
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-ipc proto::`
Expected: FAIL to compile - the new variants don't exist.

- [ ] **Step 3: Add the variants**

```rust
// crates/ipc/src/proto.rs, inside `pub enum ToHub { ... }`, add before the closing brace:
    /// Run a Function's steps, expanding placeholders against this Lister's
    /// (SOURCE) and the current DEST Lister's selection.
    RunFunction { steps: Vec<crate::function::FnStep> },
    /// Reply to `ToLister::QuerySelection`.
    SelectionReply { paths: Vec<PathBuf>, cwd: PathBuf },

// crates/ipc/src/proto.rs, inside `pub enum ToLister { ... }`, add before the closing brace:
    /// Ask this Lister for its current selection and cwd, for Function
    /// argument expansion. Reply with `ToHub::SelectionReply`.
    QuerySelection,
    /// One line of output from a running Function step (internal-command
    /// progress, or external-command stdout/stderr).
    FunctionOutput { step_idx: usize, line: String },
    /// A Function step finished successfully.
    FunctionStepDone { step_idx: usize, exit_code: Option<i32> },
    /// A Function step failed; execution stopped here.
    FunctionFailed { step_idx: usize, error: String },
    /// Every step of a running Function finished successfully.
    FunctionDone,
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p opusdm-ipc proto::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ipc/src/proto.rs
git commit -m "feat(ipc): add RunFunction/QuerySelection wire messages"
```

---

## Task 5: hub - pending Function-run bookkeeping (`RunFunction` / `SelectionReply` wiring)

**Files:**
- Modify: `crates/hub/src/server.rs`

**Interfaces:**
- Consumes: `opusdm_ipc::function::SelectionCtx` (Task 2), `ToHub::RunFunction`/`ToHub::SelectionReply`, `ToLister::QuerySelection` (Task 4), `crate::functions::{spawn_executor, FnEvent}` (Task 6 - written next, but referenced here; Task 6 must land before this task's `cargo build` succeeds, so implement Task 6's `functions.rs` module signature-only stub first if executing tasks out of order is ever necessary - as written, this plan does Task 6 immediately after, so just proceed and Task 5's own tests won't compile until Task 6 exists; do Task 5's Step 1-3 then Task 6's Step 1-3 before either task's `cargo test`).
- Produces: the `Hub` struct gains function-run state; `lister_msg` gains two arms; a new private `Hub::function_event(&mut self, run_id: u64, ev: crate::functions::FnEvent)` method that Task 6's forwarder thread feeds via a new `OwnerMsg::FunctionEvent(u64, crate::functions::FnEvent)` variant.

Because this task and Task 6 (`crates/hub/src/functions.rs`) are mutually referential (`server.rs` calls `functions::spawn_executor`, and the type it depends on, `FnEvent`, is defined there), implement both files' production code before running any test in either - write Task 5's Steps 1 and 3 now, then jump to Task 6's Steps 1 and 3, then come back and run both tasks' tests together at the end of Task 6. Task 5's own "run tests, verify fail / verify pass" steps below assume Task 6's module already exists; do them in that order.

- [ ] **Step 1: Write the failing integration test**

```rust
// crates/hub/src/server.rs, inside #[cfg(test)] mod tests, add (uses the
// existing bind_temp()/spawn_server()/connect() helpers already in this
// file - see the module's other tests for their exact behavior):
    #[test]
    fn run_function_queries_source_and_dest_then_executes() {
        let (_td, listener, path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));

        // SOURCE lister.
        let mut src = connect(&path);
        send(&mut src, &ToHub::Register { pid: 1, initial_path: "/src".into() }).unwrap();
        let _w: Welcome = recv(&mut src).unwrap().unwrap();
        // First registrant becomes SOURCE automatically (see roles.rs / the
        // other tests in this file for that convention); confirm via a
        // RoleChanged before proceeding is unnecessary here since we only
        // need `src` to remain SOURCE, which registration-order already
        // gives it.

        // DEST lister.
        let mut dst = connect(&path);
        send(&mut dst, &ToHub::Register { pid: 2, initial_path: "/dst".into() }).unwrap();
        let _w: Welcome = recv(&mut dst).unwrap().unwrap();
        send(&mut dst, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
        // Drain the RoleChanged broadcasts this triggers on both connections.
        let _r: ToLister = recv(&mut src).unwrap().unwrap();
        let _r: ToLister = recv(&mut dst).unwrap().unwrap();

        let steps = vec![opusdm_ipc::function::FnStep::External {
            argv: vec!["true".into()],
            cwd_source: true,
        }];
        send(&mut src, &ToHub::RunFunction { steps }).unwrap();

        // Hub must ask both listers for their selection.
        let q1: ToLister = recv(&mut src).unwrap().unwrap();
        assert!(matches!(q1, ToLister::QuerySelection));
        let q2: ToLister = recv(&mut dst).unwrap().unwrap();
        assert!(matches!(q2, ToLister::QuerySelection));

        send(&mut src, &ToHub::SelectionReply { paths: vec!["/src/a".into()], cwd: "/src".into() }).unwrap();
        send(&mut dst, &ToHub::SelectionReply { paths: vec![], cwd: "/dst".into() }).unwrap();

        // Execution happens on a worker thread; poll src for the result.
        let mut done = false;
        for _ in 0..50 {
            match recv::<_, ToLister>(&mut src) {
                Ok(Some(ToLister::FunctionDone)) => {
                    done = true;
                    break;
                }
                Ok(Some(_)) => continue, // FunctionOutput / FunctionStepDone lines
                _ => break,
            }
        }
        assert!(done, "expected FunctionDone on the requesting (SOURCE) connection");
        let _ = handle;
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p opusdm-hub run_function_queries_source_and_dest_then_executes`
Expected: FAIL to compile - `ToHub::RunFunction` has no handler, `crate::functions` does not exist yet.

- [ ] **Step 3: Add the pending-run state and message handling**

```rust
// crates/hub/src/server.rs - add near the top, after the existing `use` block
use opusdm_ipc::function::{FnStep, SelectionCtx};

// crates/hub/src/server.rs - extend `enum OwnerMsg` (around line 132):
enum OwnerMsg {
    Accepted(UnixStream),
    FromLister(u64, ToHub),
    ListerEof(u64),
    JobProgress(u64, Progress),
    FromGtk(HubCmd),
    FunctionEvent(u64, crate::functions::FnEvent),
}

// crates/hub/src/server.rs - a Function run waiting on one or both
// selections, tracked between `ToHub::RunFunction` and the point both
// `SelectionReply`s (or the one from a Lister with no DEST) have arrived.
// Add just above `struct Hub`:
struct PendingFunctionRun {
    requester_cid: u64,
    steps: Vec<FnStep>,
    source_cid: u64,
    dest_cid: Option<u64>,
    source_sel: Option<SelectionCtx>,
    dest_sel: Option<SelectionCtx>,
}

// crates/hub/src/server.rs - extend `struct Hub` (around line 210):
struct Hub {
    conns: HashMap<u64, Conn>,
    free_conns: BTreeSet<u64>,
    next_conn: u64,
    roles: RoleState,
    current_settings: opusdm_ipc::Settings,
    current_desktop: DesktopSettings,
    persist_tx: Sender<PersistMsg>,
    engine: JobEngine,
    jobs: HashMap<u64, JobMeta>,
    replies: HashMap<u64, Sender<Resolution>>,
    next_req: u64,
    evt_tx: Sender<HubEvent>,
    on_spawn: SpawnFn,
    /// Function runs waiting on `SelectionReply` from SOURCE and/or DEST.
    pending_functions: HashMap<u64, PendingFunctionRun>,
    /// Which pending run a Lister connection's next `SelectionReply` is for.
    selection_waiters: HashMap<u64, u64>,
    /// Which connection to route a running Function's events back to, kept
    /// alive from the moment execution starts until `FunctionDone`/`Failed`.
    function_requesters: HashMap<u64, u64>,
    next_run_id: u64,
    function_events_tx: Sender<(u64, crate::functions::FnEvent)>,
}
```

Add the two `lister_msg` arms (inside the existing `match msg` in `fn lister_msg`, next to `ToHub::RunJob`):

```rust
            ToHub::RunFunction { steps } => {
                self.start_function_run(cid, steps);
            }

            ToHub::SelectionReply { paths, cwd } => {
                self.selection_reply(cid, paths, cwd);
            }
```

Add the two new methods on `impl Hub` (near `lister_run_job`):

```rust
    /// `ToHub::RunFunction` from `cid`: record a pending run and ask `cid`
    /// (SOURCE) and, if one exists, the current DEST Lister for their
    /// selection.
    fn start_function_run(&mut self, cid: u64, steps: Vec<FnStep>) {
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        let dest_cid = self.roles.dests().first().map(|d| d.0);
        self.pending_functions.insert(
            run_id,
            PendingFunctionRun {
                requester_cid: cid,
                steps,
                source_cid: cid,
                dest_cid,
                source_sel: None,
                dest_sel: None,
            },
        );
        self.selection_waiters.insert(cid, run_id);
        if let Some(c) = self.conns.get_mut(&cid) {
            let _ = c.tx.send(ToLister::QuerySelection.into());
        }
        if let Some(d) = dest_cid {
            self.selection_waiters.insert(d, run_id);
            if let Some(c) = self.conns.get_mut(&d) {
                let _ = c.tx.send(ToLister::QuerySelection.into());
            }
        }
    }

    /// `ToHub::SelectionReply` from `cid`: fill in whichever slot (source or
    /// dest) of its pending run this answers, and start execution once every
    /// slot the run needs is filled.
    fn selection_reply(&mut self, cid: u64, paths: Vec<PathBuf>, cwd: PathBuf) {
        let Some(&run_id) = self.selection_waiters.get(&cid) else { return };
        self.selection_waiters.remove(&cid);
        let Some(run) = self.pending_functions.get_mut(&run_id) else { return };
        let sel = SelectionCtx { paths, cwd };
        if cid == run.source_cid {
            run.source_sel = Some(sel);
        } else if Some(cid) == run.dest_cid {
            run.dest_sel = Some(sel);
        }
        let ready = run.source_sel.is_some() && (run.dest_cid.is_none() || run.dest_sel.is_some());
        if !ready {
            return;
        }
        let run = self.pending_functions.remove(&run_id).unwrap();
        self.function_requesters.insert(run_id, run.requester_cid);
        crate::functions::spawn_executor(
            run_id,
            run.steps,
            run.source_sel.unwrap(),
            run.dest_sel,
            self.function_events_tx.clone(),
        );
    }

    /// A `FnEvent` arrived from a running Function's executor thread; turn
    /// it into the matching `ToLister` and send it to whichever connection
    /// asked for this run.
    fn function_event(&mut self, run_id: u64, ev: crate::functions::FnEvent) {
        let Some(&requester) = self.function_requesters.get(&run_id) else { return };
        let terminal = matches!(ev, crate::functions::FnEvent::Failed { .. } | crate::functions::FnEvent::AllDone);
        let msg = match ev {
            crate::functions::FnEvent::Output { step_idx, line } => ToLister::FunctionOutput { step_idx, line },
            crate::functions::FnEvent::StepDone { step_idx, exit_code } => {
                ToLister::FunctionStepDone { step_idx, exit_code }
            }
            crate::functions::FnEvent::Failed { step_idx, error } => ToLister::FunctionFailed { step_idx, error },
            crate::functions::FnEvent::AllDone => ToLister::FunctionDone,
        };
        if terminal {
            self.function_requesters.remove(&run_id);
        }
        if let Some(c) = self.conns.get_mut(&requester) {
            let _ = c.tx.send(msg.into());
        }
    }
```

Wire the new channel and fields into `owner_loop` (mirrors the existing `jp_tx`/`jp_rx` job-progress forwarder immediately above the `let mut hub = Hub { ... }` construction):

```rust
    let (fe_tx, fe_rx) = channel::<(u64, crate::functions::FnEvent)>();
    {
        let owner_tx = owner_tx.clone();
        std::thread::Builder::new()
            .name("opusdm-function-events".into())
            .spawn(move || {
                for (id, ev) in fe_rx {
                    if owner_tx.send(OwnerMsg::FunctionEvent(id, ev)).is_err() {
                        break;
                    }
                }
                opusdm_ipc::debug_log!("hub", "function event forwarder exiting");
            })
            .expect("spawn function event forwarder thread");
    }
```

Add the matching fields to the `Hub { ... }` struct literal:

```rust
        pending_functions: HashMap::new(),
        selection_waiters: HashMap::new(),
        function_requesters: HashMap::new(),
        next_run_id: 1,
        function_events_tx: fe_tx,
```

Add the `OwnerMsg::FunctionEvent` arm to the owner loop's `match msg`:

```rust
            OwnerMsg::FunctionEvent(run_id, ev) => {
                hub.function_event(run_id, ev);
                true
            }
```

- [ ] **Step 4: (deferred)** Do not run tests yet - `crate::functions` does not exist until Task 6. Proceed to Task 6 Steps 1-3, then return here.

---

## Task 6: hub - Function step executor (`crates/hub/src/functions.rs`)

**Files:**
- Create: `crates/hub/src/functions.rs`
- Modify: `crates/hub/src/lib.rs` (add `pub mod functions;` if `lib.rs` declares hub's modules that way - check the existing `mod jobs;`/`pub mod jobs;` line and match its visibility exactly; `server.rs` refers to `crate::functions::...`, so it must be reachable from `server.rs` at minimum - `mod functions;` without `pub` is sufficient if only `server.rs` uses it, matching how other hub-internal modules are declared)

**Interfaces:**
- Consumes: `opusdm_ipc::function::{expand_text, ExpandError, FnStep, InternalArgs, InternalCmd, SelectionCtx}` (Tasks 1-2), `opusdm_fsops::{run_job, CancelToken, JobSpec, Progress}` (existing, used identically to `crates/hub/src/jobs.rs`), `opusdm_ipc::{Op, Resolution}` (existing).
- Produces: `pub enum FnEvent { Output { step_idx: usize, line: String }, StepDone { step_idx: usize, exit_code: Option<i32> }, Failed { step_idx: usize, error: String }, AllDone }`, `pub fn spawn_executor(run_id: u64, steps: Vec<FnStep>, source: SelectionCtx, dest: Option<SelectionCtx>, events_tx: Sender<(u64, FnEvent)>)`. Task 5 calls `spawn_executor` and matches on `FnEvent`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/hub/src/functions.rs, #[cfg(test)] mod tests at the bottom
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::mpsc::channel;

    fn ctx(paths: &[&str], cwd: &str) -> SelectionCtx {
        SelectionCtx { paths: paths.iter().map(std::path::PathBuf::from).collect(), cwd: cwd.into() }
    }

    fn drain(rx: std::sync::mpsc::Receiver<(u64, FnEvent)>) -> Vec<FnEvent> {
        rx.into_iter().map(|(_, ev)| ev).collect()
    }

    #[test]
    fn external_step_streams_stdout_and_reports_success() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["echo".into(), "hi {o}".into()], cwd_source: true }];
        let src = ctx(&["/tmp/a.txt"], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(events.iter().any(|e| matches!(e, FnEvent::Output { line, .. } if line == "hi a.txt")));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn external_step_nonzero_exit_fails_the_run() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["false".into()], cwd_source: true }];
        let src = ctx(&[], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, .. })));
    }

    #[test]
    fn unknown_command_fails_with_context_not_a_panic() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["opusdm-does-not-exist-xyz".into()], cwd_source: true }];
        let src = ctx(&[], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("opusdm-does-not-exist-xyz")));
    }

    #[test]
    fn dest_placeholder_without_a_dest_lister_fails_with_context() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["echo".into(), "{do}".into()], cwd_source: true }];
        let src = ctx(&["/tmp/a.txt"], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("DEST")));
    }

    #[test]
    fn makedir_step_creates_a_directory_in_dest() {
        let td = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let mut args = BTreeMap::new();
        args.insert("name".to_string(), "new_dir".to_string());
        let steps = vec![FnStep::Internal {
            cmd: InternalCmd::MakeDir,
            args: InternalArgs(args),
        }];
        let src = ctx(&[], "/tmp");
        let dst = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &steps, &src, Some(&dst), &tx);
        drop(tx);
        assert!(td.path().join("new_dir").is_dir());
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn rename_step_renames_the_first_selected_path() {
        let td = tempfile::tempdir().unwrap();
        let old = td.path().join("old.txt");
        std::fs::write(&old, b"hi").unwrap();
        let (tx, rx) = channel();
        let mut args = BTreeMap::new();
        args.insert("new_name".to_string(), "new.txt".to_string());
        let steps = vec![FnStep::Internal {
            cmd: InternalCmd::Rename,
            args: InternalArgs(args),
        }];
        let src = ctx(&[old.to_str().unwrap()], td.path().to_str().unwrap());
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        assert!(!old.exists());
        assert!(td.path().join("new.txt").exists());
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn copy_conflict_is_auto_skipped_instead_of_deadlocking() {
        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();
        let src_file = src_dir.path().join("a.txt");
        std::fs::write(&src_file, b"new").unwrap();
        std::fs::write(dst_dir.path().join("a.txt"), b"existing").unwrap();

        let (tx, rx) = channel();
        let steps = vec![FnStep::Internal { cmd: InternalCmd::Copy, args: InternalArgs::default() }];
        let src = ctx(&[src_file.to_str().unwrap()], src_dir.path().to_str().unwrap());
        let dst = ctx(&[], dst_dir.path().to_str().unwrap());

        // Must return promptly (the test's own completion is the proof this
        // does not deadlock on Progress::Conflict).
        run_steps(1, &steps, &src, Some(&dst), &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
        assert_eq!(std::fs::read(dst_dir.path().join("a.txt")).unwrap(), b"existing", "conflict must be skipped, not overwritten");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p opusdm-hub functions::`
Expected: FAIL to compile - `crates/hub/src/functions.rs` does not exist.

- [ ] **Step 3: Implement the executor**

```rust
// crates/hub/src/functions.rs
//! Executes a Function's steps on their own thread once the hub has
//! collected the SOURCE and (if any) DEST selection: internal steps reuse
//! `opusdm_fsops::run_job` or a direct `std::fs::rename`, external steps
//! spawn a subprocess and stream its output. See
//! `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use opusdm_fsops::{run_job, CancelToken, JobSpec, Progress};
use opusdm_ipc::function::{expand_text, ExpandError, FnStep, InternalArgs, InternalCmd, SelectionCtx};
use opusdm_ipc::{Op, Resolution};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Sender};

/// One event a running Function reports, tagged with its `run_id` by the
/// caller when relayed onward (see `server.rs::function_event`).
pub enum FnEvent {
    /// One line of output (internal-step progress, or external-step
    /// stdout/stderr).
    Output { step_idx: usize, line: String },
    /// A step finished successfully.
    StepDone { step_idx: usize, exit_code: Option<i32> },
    /// A step failed; the run stops here.
    Failed { step_idx: usize, error: String },
    /// Every step finished successfully.
    AllDone,
}

/// Spawn the thread that runs `steps` in order, reporting each event on
/// `events_tx` tagged with `run_id`.
pub fn spawn_executor(
    run_id: u64,
    steps: Vec<FnStep>,
    source: SelectionCtx,
    dest: Option<SelectionCtx>,
    events_tx: Sender<(u64, FnEvent)>,
) {
    std::thread::Builder::new()
        .name(format!("opusdm-function-{run_id}"))
        .spawn(move || run_steps(run_id, &steps, &source, dest.as_ref(), &events_tx))
        .expect("spawn function executor thread");
}

/// Run every step in order, stopping (and emitting `Failed`) at the first
/// error; emits `AllDone` only if every step succeeded. Free function (not
/// a method) so it is directly unit-testable without a running hub.
fn run_steps(
    run_id: u64,
    steps: &[FnStep],
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) {
    for (step_idx, step) in steps.iter().enumerate() {
        let result = match step {
            FnStep::Internal { cmd, args } => run_internal(run_id, step_idx, *cmd, args, source, dest, events_tx),
            FnStep::External { argv, cwd_source } => {
                run_external(run_id, step_idx, argv, *cwd_source, source, dest, events_tx)
            }
        };
        if let Err(error) = result {
            let _ = events_tx.send((run_id, FnEvent::Failed { step_idx, error }));
            return;
        }
    }
    let _ = events_tx.send((run_id, FnEvent::AllDone));
}

fn expand_err_msg(e: ExpandError) -> String {
    match e {
        ExpandError::MissingDest(token) => format!("{token}: no DEST Lister open"),
    }
}

/// Run one `Internal` step. `Copy`/`Move`/`Delete` reuse the same
/// `opusdm_fsops::run_job` the hub's own `JobEngine` calls; `MakeDir`/
/// `NewFile` create directly (no scan/progress needed for one entry);
/// `Rename` is a direct `std::fs::rename`.
fn run_internal(
    run_id: u64,
    step_idx: usize,
    cmd: InternalCmd,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    match cmd {
        InternalCmd::Rename => {
            let template = args.get("new_name").unwrap_or("");
            let new_name = expand_text(template, source, dest).map_err(expand_err_msg)?;
            let old = source.paths.first().ok_or_else(|| "Rename: nothing selected".to_string())?;
            let new_path = old.with_file_name(new_name.trim());
            std::fs::rename(old, &new_path).map_err(|e| format!("rename failed: {e}"))?;
            let _ = events_tx.send((
                run_id,
                FnEvent::Output { step_idx, line: format!("renamed {} -> {}", old.display(), new_path.display()) },
            ));
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
        InternalCmd::MakeDir | InternalCmd::NewFile => {
            // Both commands use the same schema key ("name", see Task 1).
            let template = args.get("name").unwrap_or("");
            let name = expand_text(template, source, dest).map_err(expand_err_msg)?;
            let dest_dir = dest.map(|d| d.cwd.clone()).unwrap_or_else(|| source.cwd.clone());
            let target = dest_dir.join(name.trim());
            let result = if cmd == InternalCmd::MakeDir {
                std::fs::create_dir(&target)
            } else {
                std::fs::File::create(&target).map(|_| ())
            };
            result.map_err(|e| format!("{}: {e}", cmd.label()))?;
            let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("created {}", target.display()) }));
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
        InternalCmd::Copy | InternalCmd::Move | InternalCmd::Delete => {
            let op = match cmd {
                InternalCmd::Copy => Op::Copy,
                InternalCmd::Move => Op::Move,
                InternalCmd::Delete => Op::Delete,
                _ => unreachable!("Rename/MakeDir/NewFile handled above"),
            };
            let dest_dir = match dest {
                Some(d) => d.cwd.clone(),
                None if cmd == InternalCmd::Delete => source.cwd.clone(),
                None => return Err(format!("{}: no DEST Lister open", cmd.label())),
            };
            let spec = JobSpec { op, srcs: source.paths.clone(), dest_dir };
            let (tx, rx) = channel::<Progress>();
            run_job(spec, tx, CancelToken::new());
            for ev in rx {
                match ev {
                    Progress::File { path, .. } => {
                        let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("{}: {}", cmd.label(), path.display()) }));
                    }
                    // No conflict-prompt UI exists for Function steps yet
                    // (see the design doc's non-goals): always skip, so a
                    // pre-existing destination entry cannot hang this
                    // thread forever waiting on a reply nobody will send.
                    Progress::Conflict { reply, path, .. } => {
                        let _ = reply.send(Resolution::Skip);
                        let _ = events_tx.send((run_id, FnEvent::Output {
                            step_idx,
                            line: format!("conflict: {} already exists, skipped", path.display()),
                        }));
                    }
                    Progress::Done { copied, skipped, errors, .. } => {
                        if let Some((path, msg)) = errors.first() {
                            return Err(format!("{}: {} ({msg})", cmd.label(), path.display()));
                        }
                        let _ = events_tx.send((run_id, FnEvent::Output {
                            step_idx,
                            line: format!("{}: {copied} copied, {skipped} skipped", cmd.label()),
                        }));
                    }
                    Progress::Scanning { .. } => {}
                }
            }
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
    }
}

/// Run one `External` step: expand every argv element, spawn it with piped
/// stdout/stderr, stream every line as it arrives, and report the exit
/// status.
fn run_external(
    run_id: u64,
    step_idx: usize,
    argv: &[String],
    cwd_source: bool,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    if argv.is_empty() {
        return Err("External step: command is required".to_string());
    }
    let expanded: Vec<String> =
        argv.iter().map(|a| expand_text(a, source, dest)).collect::<Result<_, _>>().map_err(expand_err_msg)?;
    let cwd = if cwd_source { &source.cwd } else { dest.map(|d| &d.cwd).unwrap_or(&source.cwd) };

    let mut child = Command::new(&expanded[0])
        .args(&expanded[1..])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start '{}': {e}", expanded[0]))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_tx = events_tx.clone();
    let out_thread = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = out_tx.send((run_id, FnEvent::Output { step_idx, line }));
        }
    });
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line }));
    }
    let _ = out_thread.join();

    let status = child.wait().map_err(|e| format!("wait failed: {e}"))?;
    let exit_code = status.code();
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code }));
    if !status.success() {
        return Err(format!("'{}' exited with {:?}", expanded[0], exit_code));
    }
    Ok(())
}
```

Check `crates/hub/Cargo.toml` already depends on `opusdm-fsops` and `tempfile` (dev-dependency) - both are already used by `crates/hub/src/jobs.rs`'s own tests, so no `Cargo.toml` change should be needed; if `cargo test -p opusdm-hub` reports a missing crate, add the missing line matching `jobs.rs`'s existing dependencies exactly.

Add the module to `crates/hub/src/lib.rs` next to the other `mod` declarations (match whichever of `mod jobs;` / `pub mod jobs;` is already there):

```rust
mod functions;
```

- [ ] **Step 4: Run Task 6's tests**

Run: `cargo test -p opusdm-hub functions::`
Expected: PASS (7 tests).

- [ ] **Step 5: Now go back and run Task 5's test**

Run: `cargo test -p opusdm-hub run_function_queries_source_and_dest_then_executes`
Expected: PASS.

- [ ] **Step 6: Run the full hub test suite to catch any wiring regression**

Run: `cargo test -p opusdm-hub`
Expected: PASS (every existing hub test still passes).

- [ ] **Step 7: Commit both tasks together (they only compile together)**

```bash
git add crates/hub/src/functions.rs crates/hub/src/server.rs crates/hub/src/lib.rs
git commit -m "feat(hub): execute Function steps (RunFunction/QuerySelection wiring)"
```

---

## Task 7: lister - answer `QuerySelection`

**Files:**
- Modify: `crates/lister/src/window.rs` (the `apply_inbound` match, `crates/lister/src/window.rs:444-487`)

**Interfaces:**
- Consumes: `Ui::selected_paths(&self) -> Vec<PathBuf>` and `Ui::cwd_path(&self) -> PathBuf` (existing, `crates/lister/src/window.rs` around lines 356 and 395), `ui.hub.client.send(&ToHub::SelectionReply { .. })` (existing `Client::send`, `crates/lister/src/client.rs:90`).
- Produces: nothing new consumed elsewhere; this closes the loop Task 5 depends on to ever receive a reply from a real Lister process (Task 5's own test used raw socket messages standing in for a Lister, so this task is what makes the feature work end to end rather than just in the hub's own test harness).

This task has no independent unit-testable surface of its own (it is one match arm calling two already-tested existing methods and one already-tested `send`); there is no new pure function to TDD in isolation. Skip the red/green cycle and go straight to the edit, then verify by extending Task 6/5's coverage is already done - the manual acceptance walkthrough in Task 11 is what actually exercises this arm end to end.

- [ ] **Step 1: Add the match arm**

```rust
// crates/lister/src/window.rs, inside `apply_inbound`'s match, next to
// `ToLister::Pong => {}`:
            ToLister::QuerySelection => {
                let _ = self.hub.client.send(&opusdm_ipc::ToHub::SelectionReply {
                    paths: self.selected_paths(),
                    cwd: self.cwd_path(),
                });
            }
```

(Leave `ToLister::FunctionOutput`/`FunctionStepDone`/`FunctionFailed`/`FunctionDone` unhandled for now - Rust will not compile an inexhaustive match, so Task 10 must land before this file compiles again. To keep this task independently committable, also add the four remaining arms now as bare no-ops, and let Task 10 replace them:)

```rust
            ToLister::FunctionOutput { .. } => {}
            ToLister::FunctionStepDone { .. } => {}
            ToLister::FunctionFailed { .. } => {}
            ToLister::FunctionDone => {}
```

- [ ] **Step 2: Build the lister crate**

Run: `cargo build -p opusdm-lister`
Expected: builds clean (the `match` is exhaustive again).

- [ ] **Step 3: Commit**

```bash
git add crates/lister/src/window.rs
git commit -m "feat(lister): answer hub QuerySelection with the current selection"
```

---

## Task 8: lister - Function Editor UI

**Files:**
- Create: `crates/lister/src/function_editor.rs`
- Modify: `crates/lister/src/lib.rs` (add `mod function_editor;`)

**Interfaces:**
- Consumes: `opusdm_ipc::function::{Function, FnStep, InternalCmd, InternalArgs, schema, validate, load_functions, save_functions}` (Tasks 1-3), `opusdm_ui::{DialogShell, requester, requester_entry}` (existing, `crates/ui/src/shell.rs`, `crates/ui/src/requester.rs`), `ToHub::RunFunction` (Task 4), `ui.hub.client.send` (existing).
- Produces: `pub fn open_functions_manager(ui: &Rc<Ui>)` - the entry point Task 9's new palette command calls.

This task is GTK4 UI wiring with no pure logic of its own beyond what Tasks 1-3 already unit-test (validation, expansion, persistence); there is no meaningful red/green cycle for widget-construction code (this matches how `crates/desktop-config/src/menu_ui.rs`, its structural template, has no unit tests either - GTK4 UI in this codebase is verified manually, per the existing convention noted in the design doc). Write it directly, then verify with the manual walkthrough in Task 11.

- [ ] **Step 1: Write the manager window (list of saved functions)**

```rust
// crates/lister/src/function_editor.rs
//! The Function Editor: a manager window listing saved functions (New/
//! Edit/Delete), and a per-function editor window (step list, parameter
//! forms, Test, Save). See
//! `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::function::{self, schema, Function, FnStep, InternalArgs, InternalCmd};
use std::cell::RefCell;
use std::rc::Rc;

/// Open the "Manage Functions" window: every saved function, with New/Edit/
/// Delete buttons.
pub fn open_functions_manager(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Manage Functions").size(420, 360);
    let root = shell.content();

    let list_box = gtk::ListBox::new();
    let functions: Rc<RefCell<Vec<Function>>> = Rc::new(RefCell::new(function::load_functions()));

    fn rebuild(list_box: &gtk::ListBox, functions: &[Function]) {
        while let Some(row) = list_box.first_child() {
            list_box.remove(&row);
        }
        for f in functions {
            let row = gtk::ListBoxRow::new();
            let label = gtk::Label::new(Some(&f.name));
            label.set_xalign(0.0);
            label.set_margin_start(6);
            label.set_margin_end(6);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            row.set_child(Some(&label));
            list_box.append(&row);
        }
    }
    rebuild(&list_box, &functions.borrow());

    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).build();
    root.append(&scroller);

    let new_btn = shell.button("New Function...");
    let edit_btn = shell.button("Edit...");
    let delete_btn = shell.button("Delete");
    shell.button_spacer();
    shell.close_button("Close");

    {
        let ui = ui.clone();
        let functions = functions.clone();
        let window = shell.window().clone();
        new_btn.connect_clicked(move |_| {
            let ui = ui.clone();
            let functions = functions.clone();
            let window = window.clone();
            opusdm_ui::requester_entry(&window, "New Function", "", &[("Create", true), ("Cancel", false)], move |ok, name| {
                if !ok || name.trim().is_empty() {
                    return;
                }
                let f = Function::new(name.trim());
                open_function_editor(&ui, f, functions.clone());
            });
        });
    }
    {
        let ui = ui.clone();
        let functions = functions.clone();
        let list_box = list_box.clone();
        edit_btn.connect_clicked(move |_| {
            let Some(row) = list_box.selected_row() else { return };
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let Some(f) = functions.borrow().get(idx as usize).cloned() else { return };
            open_function_editor(&ui, f, functions.clone());
        });
    }
    {
        let list_box = list_box.clone();
        let functions = functions.clone();
        let window = shell.window().clone();
        delete_btn.connect_clicked(move |_| {
            let Some(row) = list_box.selected_row() else { return };
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let functions = functions.clone();
            let list_box = list_box.clone();
            opusdm_ui::requester(&window, "Delete Function", "Delete this function?", None, &[("Delete", true), ("Cancel", false)], move |ok| {
                if !ok {
                    return;
                }
                functions.borrow_mut().remove(idx as usize);
                let _ = function::save_functions(&functions.borrow());
                rebuild(&list_box, &functions.borrow());
            });
        });
    }

    // Re-render whenever `functions` changes underneath this window (a save
    // in the per-function editor mutates the shared `Rc<RefCell<_>>`).
    {
        let functions = functions.clone();
        let list_box = list_box.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            rebuild(&list_box, &functions.borrow());
            gtk::glib::ControlFlow::Continue
        });
    }

    shell.present();
}
```

The `timeout_add_local` poll is a deliberately simple way to keep the manager list in sync with edits made in a separately-opened per-function editor window, without adding an event-bus abstraction the rest of this codebase doesn't have; `rebuild` is idempotent and cheap (a handful of labels), so a 300ms poll is not a real cost. If the manager and editor windows are never open at the same time in practice, this still causes no harm - it only becomes visibly useful the one time they are.

- [ ] **Step 2: Write the per-function editor window (step list + Save/Test)**

```rust
// crates/lister/src/function_editor.rs, appended

struct EditorState {
    function: RefCell<Function>,
    steps_box: gtk::Box,
    log_buffer: gtk::TextBuffer,
    save_btn: gtk::Button,
    test_btn: gtk::Button,
    window: gtk::Window,
}

/// Open the editor for one function. `all_functions` is the manager's shared
/// list; Save replaces this function's entry (by id) in place and persists
/// the whole list.
fn open_function_editor(ui: &Rc<Ui>, function: Function, all_functions: Rc<RefCell<Vec<Function>>>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Edit Function").size(520, 480).scrolled();
    let root = shell.content();

    let name_entry = gtk::Entry::new();
    name_entry.set_text(&function.name);
    root.append(&name_entry);

    let steps_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.append(&steps_box);

    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let add_internal = gtk::Button::with_label("Add Internal Step...");
    let add_external = gtk::Button::with_label("Add External Step...");
    add_row.append(&add_internal);
    add_row.append(&add_external);
    root.append(&add_row);

    let log_view = gtk::TextView::new();
    log_view.set_editable(false);
    log_view.set_monospace(true);
    let log_buffer = log_view.buffer();
    let log_scroller = gtk::ScrolledWindow::builder().child(&log_view).min_content_height(100).build();
    root.append(&log_scroller);

    let save_btn = shell.button("Save");
    let test_btn = shell.button("Test");
    shell.button_spacer();
    shell.close_button("Close");

    let state = Rc::new(EditorState {
        function: RefCell::new(function),
        steps_box,
        log_buffer,
        save_btn: save_btn.clone(),
        test_btn: test_btn.clone(),
        window: shell.window().clone(),
    });

    {
        let state = state.clone();
        name_entry.connect_changed(move |e| {
            state.function.borrow_mut().name = e.text().to_string();
        });
    }
    {
        let state = state.clone();
        add_internal.connect_clicked(move |_| prompt_add_internal(&state));
    }
    {
        let state = state.clone();
        add_external.connect_clicked(move |_| prompt_add_external(&state));
    }
    {
        let state = state.clone();
        let all_functions = all_functions.clone();
        save_btn.connect_clicked(move |_| {
            let f = state.function.borrow().clone();
            let mut all = all_functions.borrow_mut();
            match all.iter_mut().find(|existing| existing.id == f.id) {
                Some(slot) => *slot = f,
                None => all.push(f),
            }
            let _ = function::save_functions(&all);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        test_btn.connect_clicked(move |_| {
            let f = state.function.borrow().clone();
            state.log_buffer.set_text("");
            *ui.function_log.borrow_mut() = Some(state.log_buffer.clone());
            let _ = ui.hub.client.send(&opusdm_ipc::ToHub::RunFunction { steps: f.steps });
        });
    }

    rebuild_steps(&state);
    update_button_states(&state);
    shell.present();
}
```

- [ ] **Step 3: Write the step-list rendering (up/down/remove, matching `menu_ui.rs`) and validation-driven button state**

```rust
// crates/lister/src/function_editor.rs, appended

/// Repaint `state.steps_box` from `state.function`'s steps, and refresh
/// Save/Test enabled state to match `function::validate`.
fn rebuild_steps(state: &Rc<EditorState>) {
    while let Some(row) = state.steps_box.first_child() {
        state.steps_box.remove(&row);
    }
    let steps: Vec<FnStep> = state.function.borrow().steps.clone();
    for (i, step) in steps.iter().enumerate() {
        state.steps_box.append(&build_step_row(state, i, step));
    }
    update_button_states(state);
}

fn update_button_states(state: &Rc<EditorState>) {
    let valid = function::validate(&state.function.borrow()).is_empty();
    state.save_btn.set_sensitive(valid);
    state.test_btn.set_sensitive(valid);
}

fn build_step_row(state: &Rc<EditorState>, idx: usize, step: &FnStep) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let summary = match step {
        FnStep::Internal { cmd, .. } => format!("Internal: {}", cmd.label()),
        FnStep::External { argv, .. } => format!("External: {}", argv.join(" ")),
    };
    let label = gtk::Label::new(Some(&summary));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    header.append(&label);

    for (icon, apply) in step_move_and_remove(idx) {
        let btn = gtk::Button::from_icon_name(icon);
        btn.add_css_class("flat");
        let state = state.clone();
        btn.connect_clicked(move |_| {
            apply(&mut state.function.borrow_mut().steps);
            rebuild_steps(&state);
        });
        header.append(&btn);
    }
    row.append(&header);

    // Required-parameter form for an Internal step, shown inline under its
    // summary; External steps have nothing further to configure here (the
    // argv itself, entered at Add time, is their one required field).
    if let FnStep::Internal { cmd, .. } = step {
        let specs = schema::params_for(*cmd);
        if !specs.is_empty() {
            let form = gtk::Box::new(gtk::Orientation::Vertical, 2);
            form.set_margin_start(18);
            let required = gtk::Label::new(Some("Required"));
            required.set_xalign(0.0);
            required.add_css_class("heading");
            form.append(&required);
            for spec in specs.iter().filter(|s| s.required) {
                form.append(&build_param_field(state, idx, spec));
            }
            row.append(&form);
        }
    }
    row
}

/// Move-up/move-down/remove for the step at `idx`, mirroring
/// `crates/desktop-config/src/menu_ui.rs::MenuInner::move_and_remove`.
fn step_move_and_remove(idx: usize) -> [(&'static str, Box<dyn Fn(&mut Vec<FnStep>)>); 3] {
    [
        ("go-up-symbolic", Box::new(move |steps: &mut Vec<FnStep>| {
            if idx > 0 {
                steps.swap(idx, idx - 1);
            }
        })),
        ("go-down-symbolic", Box::new(move |steps: &mut Vec<FnStep>| {
            if idx + 1 < steps.len() {
                steps.swap(idx, idx + 1);
            }
        })),
        ("list-remove-symbolic", Box::new(move |steps: &mut Vec<FnStep>| {
            if idx < steps.len() {
                steps.remove(idx);
            }
        })),
    ]
}

/// One required-parameter entry, writing back into `InternalArgs` on every
/// keystroke and re-validating so Save/Test flip live.
fn build_param_field(state: &Rc<EditorState>, step_idx: usize, spec: &schema::ParamSpec) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(spec.label));
    label.set_xalign(0.0);
    row.append(&label);

    let entry = gtk::Entry::new();
    if let Some(FnStep::Internal { args, .. }) = state.function.borrow().steps.get(step_idx) {
        if let Some(v) = args.get(spec.key) {
            entry.set_text(v);
        }
    }
    let key = spec.key;
    let state_clone = state.clone();
    entry.connect_changed(move |e| {
        let text = e.text().to_string();
        if let Some(FnStep::Internal { args, .. }) = state_clone.function.borrow_mut().steps.get_mut(step_idx) {
            args.0.insert(key.to_string(), text);
        }
        update_button_states(&state_clone);
    });
    row.append(&entry);
    row
}
```

- [ ] **Step 4: Write the Add Step dialogs**

```rust
// crates/lister/src/function_editor.rs, appended

const INTERNAL_CMDS: &[InternalCmd] = &[
    InternalCmd::Copy,
    InternalCmd::Move,
    InternalCmd::Delete,
    InternalCmd::MakeDir,
    InternalCmd::NewFile,
    InternalCmd::Rename,
];

/// Ask which `InternalCmd` to add, append it with empty args, and rebuild.
/// Uses `DialogShell` directly (like the per-function editor window itself)
/// rather than `crates/ui/src/requester.rs`'s helpers, since those are
/// built specifically around a text body or a single `gtk::Entry` and a
/// dropdown does not fit either shape without changing that file's shared
/// `build()` signature - a bigger change than one small dialog justifies.
fn prompt_add_internal(state: &Rc<EditorState>) {
    let shell = opusdm_ui::DialogShell::new(&state.window, "Add Internal Step");
    let labels: Vec<&str> = INTERNAL_CMDS.iter().map(|c| c.label()).collect();
    let dropdown = gtk::DropDown::from_strings(&labels);
    shell.content().append(&dropdown);

    let add_btn = shell.button("Add");
    shell.close_button("Cancel");

    let state = state.clone();
    let window = shell.window().clone();
    add_btn.connect_clicked(move |_| {
        let cmd = INTERNAL_CMDS[dropdown.selected() as usize];
        state.function.borrow_mut().steps.push(FnStep::Internal { cmd, args: InternalArgs::default() });
        rebuild_steps(&state);
        window.destroy();
    });

    shell.present();
}

/// Ask for a shell command line, split it on whitespace into `argv`, append
/// an External step running it in the SOURCE Lister's directory, and
/// rebuild.
fn prompt_add_external(state: &Rc<EditorState>) {
    let state_clone = state.clone();
    opusdm_ui::requester_entry(&state.window, "Add External Step (e.g. notify-send done {o})", "", &[("Add", true), ("Cancel", false)], move |ok, text| {
        if !ok || text.trim().is_empty() {
            return;
        }
        let argv: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        state_clone.function.borrow_mut().steps.push(FnStep::External { argv, cwd_source: true });
        rebuild_steps(&state_clone);
    });
}
```

`opusdm_ui::requester_widget` (a requester whose body is an arbitrary widget instead of plain text or an entry) does not exist yet in `crates/ui/src/requester.rs` - check the file for a close match before adding one; if none exists, add it now, modeled exactly on `requester_entry`'s structure (same button layout, same "every exit path delivers exactly one choice" guarantee) but taking `body: &impl IsA<gtk::Widget>` instead of building its own `gtk::Entry`, appending `body` into the dialog where `requester_entry` appends its entry. This is a small, in-place addition to an existing shared file, not a new subsystem - keep it to the same size and doc-comment style as its neighbors in that file.

- [ ] **Step 5: Wire the module and build**

```rust
// crates/lister/src/lib.rs - add next to the other `mod` declarations
mod function_editor;
```

Run: `cargo build -p opusdm-lister`
Expected: builds clean.

- [ ] **Step 6: Manual smoke test**

Run the lister (see the project's existing `run-x11.sh` or equivalent dev-run script) and open the (not-yet-wired-to-a-command) manager by calling `opusdm_lister::function_editor::open_functions_manager` from a temporary call site, or wait until Task 9 wires it to the palette - if this task is executed by a fresh subagent that cannot run a GTK app interactively, skip this step and rely on Task 11's full walkthrough after Task 9 lands.

- [ ] **Step 7: Commit**

```bash
git add crates/lister/src/function_editor.rs crates/lister/src/lib.rs
git commit -m "feat(lister): Function Editor UI (manager + step editor)"
```

---

## Task 9: lister - Command Palette integration

**Files:**
- Modify: `crates/lister/src/keys.rs` (`CommandId` enum, `crates/lister/src/keys.rs:16-46`)
- Modify: `crates/lister/src/palette.rs` (`CommandItem`, `COMMANDS`, `get_commands`, `execute_command`)

**Interfaces:**
- Consumes: `opusdm_ipc::function::{load_functions, FunctionId}` (Task 3), `crate::function_editor::open_functions_manager` (Task 8), `ToHub::RunFunction` (Task 4).
- Produces: `CommandId::ManageFunctions` and `CommandId::RunFunction(FunctionId)`; `get_commands() -> Vec<CommandItem>` (signature change from `&'static [CommandItem]`).

`CommandId` currently derives `Copy` (`crates/lister/src/keys.rs:15`); a `FunctionId` payload (`String`) is not `Copy`, so `Copy` must be dropped from both `CommandId` and `Action` (`Action::Command(CommandId)` embeds it, `crates/lister/src/keys.rs:71-93`). This is a real, mechanical ripple through the two files below - each call site is listed explicitly so none is missed.

- [ ] **Step 1: Write the failing test for the new command existing and being reachable**

```rust
// crates/lister/src/keys.rs, inside #[cfg(test)] mod tests, add:
    #[test]
    fn manage_functions_command_exists_in_the_palette() {
        let known: Vec<CommandId> = crate::palette::get_commands().iter().map(|c| c.id.clone()).collect();
        assert!(known.contains(&CommandId::ManageFunctions));
    }
```

Also fix the existing `every_command_id_the_map_emits_exists_in_the_palette` test's now-broken `.map(|c| c.id)` (Copy removal) to `.map(|c| c.id.clone())`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p opusdm-lister keys::`
Expected: FAIL to compile - `CommandId::ManageFunctions` does not exist, and (once it does, from Step 3's partial edit) `Copy` removal breaks `.map(|c| c.id)` until Step 2's fix above is applied.

- [ ] **Step 3: Update `CommandId` and `Action`**

```rust
// crates/lister/src/keys.rs:15-16 - remove Copy from the derive
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CommandId {
    // ... existing variants unchanged ...
    TogglePin,
    /// Open the Function Editor's manager window (list/new/edit/delete).
    ManageFunctions,
    /// Run a saved Function by id.
    RunFunction(opusdm_ipc::function::FunctionId),
}
```

```rust
// crates/lister/src/keys.rs:71 - remove Copy from Action's derive too
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    // ... unchanged ...
}
```

`resolve()`'s body constructs fresh `CommandId`/`Action` values everywhere (`Some(Action::Command(CommandId::SelectAll))`, etc.) - none of that code moves or copies an existing value, so it needs no changes. Grep the crate for any other place relying on implicit `Copy` of a `CommandId`/`Action` value before moving on:

Run: `grep -rn '\.id\b' crates/lister/src/palette.rs`

Expected hits: the `CommandItem { id: ..., ... }` construction sites (fine, these build new values) and `execute_command(&ui_for_exec, cmd.id)` inside `palette.rs::open_command_palette`'s `execute_selected` closure (`crates/lister/src/palette.rs:364`, `cmd: &CommandItem` there) - that is the one site that needs `.clone()`, fixed in Step 5 below.

- [ ] **Step 4: Update `CommandItem` to hold owned, dynamic entries**

```rust
// crates/lister/src/palette.rs:1-19
use std::borrow::Cow;
// ... existing use lines unchanged ...

#[derive(Clone)]
pub struct CommandItem {
    pub id: CommandId,
    pub title: Cow<'static, str>,
    pub category: Cow<'static, str>,
    pub shortcut: Option<&'static str>,
}
```

Wrap every `title:`/`category:` literal in the existing `COMMANDS` array (`crates/lister/src/palette.rs:24-54`) with `Cow::Borrowed(...)`:

Run: `sed -i -E 's/title: "([^"]*)", category: "([^"]*)"/title: Cow::Borrowed("\1"), category: Cow::Borrowed("\2")/' crates/lister/src/palette.rs`

Then add one more static entry to the end of `COMMANDS` (before the closing `];`):

```rust
        CommandItem { id: CommandId::ManageFunctions, title: Cow::Borrowed("Manage Functions..."), category: Cow::Borrowed("Functions"), shortcut: None },
```

- [ ] **Step 5: Make `get_commands` dynamic and add the two new `execute_command` arms**

```rust
// crates/lister/src/palette.rs - replace the existing
//   pub fn get_commands() -> &'static [CommandItem] { COMMANDS }
// with:

/// Every palette entry: the static commands, plus one dynamic "Function"
/// entry per saved function (re-read from `functions.json` on every call,
/// so a function created/renamed/deleted since the palette last opened is
/// always current).
pub fn get_commands() -> Vec<CommandItem> {
    let mut items: Vec<CommandItem> = COMMANDS.to_vec();
    for f in opusdm_ipc::function::load_functions() {
        items.push(CommandItem {
            id: CommandId::RunFunction(f.id),
            title: Cow::Owned(f.name),
            category: Cow::Borrowed("Function"),
            shortcut: None,
        });
    }
    items
}
```

`COMMANDS.to_vec()` requires `CommandItem: Clone` (already derived) - fine.

Add the two new `execute_command` arms (next to `CommandId::TogglePin`'s arm):

```rust
        CommandId::ManageFunctions => {
            crate::function_editor::open_functions_manager(ui);
        }
        CommandId::RunFunction(id) => {
            let Some(function) = opusdm_ipc::function::load_functions().into_iter().find(|f| f.id == id) else {
                crate::actions::flash(ui, "function not found (it may have been deleted)");
                return;
            };
            let _ = ui.hub.client.send(&ToHub::RunFunction { steps: function.steps });
        }
```

Fix the one `.clone()` call site identified in Step 3:

```rust
// crates/lister/src/palette.rs, inside open_command_palette's execute_selected closure
                        execute_command(&ui_for_exec, cmd.id.clone());
```

Fix `open_command_palette` (`crates/lister/src/palette.rs`, the loop right after `let commands = get_commands();`) for two effects of this task's type changes:

1. `get_commands()` now returns an owned `Vec<CommandItem>` instead of `&'static [CommandItem]`; change `for cmd in commands {` to `for cmd in &commands {` so `cmd` stays `&CommandItem` and every line below it (`cmd.clone()`, the closures capturing `cmd.id`) keeps compiling exactly as before.
2. `cmd.category`/`cmd.title` are now `Cow<'static, str>`, not `&'static str`, so the two places that build a `Some(&str)` for `gtk::Label::new` need an explicit borrow:

```rust
// crates/lister/src/palette.rs - was `gtk::Label::new(Some(cmd.category));`
        let cat_label = gtk::Label::new(Some(cmd.category.as_ref()));
// ... was `gtk::Label::new(Some(cmd.title));`
        let title_label = gtk::Label::new(Some(cmd.title.as_ref()));
```

The fuzzy-match filter's `format!("{} {}", cmd.title, cmd.category)` needs no change - `Cow<str>` implements `Display`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p opusdm-lister`
Expected: PASS (every existing test, plus the new `manage_functions_command_exists_in_the_palette`).

- [ ] **Step 7: Build the whole workspace**

Run: `cargo build --workspace`
Expected: builds clean - this is the first point every crate touched by this plan compiles together.

- [ ] **Step 8: Commit**

```bash
git add crates/lister/src/keys.rs crates/lister/src/palette.rs
git commit -m "feat(lister): Command Palette integration for Functions"
```

---

## Task 10: lister - stream Function run events into the editor log (and a flash for palette-triggered runs)

**Files:**
- Modify: `crates/lister/src/window.rs` (`struct Ui`, its construction in `crates/lister/src/build.rs:380`, and the four `ToLister::Function*` no-op arms added in Task 7)
- Modify: `crates/lister/src/function_editor.rs` (nothing further needed here - Task 8 already writes into `ui.function_log`, which this task adds to `Ui`)

**Interfaces:**
- Consumes: `crate::actions::flash` (existing, used throughout `palette.rs`).
- Produces: `pub function_log: RefCell<Option<gtk::TextBuffer>>` on `Ui`, and a private `Ui::append_function_log(&self, text: &str)` helper.

Like Task 7, this is dispatch wiring with no new pure logic; verify by the manual walkthrough in Task 11.

- [ ] **Step 1: Add the field**

```rust
// crates/lister/src/window.rs, inside `pub struct Ui { ... }`, next to `preview_win` (crates/lister/src/window.rs:113):
    /// The log buffer of the currently-open Function Editor's per-function
    /// window, if one is open - Function run events are appended here while
    /// set, so the Test button shows real-time output. `None` when no
    /// editor window is open (a palette-triggered run then only flashes a
    /// summary, via `run_key_action`/`execute_command`'s existing `flash`).
    pub function_log: RefCell<Option<gtk::TextBuffer>>,
```

```rust
// crates/lister/src/build.rs:380, next to `preview_win: RefCell::new(None),`
        function_log: RefCell::new(None),
```

- [ ] **Step 2: Replace the four no-op arms with real handling**

```rust
// crates/lister/src/window.rs, inside `apply_inbound`'s match, replacing
// the four bare arms Task 7 added:
            ToLister::FunctionOutput { step_idx, line } => {
                self.append_function_log(&format!("[step {step_idx}] {line}\n"));
            }
            ToLister::FunctionStepDone { step_idx, exit_code } => {
                self.append_function_log(&format!("[step {step_idx}] done (exit {exit_code:?})\n"));
            }
            ToLister::FunctionFailed { step_idx, error } => {
                self.append_function_log(&format!("[step {step_idx}] FAILED: {error}\n"));
                crate::actions::flash(self, &format!("function failed at step {step_idx}: {error}"));
            }
            ToLister::FunctionDone => {
                self.append_function_log("function finished\n");
                crate::actions::flash(self, "function finished");
            }
```

Add the helper method on `impl Ui` (near the other small helpers, e.g. next to `reveal_filter`/`dismiss_filter`):

```rust
    /// Append `text` to the open Function Editor's log, if one is open.
    fn append_function_log(self: &Rc<Self>, text: &str) {
        if let Some(buf) = self.function_log.borrow().as_ref() {
            let mut end = buf.end_iter();
            buf.insert(&mut end, text);
        }
    }
```

- [ ] **Step 3: Build**

Run: `cargo build -p opusdm-lister`
Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add crates/lister/src/window.rs crates/lister/src/build.rs
git commit -m "feat(lister): stream Function run output into the editor log"
```

---

## Task 11: full workspace verification and manual acceptance walkthrough

**Files:** none (verification only).

- [ ] **Step 1: Run every automated test**

Run: `cargo test --workspace`
Expected: PASS, including every test added in Tasks 1-9.

- [ ] **Step 2: Run the workspace lints, matching whatever this project's existing CI/pre-commit does**

Run: `cargo clippy --workspace --all-targets -- -D warnings` (check `.githooks/` or CI config first for the exact flags already in use, and match them rather than inventing new ones)
Expected: no warnings from any file this plan touched.

- [ ] **Step 3: Manual acceptance walkthrough**

Follow the design spec's acceptance criteria exactly (`docs/superpowers/specs/2026-09-28-function-editor-design.md`, "Accettazione e test" section):

1. Start the hub and two listers (via the project's existing dev-run script), set one SOURCE and one DEST.
2. In the SOURCE lister, open the Command Palette (`Ctrl+P`) and run "Manage Functions...".
3. Click "New Function...", name it "Copy and notify".
4. Click "Add Internal Step...", pick "Copy".
5. Click "Add External Step...", enter `notify-send done {o}`.
6. Confirm Save is enabled (Copy has no required params; the External step's command is filled) and click Save.
7. Select 2 files in the SOURCE lister, click Test in the editor window: confirm both files land in the DEST directory, a notification appears, and the log panel shows per-step output ending in "function finished".
8. Close the editor, reopen the Command Palette: confirm "Copy and notify" appears under category "Function". Run it; confirm the same result.
9. Delete one of the copied files at DEST before re-running Test, to also exercise the conflict-auto-skip path (Review Focus item 1): confirm the run still finishes and the pre-existing file is left untouched, with a "conflict: ... skipped" line in the log.

- [ ] **Step 4: Update `CHANGES.md`**

Add an entry (per this session's global instructions: keep a progressive `CHANGES.md` at the project root) describing the Function Editor addition in the same style as the file's existing entries - read `CHANGES.md`'s current last few entries first and match their format exactly.

- [ ] **Step 5: Final commit**

```bash
git add CHANGES.md
git commit -m "docs: record F-01 Function Editor in CHANGES.md"
```
