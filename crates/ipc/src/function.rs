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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn functions_path_is_under_config_dir() {
        assert_eq!(functions_path(), crate::settings::config_dir().join("functions.json"));
    }

    #[test]
    fn load_functions_missing_file_is_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        assert!(load_functions().is_empty());
    }

    #[test]
    fn save_and_load_functions_roundtrips() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let f = Function::new("Copy and notify");
        save_functions(&[f.clone()]).unwrap();
        let loaded = load_functions();
        assert_eq!(loaded, vec![f]);
    }
}
