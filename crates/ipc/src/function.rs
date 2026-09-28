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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
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

/// A recognized placeholder code, parsed out of the text between `{` and
/// `}` (e.g. `"f"`, `"do"`, `"df!"`).
struct Token {
    /// Whether the code draws from `dest` (`d`-prefixed) or `source`.
    is_dest: bool,
    /// Whether it names full paths (`f`/`F`) or names only (`o`/`O`).
    is_path: bool,
    /// Whether a missing `dest` expands to nothing instead of failing.
    optional: bool,
}

/// Parse the text between `{` and `}` as one of the eight recognized
/// placeholder codes, each optionally suffixed with `!`. Returns `None` for
/// anything else, so the caller can leave an unrecognized `{...}` in the
/// output untouched rather than mistake it for a placeholder.
fn parse_token(inner: &str) -> Option<Token> {
    let (code, optional) = match inner.strip_suffix('!') {
        Some(rest) => (rest, true),
        None => (inner, false),
    };
    if !matches!(code, "df" | "dF" | "do" | "dO" | "f" | "F" | "o" | "O") {
        return None;
    }
    Some(Token { is_dest: code.starts_with('d'), is_path: code.ends_with('f') || code.ends_with('F'), optional })
}

/// The raw (unquoted) values a `Token` names: full paths for `f`/`F`, file
/// names for `o`/`O`. `Ok(None)` means an optional `d`-prefixed code with no
/// `dest` to draw from - the caller treats that as "expands to nothing".
fn resolve_token<'a>(
    token: &Token,
    source: &'a SelectionCtx,
    dest: Option<&'a SelectionCtx>,
    raw_for_error: &str,
) -> Result<Option<&'a [PathBuf]>, ExpandError> {
    let ctx = if token.is_dest {
        match dest {
            Some(d) => Some(d),
            None if token.optional => None,
            None => return Err(ExpandError::MissingDest(format!("{{{raw_for_error}}}"))),
        }
    } else {
        Some(source)
    };
    Ok(ctx.map(|c| c.paths.as_slice()))
}

fn path_strings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.display().to_string()).collect()
}

fn name_strings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect()
}

/// Substitute every `{f}` `{F}` `{o}` `{O}` `{df}` `{dF}` `{do}` `{dO}`
/// (each also accepted with a trailing `!`, e.g. `{df!}`) in `text` with a
/// single left-to-right pass: multiple matching paths/names are joined with
/// a plain space, and an unrecognized `{...}` (including one that came from
/// a *substituted* value, such as a filename that itself contains literal
/// `{f}` text) is copied through untouched rather than re-scanned. There is
/// no shell-quoting - `External` steps run their argv directly, never
/// through a shell, so quote characters here would just become literal
/// output; see [`expand_argv_element`] for the per-path expansion a whole
/// argv element made of one placeholder gets instead.
pub fn expand_text(text: &str, source: &SelectionCtx, dest: Option<&SelectionCtx>) -> Result<String, ExpandError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let inner = &after_open[..close];
        match parse_token(inner) {
            Some(token) => {
                let values = match resolve_token(&token, source, dest, inner)? {
                    Some(paths) if token.is_path => path_strings(paths),
                    Some(paths) => name_strings(paths),
                    None => Vec::new(),
                };
                out.push_str(&values.join(" "));
            }
            None => {
                out.push('{');
                out.push_str(inner);
                out.push('}');
            }
        }
        rest = &after_open[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The result of expanding one argv element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandedArg {
    /// The element was exactly one placeholder naming a list (optionally
    /// `!`-suffixed): expands to zero or more raw, unquoted argv elements -
    /// one per selected path/name - rather than one joined string, since
    /// nothing downstream runs through a shell to split them back apart.
    Many(Vec<String>),
    /// Anything else (plain text, or a placeholder embedded in a larger
    /// string): plain [`expand_text`] expansion.
    One(String),
}

/// Expand one `External` step argv element. `text` trimmed is checked
/// against the eight placeholder forms first; only an exact whole-element
/// match gets the per-path [`ExpandedArg::Many`] treatment described there.
pub fn expand_argv_element(
    text: &str,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
) -> Result<ExpandedArg, ExpandError> {
    if let Some(inner) = text.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        if let Some(token) = parse_token(inner) {
            let values = match resolve_token(&token, source, dest, inner)? {
                Some(paths) if token.is_path => path_strings(paths),
                Some(paths) => name_strings(paths),
                None => Vec::new(),
            };
            return Ok(ExpandedArg::Many(values));
        }
    }
    Ok(ExpandedArg::One(expand_text(text, source, dest)?))
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
    fn f_embedded_in_text_expands_to_space_joined_raw_absolute_paths() {
        // No shell quoting: External steps run their argv directly, never
        // through a shell, so quote characters here would just become part
        // of the literal string. This embedded form only produces one
        // output string; `{f}` as a *whole* argv element is expanded
        // per-path instead (see `expand_argv_element` tests below).
        let src = ctx(&["/home/x/a.txt", "/home/x/b c.txt"], "/home/x");
        let out = expand_text("paths: {f}", &src, None).unwrap();
        assert_eq!(out, "paths: /home/x/a.txt /home/x/b c.txt");
    }

    #[test]
    fn o_embedded_in_text_expands_to_space_joined_raw_basenames() {
        let src = ctx(&["/home/x/a.txt", "/home/x/b c.txt"], "/home/x");
        let out = expand_text("notify-send done {o}", &src, None).unwrap();
        assert_eq!(out, "notify-send done a.txt b c.txt");
    }

    #[test]
    fn a_filename_containing_literal_placeholder_text_is_not_re_expanded() {
        // Regression for a real shell-injection-shaped bug: expansion must
        // be a single left-to-right pass over the *original* text. A
        // multi-pass implementation that re-scans its own output after
        // substituting one placeholder would expand a filename like
        // `;touch PWNED;{f}` a second time once it lands in `out`.
        let src = ctx(&["/home/x/;touch PWNED;{f}"], "/home/x");
        let out = expand_text("ls {f}", &src, None).unwrap();
        assert_eq!(out, "ls /home/x/;touch PWNED;{f}");
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
    fn whole_token_f_expands_to_one_argv_element_per_selected_path() {
        let src = ctx(&["/home/x/a.txt", "/home/x/b c.txt"], "/home/x");
        let out = expand_argv_element("{f}", &src, None).unwrap();
        assert_eq!(
            out,
            ExpandedArg::Many(vec!["/home/x/a.txt".to_string(), "/home/x/b c.txt".to_string()])
        );
    }

    #[test]
    fn whole_token_o_expands_to_one_argv_element_per_selected_name() {
        let src = ctx(&["/home/x/a.txt", "/home/x/b c.txt"], "/home/x");
        let out = expand_argv_element("{o}", &src, None).unwrap();
        assert_eq!(out, ExpandedArg::Many(vec!["a.txt".to_string(), "b c.txt".to_string()]));
    }

    #[test]
    fn whole_token_on_empty_selection_expands_to_zero_argv_elements() {
        let src = ctx(&[], "/home/x");
        assert_eq!(expand_argv_element("{f}", &src, None).unwrap(), ExpandedArg::Many(vec![]));
    }

    #[test]
    fn non_whole_token_text_still_expands_as_one_joined_element() {
        let src = ctx(&["/home/x/a.txt", "/home/x/b.txt"], "/home/x");
        let out = expand_argv_element("hi {o}", &src, None).unwrap();
        assert_eq!(out, ExpandedArg::One("hi a.txt b.txt".to_string()));
    }

    #[test]
    fn plain_argv_element_with_no_placeholder_passes_through() {
        let src = ctx(&[], "/home/x");
        let out = expand_argv_element("notify-send", &src, None).unwrap();
        assert_eq!(out, ExpandedArg::One("notify-send".to_string()));
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
        save_functions(std::slice::from_ref(&f)).unwrap();
        let loaded = load_functions();
        assert_eq!(loaded, vec![f]);
    }
}
