# Inline commands (phase 1: hotkeys) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In Edit Hotkeys, a row can run a saved Function, a bash command or an executable, and the hub runs all three.

**Architecture:** An inline command is a prefixed string kept in the existing `FunctionId` field (`bash:`, `bash+out:`, `exec:`), so no schema or protocol changes. A new `opusdm_ipc::funcref` module parses the string and turns it into a single synthetic `Run` step, which the hub already knows how to execute. The hub resolves the reference in `run_function_by_id`, the validators accept valid inline references, and a shared GTK picker widget lets Edit Hotkeys choose the kind.

**Tech Stack:** Rust workspace (`opusdm-ipc`, `opusdm-hub`, `opusdm-lister`), GTK4 (gtk4-rs 0.10), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-05-inline-commands-design.md`

**Scope:** this plan is phase 1 of the spec (core, hub, validation, picker, Edit Hotkeys, Find Key). Phase 2 (user menu, script events, button banks, file types) gets its own plan once phase 1 is merged. The ipc validators for menu and scripts are done here because they live next to the hotkey one.

## Global Constraints

- Never use the em dash character in text, code or comments; use `-`.
- Code and comments in English.
- Every function and method gets a doc comment; public ones use rustdoc (`///`).
- Constants in `UPPER_SNAKE_CASE`. Exit conditions at the top of a function, as one-liners when possible.
- Text files end with an empty line.
- No new dependencies. **Deviation from the spec:** the spec asked for `shlex`, but the hub's `Run` step already splits its `command` with `opusdm_ipc::cli::tokenize` (quotes group, backslash escapes). `funcref` uses the same function, so validation and execution can never disagree about where an argument ends. The spec is corrected in Task 1.
- Reference prefixes, exactly: `bash:`, `bash+out:`, `exec:`. A string without a prefix is a saved Function id (`fn-<hex>-<hex>`).
- Run `cargo fmt` only if the project already formats with it (check `rustfmt.toml`/`.editorconfig` before; do not reformat unrelated code).
- Commit after every task, only the files that task changed. Never push. Never use `--no-verify`. No AI attribution lines in commit messages.
- Update `CHANGES.md` (top section, today's date) in the task that finishes a user-visible behavior.

## Review Focus

- `exec:"/opt/my app/run" --flag` (a quoted program path with a space) splits into program and argument correctly (Task 1).
- A bash command with `'`, `"`, `\`, `$HOME`, `{a,b}`, `${HOME}` and a newline reaches `bash -c` unchanged, and braces that are not placeholders are not touched by placeholder expansion (Tasks 1 and 3).
- A hand-edited `hotkeys.json` holding `exec:` with a program that does not exist loads, is kept by `sanitize`, and fails at run time with a readable message instead of crashing the hub (Tasks 2 and 3).
- A global hotkey fired with no Lister open (headless run) runs an inline command in `$HOME` (Task 3).
- Typing in the command field of a row keeps the keyboard focus (no row rebuild per keystroke) and keeps Save disabled with the reason shown while the command is invalid (Task 5, manual check).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ipc/src/funcref.rs` (new) | The reference type: parse, format, quote, validity check, conversion to a `Run` step |
| `crates/ipc/src/lib.rs` | `pub mod funcref;` |
| `crates/ipc/src/bindings.rs` | Validators and `prune` accept valid inline references; three new error variants |
| `crates/hub/src/server/function_run.rs` | `run_function_by_id` resolves inline references |
| `crates/hub/src/server.rs` | Hub tests for inline runs (tests module only) |
| `crates/lister/src/binding_ui.rs` | `reference_picker` widget and its pure helpers |
| `crates/lister/src/hotkey_editor.rs` | Row uses the picker; in-place error refresh; Find Key text |
| `docs/usage.md`, `CHANGES.md`, `FEATURES.md` | Documentation |

---

### Task 1: The `funcref` module

**Files:**
- Create: `crates/ipc/src/funcref.rs`
- Modify: `crates/ipc/src/lib.rs` (add `pub mod funcref;` after `pub mod function;`)
- Modify: `docs/superpowers/specs/2026-10-05-inline-commands-design.md` (replace the `shlex` mentions)

**Interfaces:**
- Consumes: `crate::cli::tokenize(&str) -> Result<Vec<String>, String>`; `crate::function::{FnStep, FunctionId, InternalArgs, InternalCmd, BOOL_TRUE}` (`InternalArgs` is a tuple struct over `BTreeMap<String, String>`).
- Produces (used by Tasks 2 to 5):
  - `pub const BASH_PREFIX: &str`, `BASH_OUT_PREFIX: &str`, `EXEC_PREFIX: &str`
  - `pub enum FuncRef { Saved(FunctionId), Bash { cmd: String, show_output: bool }, Exec { line: String } }`
  - `pub fn parse(reference: &str) -> FuncRef`
  - `pub fn format(r: &FuncRef) -> String`
  - `pub fn quote(text: &str) -> String`
  - `pub fn program_exists(program: &str) -> bool`
  - `pub fn problem(r: &FuncRef) -> Option<String>`
  - `pub fn describe(r: &FuncRef) -> Option<String>` (`None` for `Saved`)
  - `pub fn to_steps(r: &FuncRef) -> Result<Vec<FnStep>, String>`

- [ ] **Step 1: Write the module with its tests first (tests only compile once the code exists, so write both, then run)**

Create `crates/ipc/src/funcref.rs`:

```rust
//! References to something a hotkey, menu entry or script event can run.
//!
//! The places that used to store a saved Function's id keep storing one
//! string. A reserved prefix turns it into an inline command: `bash:<cmd>`
//! (run by `bash -c`), `bash+out:<cmd>` (the same, output in a window) or
//! `exec:<program and arguments>` (started directly, no shell). A string
//! without a prefix is a saved Function id, as before. An inline reference
//! runs as one synthetic `Run` step, so the hub needs no new executor. See
//! `docs/superpowers/specs/2026-10-05-inline-commands-design.md`.

use crate::cli::tokenize;
use crate::function::{FnStep, FunctionId, InternalArgs, InternalCmd, BOOL_TRUE};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Prefix of a bash command whose output is not shown.
pub const BASH_PREFIX: &str = "bash:";
/// Prefix of a bash command whose output goes to a window.
pub const BASH_OUT_PREFIX: &str = "bash+out:";
/// Prefix of a program started directly.
pub const EXEC_PREFIX: &str = "exec:";

/// What a reference string stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuncRef {
    /// A saved Function, by id (no prefix).
    Saved(FunctionId),
    /// `bash -c cmd`; the output goes to a window when `show_output`.
    Bash { cmd: String, show_output: bool },
    /// A program and its arguments, split like a `Run` step splits them.
    Exec { line: String },
}

/// Read a reference string. Total: anything that is not an inline command is
/// a saved Function id.
pub fn parse(reference: &str) -> FuncRef {
    if let Some(cmd) = reference.strip_prefix(BASH_OUT_PREFIX) {
        return FuncRef::Bash { cmd: cmd.to_string(), show_output: true };
    }
    if let Some(cmd) = reference.strip_prefix(BASH_PREFIX) {
        return FuncRef::Bash { cmd: cmd.to_string(), show_output: false };
    }
    if let Some(line) = reference.strip_prefix(EXEC_PREFIX) {
        return FuncRef::Exec { line: line.to_string() };
    }
    FuncRef::Saved(reference.to_string())
}

/// The reference string for `r`; the inverse of [`parse`].
pub fn format(r: &FuncRef) -> String {
    match r {
        FuncRef::Saved(id) => id.clone(),
        FuncRef::Bash { cmd, show_output: false } => format!("{BASH_PREFIX}{cmd}"),
        FuncRef::Bash { cmd, show_output: true } => format!("{BASH_OUT_PREFIX}{cmd}"),
        FuncRef::Exec { line } => format!("{EXEC_PREFIX}{line}"),
    }
}

/// `text` as one word that [`tokenize`] reads back unchanged: single quotes
/// around it, with `\` and `'` escaped by a backslash.
pub fn quote(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        if c == '\\' || c == '\'' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Whether `path` is a regular file with an execute bit.
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Whether `program` can be started: a path (it contains `/`) to an
/// executable file, or a name found in a `PATH` directory.
pub fn program_exists(program: &str) -> bool {
    if program.contains('/') {
        return is_executable_file(Path::new(program));
    }
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| is_executable_file(&dir.join(program)))
}

/// Why `r` cannot run, or `None` when it can. A saved id is never a problem
/// here: whether it exists is checked against the function list.
pub fn problem(r: &FuncRef) -> Option<String> {
    match r {
        FuncRef::Saved(_) => None,
        FuncRef::Bash { cmd, .. } => cmd.trim().is_empty().then(|| "the bash command is empty".to_string()),
        FuncRef::Exec { line } => {
            let words = match tokenize(line) {
                Ok(words) => words,
                Err(e) => return Some(format!("exec: {e}")),
            };
            let Some(program) = words.first() else { return Some("exec: no program given".to_string()) };
            (!program_exists(program)).then(|| format!("exec: '{program}' was not found"))
        }
    }
}

/// A short text for an inline reference (`bash: ls`, `exec: kitty`), or
/// `None` for a saved Function.
pub fn describe(r: &FuncRef) -> Option<String> {
    match r {
        FuncRef::Saved(_) => None,
        FuncRef::Bash { cmd, .. } => Some(format!("bash: {cmd}")),
        FuncRef::Exec { line } => Some(format!("exec: {line}")),
    }
}

/// The steps that run an inline reference: one `Run` step. Fails with the
/// [`problem`] text, and for a saved Function (it has its own steps).
pub fn to_steps(r: &FuncRef) -> Result<Vec<FnStep>, String> {
    if let FuncRef::Saved(id) = r {
        return Err(format!("'{id}' is a saved function, not an inline command"));
    }
    if let Some(reason) = problem(r) {
        return Err(reason);
    }
    let mut args = InternalArgs::default();
    match r {
        FuncRef::Saved(_) => {}
        FuncRef::Bash { cmd, show_output } => {
            args.0.insert("command".to_string(), format!("bash -c {}", quote(cmd)));
            if *show_output {
                args.0.insert("window".to_string(), BOOL_TRUE.to_string());
            }
        }
        FuncRef::Exec { line } => {
            args.0.insert("command".to_string(), line.trim().to_string());
        }
    }
    Ok(vec![FnStep::Internal { cmd: InternalCmd::Run, args }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_args(r: &FuncRef) -> InternalArgs {
        match to_steps(r).unwrap().remove(0) {
            FnStep::Internal { cmd: InternalCmd::Run, args } => args,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_string_without_a_prefix_is_a_saved_id() {
        assert_eq!(parse("fn-1a-2"), FuncRef::Saved("fn-1a-2".into()));
        assert_eq!(parse(""), FuncRef::Saved(String::new()));
    }

    #[test]
    fn parse_and_format_round_trip_every_kind() {
        for text in ["fn-1a-2", "bash:ls -l", "bash+out:ls -l", "exec:kitty --class x", "bash:echo a:b", "exec:"] {
            assert_eq!(format(&parse(text)), text);
        }
        assert_eq!(parse("bash+out:ls"), FuncRef::Bash { cmd: "ls".into(), show_output: true });
        assert_eq!(parse("bash:ls"), FuncRef::Bash { cmd: "ls".into(), show_output: false });
    }

    #[test]
    fn quote_survives_tokenize_for_awkward_text() {
        for text in ["plain", "two words", "it's", "say \"hi\"", "back\\slash", "$HOME {a,b} ${HOME}", "", "line1\nline2", "'"] {
            assert_eq!(tokenize(&quote(text)).unwrap(), vec![text.to_string()], "{text:?}");
        }
    }

    #[test]
    fn a_bash_command_becomes_one_quoted_argument_of_bash_c() {
        let r = parse("bash:echo 'x' | wc -l");
        let command = run_args(&r).get("command").unwrap().to_string();
        assert_eq!(tokenize(&command).unwrap(), vec!["bash", "-c", "echo 'x' | wc -l"]);
        assert!(!run_args(&r).flag("window"));
    }

    #[test]
    fn show_output_sets_the_window_flag() {
        assert!(run_args(&parse("bash+out:ls")).flag("window"));
    }

    #[test]
    fn exec_keeps_the_line_and_never_opens_a_window() {
        let args = run_args(&parse("exec:sh -c 'echo hi'"));
        assert_eq!(args.get("command"), Some("sh -c 'echo hi'"));
        assert!(!args.flag("window"));
    }

    #[test]
    fn exec_splits_a_quoted_program_path_with_a_space() {
        let words = tokenize("\"/opt/my app/run\" --flag").unwrap();
        assert_eq!(words, vec!["/opt/my app/run", "--flag"]);
    }

    #[test]
    fn problem_reports_empty_bash_blank_exec_and_unbalanced_quotes() {
        assert!(problem(&parse("bash:   ")).is_some());
        assert!(problem(&parse("exec:  ")).is_some());
        assert!(problem(&parse("exec:sh 'oops")).unwrap().contains("quote"));
        assert_eq!(problem(&parse("bash:ls")), None);
        assert_eq!(problem(&parse("fn-1")), None);
    }

    #[test]
    fn problem_checks_that_the_program_exists() {
        assert_eq!(problem(&parse("exec:sh -c true")), None);
        assert_eq!(problem(&parse("exec:/bin/sh")), None);
        assert!(problem(&parse("exec:opusdm-no-such-program-xyz")).unwrap().contains("was not found"));
        assert!(problem(&parse("exec:/no/such/dir/prog")).is_some());
    }

    #[test]
    fn program_exists_rejects_a_file_without_the_execute_bit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain");
        std::fs::write(&file, "x").unwrap();
        assert!(!program_exists(file.to_str().unwrap()));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(program_exists(file.to_str().unwrap()));
    }

    #[test]
    fn to_steps_refuses_a_saved_id_and_a_broken_command() {
        assert!(to_steps(&parse("fn-1")).is_err());
        assert!(to_steps(&parse("bash:")).is_err());
    }

    #[test]
    fn describe_names_inline_commands_only() {
        assert_eq!(describe(&parse("bash+out:ls")), Some("bash: ls".to_string()));
        assert_eq!(describe(&parse("exec:kitty")), Some("exec: kitty".to_string()));
        assert_eq!(describe(&parse("fn-1")), None);
    }
}
```

Check that `InternalArgs` derives `Default` and has `get(&str) -> Option<&str>`:

Run: `grep -n "pub struct InternalArgs" -B3 -A12 crates/ipc/src/function.rs`
Expected: `#[derive(... Default ...)]` on `InternalArgs` and a `pub fn get`. If `Default` is missing, build the value with `InternalArgs(std::collections::BTreeMap::new())` in `to_steps` instead.

- [ ] **Step 2: Register the module and run the tests**

In `crates/ipc/src/lib.rs` add `pub mod funcref;` between `pub mod function;` and `pub mod glob;`.

Run: `cargo test -p opusdm-ipc funcref 2>&1 | tail -30`
Expected: all `funcref::tests::*` PASS. Fix any failure in the module (not in the tests) unless the test itself is wrong.

- [ ] **Step 3: Correct the spec**

In `docs/superpowers/specs/2026-10-05-inline-commands-design.md`:
- In the `to_steps` bullet, replace the sentences that mention `shlex::try_quote` and `shlex::split` with: "quoted with `funcref::quote` (single quotes, `\` and `'` escaped), and split with the same `cli::tokenize` the `Run` step uses".
- In the `problem` bullet for `Exec`, replace "(`shlex::split` returns `None`)" with "(`cli::tokenize` fails)".
- Delete the line "New dependency: `shlex` in `crates/ipc` ...".
- Add under Design, after the reference format table: "No `shlex`: the `Run` step already splits its command with `cli::tokenize`, and validation must split exactly as execution does."

- [ ] **Step 4: Commit**

```bash
git add crates/ipc/src/funcref.rs crates/ipc/src/lib.rs docs/superpowers/specs/2026-10-05-inline-commands-design.md
git commit -m "feat(ipc): funcref module for inline bash and exec references"
```

---

### Task 2: Validators and prune accept inline references

**Files:**
- Modify: `crates/ipc/src/bindings.rs` (`BindingError`, its three accessors, `Display`, `validate_hotkeys`, `walk_menu`, `validate_scripts`, `prune`, tests)

**Interfaces:**
- Consumes: `funcref::{parse, problem, FuncRef}` from Task 1.
- Produces:
  - `BindingError::HotkeyInvalidCommand { index: usize, reason: String }`
  - `BindingError::MenuInvalidCommand { path: Vec<usize>, reason: String }`
  - `BindingError::ScriptInvalidCommand { index: usize, reason: String }`
  - The existing validators keep their signatures; `known` still holds saved ids only.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `crates/ipc/src/bindings.rs`:

```rust
    #[test]
    fn validate_hotkeys_accepts_a_good_inline_command_and_reports_a_bad_one() {
        let list = [hk("Ctrl+u", "bash:ls", false), hk("Ctrl+j", "exec:sh -c true", true), hk("Ctrl+k", "bash:  ", false), hk("Ctrl+l", "exec:opusdm-nope-xyz", false)];
        let errors = validate_hotkeys(&list, &known(&[]));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(matches!(&errors[0], BindingError::HotkeyInvalidCommand { index: 2, .. }));
        assert!(matches!(&errors[1], BindingError::HotkeyInvalidCommand { index: 3, reason } if reason.contains("was not found")));
        assert_eq!(errors[0].hotkey_index(), Some(2));
    }

    #[test]
    fn validate_menu_and_scripts_accept_inline_commands() {
        let menu = vec![
            MenuNode::Entry { label: "ok".into(), function: "bash:ls".into() },
            MenuNode::Entry { label: "bad".into(), function: "exec:".into() },
        ];
        let errors = validate_menu(&menu, &known(&[]));
        assert!(matches!(errors.as_slice(), [BindingError::MenuInvalidCommand { path, .. }] if path == &vec![1]));
        let scripts = [
            ScriptBinding { event: ScriptEvent::Startup, function: "bash:ls".into() },
            ScriptBinding { event: ScriptEvent::Shutdown, function: "bash:".into() },
        ];
        let errors = validate_scripts(&scripts, &known(&[]));
        assert!(matches!(errors.as_slice(), [BindingError::ScriptInvalidCommand { index: 1, .. }]));
        assert_eq!(errors[0].script_index(), Some(1));
    }

    #[test]
    fn a_saved_id_that_is_missing_is_still_the_old_error() {
        let errors = validate_hotkeys(&[hk("Ctrl+u", "gone", false)], &known(&["a"]));
        assert_eq!(errors, vec![BindingError::HotkeyUnknownFunction { index: 0 }]);
    }

    #[test]
    fn prune_keeps_valid_inline_entries_and_drops_broken_ones() {
        let menu = vec![
            MenuNode::Entry { label: "keep".into(), function: "bash:ls".into() },
            MenuNode::Entry { label: "drop".into(), function: "exec:".into() },
        ];
        let pruned = prune(&menu, &known(&[]));
        assert_eq!(pruned.len(), 1);
        assert!(matches!(&pruned[0], MenuNode::Entry { label, .. } if label == "keep"));
    }

    #[test]
    fn sanitize_keeps_inline_hotkeys_even_when_the_program_is_missing() {
        let dirty = Bindings { hotkeys: vec![hk("ctrl+U", "exec:opusdm-nope-xyz", true)], ..Bindings::default() };
        let clean = sanitize(dirty);
        assert_eq!(clean.hotkeys, vec![hk("Ctrl+u", "exec:opusdm-nope-xyz", true)]);
    }

    #[test]
    fn inline_references_round_trip_through_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let bindings = Bindings {
            hotkeys: vec![hk("Ctrl+Alt+t", "exec:kitty --class \"my term\"", true)],
            menu: vec![MenuNode::Entry { label: "L".into(), function: "bash+out:echo 'a b' && ls".into() }],
            scripts: vec![ScriptBinding { event: ScriptEvent::Startup, function: "bash:notify-send hi".into() }],
        };
        save_bindings_to(dir.path(), &bindings).unwrap();
        assert_eq!(load_bindings_from(dir.path()), bindings);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p opusdm-ipc bindings 2>&1 | tail -20`
Expected: compile error, `no variant named HotkeyInvalidCommand`.

- [ ] **Step 3: Implement**

In `crates/ipc/src/bindings.rs`:

1. Add `use crate::funcref::{self, FuncRef};` next to the other imports.
2. Add the three variants to `BindingError`, each with a doc comment:
   ```rust
       /// An inline `bash:` or `exec:` command that cannot run.
       HotkeyInvalidCommand { index: usize, reason: String },
       /// An inline command in a menu entry that cannot run.
       MenuInvalidCommand { path: Vec<usize>, reason: String },
       /// An inline command in a script binding that cannot run.
       ScriptInvalidCommand { index: usize, reason: String },
   ```
3. Extend the accessors: `hotkey_index` gets `| BindingError::HotkeyInvalidCommand { index, .. }`, `script_index` gets `| BindingError::ScriptInvalidCommand { index, .. }`, `menu_path` gets `| BindingError::MenuInvalidCommand { path, .. }`.
4. Extend `Display`:
   ```rust
               BindingError::HotkeyInvalidCommand { reason, .. }
               | BindingError::MenuInvalidCommand { reason, .. }
               | BindingError::ScriptInvalidCommand { reason, .. } => write!(f, "invalid command: {reason}"),
   ```
5. Add the helper above `validate_hotkeys`:
   ```rust
   /// Why a function reference cannot be used: it names a saved function
   /// that does not exist, or an inline command that cannot run.
   enum RefIssue {
       /// A saved id that is not in the function list.
       Missing,
       /// An inline command with this problem.
       Invalid(String),
   }

   /// The issue with `reference`, if any. A saved id is checked against
   /// `known`; an inline command by [`funcref::problem`].
   fn check_reference(reference: &str, known: &HashSet<FunctionId>) -> Option<RefIssue> {
       match funcref::parse(reference) {
           FuncRef::Saved(id) => (!known.contains(&id)).then_some(RefIssue::Missing),
           inline => funcref::problem(&inline).map(RefIssue::Invalid),
       }
   }
   ```
6. In `validate_hotkeys` replace the final `if !known.contains(&hotkey.function) { ... }` with:
   ```rust
           match check_reference(&hotkey.function, known) {
               Some(RefIssue::Missing) => errors.push(BindingError::HotkeyUnknownFunction { index }),
               Some(RefIssue::Invalid(reason)) => errors.push(BindingError::HotkeyInvalidCommand { index, reason }),
               None => {}
           }
   ```
7. In `walk_menu`, replace `if !known.contains(function) { errors.push(MenuUnknownFunction ...) }` with the same three-arm match using `MenuUnknownFunction { path: path.clone() }` and `MenuInvalidCommand { path: path.clone(), reason }`.
8. In `validate_scripts`, replace the `ScriptUnknownFunction` check with the same match using `ScriptUnknownFunction { index }` and `ScriptInvalidCommand { index, reason }`.
9. In `prune`, replace `known.contains(function).then(|| node.clone())` with `check_reference(function, known).is_none().then(|| node.clone())`.
10. Update the module doc of `prune` is unnecessary; update the doc comment of `prune` to say "entries whose function is neither a saved function in `known` nor a runnable inline command".

- [ ] **Step 4: Run all ipc tests**

Run: `cargo test -p opusdm-ipc 2>&1 | tail -15`
Expected: all PASS, including the older `validate_*`, `prune` and `sanitize` tests.

- [ ] **Step 5: Build the workspace to find exhaustive matches on `BindingError`**

Run: `cargo build --workspace 2>&1 | tail -20`
Expected: builds. If a `match` on `BindingError` elsewhere is not exhaustive, add the new variants next to their siblings.

- [ ] **Step 6: Commit**

```bash
git add crates/ipc/src/bindings.rs
git commit -m "feat(ipc): validate and prune accept inline bash and exec references"
```

---

### Task 3: The hub runs inline references

**Files:**
- Modify: `crates/hub/src/server/function_run.rs` (`run_function_by_id`, new `run_inline`)
- Modify: `crates/hub/src/server.rs` (tests module only, near `run_function_by_id_runs_a_saved_function`, about line 2244)

**Interfaces:**
- Consumes: `funcref::{parse, to_steps, FuncRef}`; `Hub::start_function_run(requester: Option<u64>, source: Option<u64>, steps: Vec<FnStep>) -> u64`; `Hub::function_failed(requester: Option<u64>, error: String)`.
- Produces: `run_function_by_id` accepts `bash:`, `bash+out:` and `exec:` ids from every trigger (`ToHub::RunFunctionById`, `HubCmd::RunFunctionHeadless`).

- [ ] **Step 1: Write the failing tests**

In the `tests` module of `crates/hub/src/server.rs`, after `run_function_by_id_runs_a_saved_function`, add (they reuse `ENV_LOCK`, `bind_temp`, `spawn_server`, `connect`, `send`, `recv`, `wait_for_lines`):

```rust
    #[test]
    fn run_function_by_id_runs_an_inline_bash_command() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        // `{a,b}` is bash brace expansion, not a placeholder: it must reach bash untouched.
        let id = format!("bash:echo {{a,b}} \"$((1+1))\" >> {}", marker.display());
        send(&mut a, &ToHub::RunFunctionById { id }).unwrap();
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

        wait_for_lines(&marker, 1);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "a b 2\n");
    }

    #[test]
    fn run_function_by_id_runs_an_inline_exec_line() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");

        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        let id = format!("exec:sh -c 'echo x >> {}'", marker.display());
        send(&mut a, &ToHub::RunFunctionById { id }).unwrap();
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

        wait_for_lines(&marker, 1);
    }

    #[test]
    fn an_inline_command_that_cannot_run_reports_failure() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let (_td, listener, path) = bind_temp();
        let _handle = spawn_server(listener, Box::new(|_p| {}));
        let mut a = connect(&path);
        send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into() }).unwrap();
        let _w: Welcome = recv(&mut a).unwrap().unwrap();

        send(&mut a, &ToHub::RunFunctionById { id: "exec:opusdm-no-such-program-xyz".into() }).unwrap();
        let got: ToLister = recv(&mut a).unwrap().unwrap();
        match got {
            ToLister::FunctionFailed { error, .. } => assert!(error.contains("was not found"), "{error}"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn headless_run_of_an_inline_command_works_with_no_lister_open() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", cfg.path());
        let marker = out.path().join("marker");

        let (_td, listener, _path) = bind_temp();
        let handle = spawn_server(listener, Box::new(|_p| {}));
        let id = format!("bash:echo x >> {}", marker.display());
        handle.cmd_tx.send(HubCmd::RunFunctionHeadless(id)).unwrap();

        wait_for_lines(&marker, 1);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p opusdm-hub inline 2>&1 | tail -25`
Expected: FAIL (`function 'bash:...' not found` reported as `FunctionFailed`; the headless one times out).

- [ ] **Step 3: Implement**

In `crates/hub/src/server/function_run.rs`, add the doc'd helper inside the `impl Hub` block, and the early return in `run_function_by_id`:

```rust
    /// Start the inline command `reference` (`bash:`, `bash+out:` or
    /// `exec:`) as a one-step run for `requester`. A command that cannot run
    /// is reported through [`Self::function_failed`]. Returns the run id.
    fn run_inline(&mut self, requester: Option<u64>, source: Option<u64>, reference: &opusdm_ipc::funcref::FuncRef) -> Option<u64> {
        match opusdm_ipc::funcref::to_steps(reference) {
            Ok(steps) => Some(self.start_function_run(requester, source, steps)),
            Err(e) => {
                self.function_failed(requester, e);
                None
            }
        }
    }
```

At the top of `run_function_by_id`, before the `let Some(function) = ...` line:

```rust
        let reference = opusdm_ipc::funcref::parse(id);
        if !matches!(reference, opusdm_ipc::funcref::FuncRef::Saved(_)) {
            return self.run_inline(requester, source, &reference);
        }
```

Update the doc comment of `run_function_by_id` to start: "Run the inline command `id` names (`bash:`, `exec:`), else load the saved function `id`, ...".

- [ ] **Step 4: Run the hub tests**

Run: `cargo test -p opusdm-hub 2>&1 | tail -20`
Expected: all PASS, including the four new tests and the existing saved-function ones.

- [ ] **Step 5: Commit**

```bash
git add crates/hub/src/server/function_run.rs crates/hub/src/server.rs
git commit -m "feat(hub): run inline bash and exec references"
```

---

### Task 4: The reference picker widget

**Files:**
- Modify: `crates/lister/src/binding_ui.rs` (keep `function_dropdown` and `selected_function`: the user menu and script editors still use them until phase 2)

**Interfaces:**
- Consumes: `funcref::{parse, format, quote, FuncRef}`; `crate::...::Function`.
- Produces (used by Task 5):
  - `pub enum RefKind { Function, Bash, Exec }` with `pub const ALL: [RefKind; 3]`, `pub fn label(self) -> &'static str`
  - `pub fn browsed_line(current: &str, program: &Path) -> String`
  - `pub fn reference_picker(functions: &[Function], current: &str, parent: &gtk::Window, on_change: impl Fn(String) + 'static) -> gtk::Box`
  - `on_change` receives the new reference string on every edit (kind, function, text, check box, Browse). It never asks the caller to rebuild: the picker keeps every kind's draft while it lives.

- [ ] **Step 1: Write the failing tests for the pure helpers**

Append to `crates/lister/src/binding_ui.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_reference_selects_the_matching_kind_and_keeps_every_draft() {
        let d = Draft::from_reference("bash+out:ls -l");
        assert_eq!((d.kind, d.bash.as_str(), d.show_output), (RefKind::Bash, "ls -l", true));
        assert_eq!(d.reference(), "bash+out:ls -l");
        let d = Draft::from_reference("exec:kitty --x");
        assert_eq!((d.kind, d.exec.as_str()), (RefKind::Exec, "kitty --x"));
        let d = Draft::from_reference("fn-1-2");
        assert_eq!((d.kind, d.function.as_str()), (RefKind::Function, "fn-1-2"));
    }

    #[test]
    fn switching_kind_keeps_what_was_typed_for_the_others() {
        let mut d = Draft::from_reference("bash:ls");
        d.kind = RefKind::Exec;
        d.exec = "kitty".into();
        assert_eq!(d.reference(), "exec:kitty");
        d.kind = RefKind::Bash;
        assert_eq!(d.reference(), "bash:ls");
        d.kind = RefKind::Function;
        assert_eq!(d.reference(), "", "no function chosen yet");
    }

    #[test]
    fn browsed_line_replaces_the_program_and_keeps_the_arguments() {
        let prog = Path::new("/usr/bin/kitty");
        assert_eq!(browsed_line("", prog), "/usr/bin/kitty");
        assert_eq!(browsed_line("old --class 'my term'", prog), "/usr/bin/kitty --class 'my term'");
        assert_eq!(browsed_line("old", Path::new("/opt/my app/run")), "'/opt/my app/run'");
        assert_eq!(browsed_line("old 'unclosed", prog), "/usr/bin/kitty", "an unparsable line loses its arguments");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p opusdm-lister binding_ui 2>&1 | tail -15`
Expected: compile error, `Draft` / `browsed_line` not found.

- [ ] **Step 3: Implement**

Replace the imports at the top of `crates/lister/src/binding_ui.rs` and append the new code before the `tests` module:

```rust
use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::funcref::{self, FuncRef};
use opusdm_ipc::function::{Function, FunctionId};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
```

```rust
/// What a function reference points at; the order of the kind drop-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A saved Function.
    Function,
    /// A bash command.
    Bash,
    /// A program started directly.
    Exec,
}

impl RefKind {
    /// Every kind, in drop-down order.
    pub const ALL: [RefKind; 3] = [RefKind::Function, RefKind::Bash, RefKind::Exec];

    /// The drop-down text.
    pub fn label(self) -> &'static str {
        match self {
            RefKind::Function => "Function",
            RefKind::Bash => "Bash",
            RefKind::Exec => "Exec",
        }
    }
}

/// What a picker holds: the chosen kind plus the last value of every kind, so
/// switching kind and back loses nothing.
struct Draft {
    kind: RefKind,
    function: FunctionId,
    bash: String,
    show_output: bool,
    exec: String,
}

impl Draft {
    /// The draft for a stored reference string.
    fn from_reference(reference: &str) -> Draft {
        let mut draft = Draft { kind: RefKind::Function, function: String::new(), bash: String::new(), show_output: false, exec: String::new() };
        match funcref::parse(reference) {
            FuncRef::Saved(id) => draft.function = id,
            FuncRef::Bash { cmd, show_output } => {
                draft.kind = RefKind::Bash;
                draft.bash = cmd;
                draft.show_output = show_output;
            }
            FuncRef::Exec { line } => {
                draft.kind = RefKind::Exec;
                draft.exec = line;
            }
        }
        draft
    }

    /// The reference string for the chosen kind.
    fn reference(&self) -> String {
        match self.kind {
            RefKind::Function => self.function.clone(),
            RefKind::Bash => funcref::format(&FuncRef::Bash { cmd: self.bash.clone(), show_output: self.show_output }),
            RefKind::Exec => funcref::format(&FuncRef::Exec { line: self.exec.clone() }),
        }
    }
}

/// Whether `word` must be quoted to stay one word in an exec line.
fn needs_quote(word: &str) -> bool {
    word.is_empty() || word.chars().any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
}

/// `current` (an exec line) with its program replaced by `program`, keeping
/// the arguments. A line that does not parse loses its arguments.
pub fn browsed_line(current: &str, program: &Path) -> String {
    let program = program.to_string_lossy();
    let mut words = vec![if needs_quote(&program) { funcref::quote(&program) } else { program.to_string() }];
    if let Ok(old) = opusdm_ipc::cli::tokenize(current) {
        words.extend(old.iter().skip(1).map(|w| if needs_quote(w) { funcref::quote(w) } else { w.clone() }));
    }
    words.join(" ")
}

/// A control that edits one function reference: a kind drop-down (Function,
/// Bash, Exec) followed by the matching fields. `current` is the stored
/// string; `on_change` gets the new string after every edit. `parent` owns
/// the Browse dialog.
pub fn reference_picker(functions: &[Function], current: &str, parent: &gtk::Window, on_change: impl Fn(String) + 'static) -> gtk::Box {
    let draft = Rc::new(RefCell::new(Draft::from_reference(current)));
    let on_change: Rc<dyn Fn(String)> = Rc::new(on_change);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let kinds: Vec<&str> = RefKind::ALL.iter().map(|k| k.label()).collect();
    let kind_dd = gtk::DropDown::from_strings(&kinds);
    kind_dd.set_selected(RefKind::ALL.iter().position(|k| *k == draft.borrow().kind).unwrap_or(0) as u32);
    line.append(&kind_dd);

    let function_dd = function_dropdown(functions, &draft.borrow().function);
    function_dd.set_hexpand(true);
    line.append(&function_dd);

    let bash_entry = gtk::Entry::new();
    bash_entry.set_hexpand(true);
    bash_entry.set_placeholder_text(Some("bash command"));
    bash_entry.set_text(&draft.borrow().bash);
    let output = gtk::CheckButton::with_label("Show output");
    output.set_active(draft.borrow().show_output);
    line.append(&bash_entry);
    line.append(&output);

    let exec_entry = gtk::Entry::new();
    exec_entry.set_hexpand(true);
    exec_entry.set_placeholder_text(Some("program and arguments"));
    exec_entry.set_text(&draft.borrow().exec);
    let browse = gtk::Button::with_label("Browse...");
    line.append(&exec_entry);
    line.append(&browse);

    // Show only the widgets of the chosen kind.
    let show_kind = {
        let (function_dd, bash_entry, output, exec_entry, browse) =
            (function_dd.clone(), bash_entry.clone(), output.clone(), exec_entry.clone(), browse.clone());
        Rc::new(move |kind: RefKind| {
            function_dd.set_visible(kind == RefKind::Function);
            bash_entry.set_visible(kind == RefKind::Bash);
            output.set_visible(kind == RefKind::Bash);
            exec_entry.set_visible(kind == RefKind::Exec);
            browse.set_visible(kind == RefKind::Exec);
        })
    };
    show_kind(draft.borrow().kind);

    {
        let (draft, on_change, show_kind) = (draft.clone(), on_change.clone(), show_kind.clone());
        kind_dd.connect_selected_notify(move |d| {
            let Some(kind) = RefKind::ALL.get(d.selected() as usize).copied() else { return };
            draft.borrow_mut().kind = kind;
            show_kind(kind);
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change, functions) = (draft.clone(), on_change.clone(), functions.to_vec());
        function_dd.connect_selected_notify(move |d| {
            let Some(id) = selected_function(d, &functions) else { return };
            draft.borrow_mut().function = id;
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change) = (draft.clone(), on_change.clone());
        bash_entry.connect_changed(move |e| {
            draft.borrow_mut().bash = e.text().to_string();
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change) = (draft.clone(), on_change.clone());
        output.connect_toggled(move |c| {
            draft.borrow_mut().show_output = c.is_active();
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change) = (draft.clone(), on_change.clone());
        exec_entry.connect_changed(move |e| {
            draft.borrow_mut().exec = e.text().to_string();
            on_change(draft.borrow().reference());
        });
    }
    {
        let (entry, parent) = (exec_entry.clone(), parent.clone());
        browse.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Choose a program").build();
            let entry = entry.clone();
            dialog.open(Some(&parent), gio::Cancellable::NONE, move |res| {
                let Some(path) = res.ok().and_then(|file| file.path()) else { return };
                // Setting the text fires `changed`, which updates the draft.
                entry.set_text(&browsed_line(&entry.text(), &path));
            });
        });
    }
    line
}
```

If the `functions: &[Function]` slice cannot be captured (the closure needs `'static`), the `to_vec()` above already clones it (`Function: Clone`).

- [ ] **Step 4: Run the tests and build**

Run: `cargo test -p opusdm-lister binding_ui 2>&1 | tail -15 && cargo build -p opusdm-lister 2>&1 | tail -15`
Expected: the three tests PASS; the build succeeds (the picker is not used yet, so a `dead_code` warning for `reference_picker` is expected and goes away in Task 5).

- [ ] **Step 5: Commit**

```bash
git add crates/lister/src/binding_ui.rs
git commit -m "feat(lister): reference picker widget for functions, bash and exec"
```

---

### Task 5: Edit Hotkeys and Find Key use the picker

**Files:**
- Modify: `crates/lister/src/hotkey_editor.rs`
- Modify: `docs/usage.md` (section 3.13), `CHANGES.md`, `FEATURES.md` (F-03 note)

**Interfaces:**
- Consumes: `binding_ui::reference_picker`, `funcref::{parse, describe, FuncRef}`, the new `BindingError` variants (their `hotkey_index()` already works).
- Produces: rows with a Function/Bash/Exec picker; `KeyMeaning::UserCommand { command: String, global: bool }`.

- [ ] **Step 1: Write the failing Find Key tests**

In the `tests` module of `crates/lister/src/hotkey_editor.rs` add:

```rust
    #[test]
    fn an_inline_hotkey_is_reported_as_a_command_not_a_function() {
        let hotkeys = [Hotkey { key: "Ctrl+Alt+t".into(), function: "exec:kitty --class x".into(), global: true }];
        assert_eq!(
            find_key(&hotkeys, &[], &combo("Ctrl+Alt+t")),
            KeyMeaning::UserCommand { command: "exec: kitty --class x".into(), global: true }
        );
        let hotkeys = [Hotkey { key: "F9".into(), function: "bash+out:ls -l".into(), global: false }];
        assert_eq!(
            find_key(&hotkeys, &[], &combo("F9")),
            KeyMeaning::UserCommand { command: "bash: ls -l".into(), global: false }
        );
    }
```

Run: `cargo test -p opusdm-lister hotkey_editor 2>&1 | tail -10`
Expected: compile error, no variant `UserCommand`.

- [ ] **Step 2: `KeyMeaning::UserCommand` and `find_key`**

In `hotkey_editor.rs`:

1. Add the variant to `KeyMeaning` with a doc comment: `/// A user hotkey runs an inline bash or exec command.` `UserCommand { command: String, global: bool },`
2. In `find_key`, replace the `let name = ...; return KeyMeaning::UserFunction { name, global: hotkey.global };` block with:
   ```rust
        let reference = opusdm_ipc::funcref::parse(&hotkey.function);
        if let Some(command) = opusdm_ipc::funcref::describe(&reference) {
            return KeyMeaning::UserCommand { command, global: hotkey.global };
        }
        let name = functions
            .iter()
            .find(|f| f.id == hotkey.function)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "(missing function)".to_string());
        return KeyMeaning::UserFunction { name, global: hotkey.global };
   ```
3. In `open_find_key`, add the arm after `UserFunction`:
   ```rust
                KeyMeaning::UserCommand { command, global } => {
                    format!("Runs the command \"{command}\"{}", if global { " (global hotkey)" } else { "" })
                }
   ```

Run: `cargo test -p opusdm-lister hotkey_editor 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 3: Rows use the picker, with in-place error refresh**

Typing in the command field must not rebuild the rows (it would drop the keyboard focus), so errors are repainted in place.

1. `State` gets a field `notes: RefCell<Vec<gtk::Box>>` (doc: "One box per row, holding that row's error and built-in notes, so they can be repainted without rebuilding the row."). Initialise it with `RefCell::new(Vec::new())` in `build_editor`.
2. Replace `rebuild` and `build_row` with:

```rust
/// Repaint every row from the state and re-run the validation that gates
/// Save.
fn rebuild(state: &Rc<State>) {
    while let Some(child) = state.rows_box.first_child() {
        state.rows_box.remove(&child);
    }
    state.notes.borrow_mut().clear();
    let hotkeys = state.hotkeys.borrow().clone();
    for (index, hotkey) in hotkeys.iter().enumerate() {
        state.rows_box.append(&build_row(state, index, hotkey));
    }
    revalidate(state);
}

/// Re-run the validation and repaint every row's notes in place, without
/// touching the widgets that have the keyboard focus. Save is enabled only
/// when nothing is wrong.
fn revalidate(state: &Rc<State>) {
    let hotkeys = state.hotkeys.borrow().clone();
    let errors = bindings::validate_hotkeys(&hotkeys, &known_ids(&state.functions));
    for (index, notes) in state.notes.borrow().iter().enumerate() {
        let Some(hotkey) = hotkeys.get(index) else { continue };
        fill_notes(notes, index, hotkey, &errors);
    }
    state.save_btn.set_sensitive(errors.is_empty());
}

/// Replace the content of a row's `notes` box with that row's errors and its
/// built-in override note.
fn fill_notes(notes: &gtk::Box, index: usize, hotkey: &Hotkey, errors: &[BindingError]) {
    while let Some(child) = notes.first_child() {
        notes.remove(&child);
    }
    for error in errors.iter().filter(|e| e.hotkey_index() == Some(index)) {
        notes.append(&note_label(&error.to_string(), true));
    }
    if let Ok(combo) = parse_key(&hotkey.key) {
        if let KeyMeaning::Builtin { title } = find_key(&[], &[], &combo) {
            notes.append(&note_label(&format!("Overrides the built-in binding: {title}"), false));
        }
    }
}

/// One editable row: key button, function picker (saved function, bash
/// command or program), Global check box, remove button, then this row's
/// notes underneath.
fn build_row(state: &Rc<State>, index: usize, hotkey: &Hotkey) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let key_text = if hotkey.key.is_empty() { "Press a key...".to_string() } else { hotkey.key.clone() };
    let key_btn = gtk::Button::with_label(&key_text);
    key_btn.set_size_request(190, -1);
    {
        let state = state.clone();
        key_btn.connect_clicked(move |_| capture_key(&state, index));
    }
    line.append(&key_btn);

    let picker = {
        let state = state.clone();
        crate::binding_ui::reference_picker(&state.functions.clone(), &hotkey.function, &state.window.clone(), move |reference| {
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                if h.function == reference {
                    return;
                }
                h.function = reference;
            }
            revalidate(&state);
        })
    };
    picker.set_hexpand(true);
    line.append(&picker);

    let global = gtk::CheckButton::with_label("Global");
    global.set_active(hotkey.global);
    {
        let state = state.clone();
        global.connect_toggled(move |c| {
            if let Some(h) = state.hotkeys.borrow_mut().get_mut(index) {
                h.global = c.is_active();
            }
            revalidate(&state);
        });
    }
    line.append(&global);

    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
    remove.add_css_class("flat");
    {
        let state = state.clone();
        remove.connect_clicked(move |_| {
            let mut hotkeys = state.hotkeys.borrow_mut();
            if index < hotkeys.len() {
                hotkeys.remove(index);
            }
            drop(hotkeys);
            rebuild(&state);
        });
    }
    line.append(&remove);
    row.append(&line);

    let notes = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.append(&notes);
    state.notes.borrow_mut().push(notes);
    row
}
```

3. `capture_key` still calls `rebuild(&state)` after storing the key: leave it (a key capture is a one-shot action, the focus is in the capture window).
4. Update the module doc comment of `hotkey_editor.rs` ("one row per binding (key, function, Global)") to "(key, function, bash command or program, Global)".
5. Remove the now-unused `crate::binding_ui::function_dropdown` / `selected_function` calls from this file (they are used inside the picker).

- [ ] **Step 4: Build and run the whole test suite**

Run: `cargo build --workspace 2>&1 | tail -20 && cargo test --workspace 2>&1 | tail -30`
Expected: builds without warnings from the files touched; all tests PASS.

- [ ] **Step 5: Manual check in the sandbox (the GTK widget is not unit-testable)**

Run the Xephyr sandbox as the project does (`./run-x11.sh`, see `docs/usage.md` / `CHANGES.md`), then, in the sandbox:
1. Desktop menu > Desktop > Edit Hotkeys. Click Add Hotkey, set the key to `Ctrl+Alt+t`, tick Global, choose **Exec**, type `xterm` (or any installed terminal). Expect: while the field holds a missing program the row shows `invalid command: exec: '...' was not found` and Save is disabled; typing keeps the focus in the field the whole time. Fix the name: the error goes and Save is enabled.
2. Click **Browse...**, pick an executable: the field gets its path, with spaces quoted.
3. Switch the kind to Bash, type `notify-send hi`, switch to Exec and back: both drafts are kept. Tick Show output with `bash+out:ls -l`.
4. Save, close all Listers, press `Ctrl+Alt+t`: the program starts with no Lister open.
5. Add a second row bound to the same key: duplicate error as before. Open a Lister and use Find Key on `Ctrl+Alt+t`: it answers `Runs the command "exec: xterm" (global hotkey)`.
6. Reopen Edit Hotkeys: the rows come back with the right kind selected.

Record the outcome in the commit message body if something had to be fixed.

- [ ] **Step 6: Documentation**

1. `docs/usage.md` section 3.13: replace "the function to run" with "what to run (a saved **Function**, a **Bash** command or an **Exec** program, see below)" and add after the rules list:

```markdown
**What a hotkey can run.** Each row has a kind selector:

- **Function**: a saved function, as before.
- **Bash**: a command line run by `bash -c`, so pipes, `&&` and variables work. Tick **Show output** to see its output in a window. `{f}`, `{o}` and the other placeholders of Manage Functions work in it; any other `{...}` (`{a,b}`, `${HOME}`) is left to bash.
- **Exec**: a program and its arguments, started directly with no shell (quotes group words, a backslash escapes the next character). **Browse...** picks the program. It is meant for graphical programs, for example a global `Ctrl+Alt+t` that opens a terminal.

A Bash or Exec hotkey runs in the SOURCE lister's folder, or in your home folder when no lister is open. A command that cannot run (empty, unbalanced quotes, a program that is not installed) is flagged on its row and Save stays disabled. The same two kinds will be offered in the user menu, script events, button banks and file types in a later change.
```

2. `CHANGES.md`, top of `## 2026-10-05`: add
```markdown
- Feature (ipc, hub, lister): a hotkey can run a **Bash** command or an **Exec** program as well as a saved Function. The choice is stored in the same `function` field with a prefix (`bash:`, `bash+out:`, `exec:`), so the JSON schema and the protocol are unchanged and old files load as before. New `opusdm_ipc::funcref` (parse, format, quote, `problem`, `to_steps`) turns an inline reference into one synthetic `Run` step, which the hub already executes; `run_function_by_id` resolves it, so global hotkeys work with no Lister open (the command starts in `$HOME`). The validators and `prune` accept runnable inline commands and report the rest on the row (`HotkeyInvalidCommand`, `MenuInvalidCommand`, `ScriptInvalidCommand`). Edit Hotkeys has a Function / Bash / Exec picker (`binding_ui::reference_picker`, with Browse...) and repaints errors in place while you type (`revalidate`); Find Key reports `Runs the command "..."`. The user menu, script events, button banks and file types get the same picker in phase 2.
```
3. `FEATURES.md`, under F-03: add one sentence "A hotkey may also run an inline `bash` command or an `exec` program (see `docs/superpowers/specs/2026-10-05-inline-commands-design.md`)." Read the F-03 section first and put it where it fits its style.

- [ ] **Step 7: Commit**

```bash
git add crates/lister/src/hotkey_editor.rs docs/usage.md CHANGES.md FEATURES.md
git commit -m "feat(lister): Edit Hotkeys can run a saved function, a bash command or a program"
```

---

## Self-Review

- **Spec coverage:** reference format (T1); `funcref` API (T1, `shlex` replaced by `cli::tokenize`, spec corrected in T1); hub resolution and `$HOME` headless behavior (T3); validation, new error variants, `sanitize` keeps inline refs, `prune` (T2); picker with Function/Bash/Exec, Show output, Browse with quoting and kept arguments (T4); Edit Hotkeys, Find Key, global hotkey (T5); docs (T5); phase 2 explicitly out of this plan; security note needs no code (the argument is passed as one quoted word, tested in T1).
- **Placeholders:** none; every code step has code. The only conditional is T1 Step 1's `Default` check, which gives the exact fallback.
- **Type consistency:** `FuncRef` variants, `parse/format/quote/problem/describe/to_steps/program_exists` are used with the same names in T2 to T5. `reference_picker(functions, current, parent, on_change)` matches its call in T5. `Draft` fields match between T4 code and tests. `KeyMeaning::UserCommand { command, global }` matches test and code.
- **Review Focus:** each line has a test or a manual check in the task named.
