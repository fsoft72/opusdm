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
