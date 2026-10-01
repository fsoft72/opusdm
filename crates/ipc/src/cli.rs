//! The internal command line (F-42). One line is either a shell command
//! (`!ls -l`), an internal command with arguments (`CopyAs new_name=*.bak`,
//! `MakeDir backup`) or the name of a saved function. Arguments may use the
//! usual codes (`{f}`, `{o}`, ...): they are expanded when the step runs,
//! exactly as in a saved function.

use crate::function::{schema, validate, FnStep, Function, FunctionId, InternalArgs, InternalCmd, BOOL_TRUE};

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    /// Run this text with `sh -c` in the SOURCE Lister's folder.
    Shell(String),
    /// Run one internal command.
    Internal { cmd: InternalCmd, args: InternalArgs },
    /// Run the saved function with this id.
    Function(FunctionId),
}

impl CliCommand {
    /// The steps to send to the hub for a shell or internal command; `None`
    /// for a saved function, which is run by id.
    pub fn steps(&self) -> Option<Vec<FnStep>> {
        match self {
            CliCommand::Shell(text) => Some(vec![FnStep::External { argv: vec!["sh".into(), "-c".into(), text.clone()], cwd_source: true }]),
            CliCommand::Internal { cmd, args } => Some(vec![FnStep::Internal { cmd: *cmd, args: args.clone() }]),
            CliCommand::Function(_) => None,
        }
    }
}

/// Short names accepted besides the command's own name and label.
const ALIASES: [(&str, InternalCmd); 4] =
    [("cd", InternalCmd::ChangeDir), ("md", InternalCmd::MakeDir), ("mkdir", InternalCmd::MakeDir), ("mv", InternalCmd::Move)];

/// Lower-case `text` without spaces, `_` and `-`, so `Make Directory`,
/// `make_directory` and `MAKEDIRECTORY` compare equal.
fn normalize(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, ' ' | '_' | '-')).flat_map(char::to_lowercase).collect()
}

/// The internal command called `word`, by variant name, label or alias.
pub fn find_command(word: &str) -> Option<InternalCmd> {
    let wanted = normalize(word);
    if let Some((_, cmd)) = ALIASES.iter().find(|(alias, _)| *alias == wanted) {
        return Some(*cmd);
    }
    InternalCmd::ALL.into_iter().find(|cmd| normalize(&format!("{cmd:?}")) == wanted || normalize(cmd.label()) == wanted)
}

/// Split `text` into words. Single and double quotes group (and are
/// removed); a backslash makes the next character literal. An unclosed quote
/// is an error.
pub fn tokenize(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') | (None, '\\') => match chars.next() {
                Some(next) => {
                    current.push(next);
                    started = true;
                }
                None => return Err("a backslash at the end of the line".to_string()),
            },
            (Some(_), c) => current.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("a quote is not closed".to_string());
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

/// Parse one command line. `functions` is the saved functions as (id, name),
/// used for lines that name one.
pub fn parse(line: &str, functions: &[(FunctionId, String)]) -> Result<CliCommand, String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("type a command".to_string());
    }
    if let Some(shell) = line.strip_prefix('!') {
        let shell = shell.trim();
        return if shell.is_empty() { Err("nothing to run after '!'".to_string()) } else { Ok(CliCommand::Shell(shell.to_string())) };
    }
    let words = tokenize(line)?;
    let (head, rest) = words.split_first().ok_or("type a command")?;
    let Some(cmd) = find_command(head) else {
        // A function name may hold spaces: try the whole line first.
        let by_name = |name: &str| functions.iter().find(|(_, n)| normalize(n) == normalize(name));
        return match by_name(line).or_else(|| by_name(head)) {
            Some((id, name)) if normalize(name) == normalize(line) || rest.is_empty() => Ok(CliCommand::Function(id.clone())),
            Some((_, name)) => Err(format!("{name} is a saved function and takes no arguments")),
            None => Err(format!("unknown command {head:?} (start with '!' to run a shell command)")),
        };
    };
    let args = parse_args(cmd, rest)?;
    let check = Function { id: String::new(), name: String::new(), steps: vec![FnStep::Internal { cmd, args: args.clone() }] };
    if let Some(problem) = validate(&check).first() {
        return Err(problem.message.clone());
    }
    Ok(CliCommand::Internal { cmd, args })
}

/// Match `key=value` words and positional words to the parameters of `cmd`.
/// A positional word fills the first parameter not named explicitly; a bare
/// checkbox name turns that checkbox on.
fn parse_args(cmd: InternalCmd, words: &[String]) -> Result<InternalArgs, String> {
    let params = schema::params_for(cmd);
    let mut args = InternalArgs::default();
    let mut positional: Vec<&String> = Vec::new();
    for word in words {
        match word.split_once('=') {
            Some((key, value)) if params.iter().any(|p| p.key == key) => {
                let spec = params.iter().find(|p| p.key == key).expect("checked above");
                let stored = if spec.kind == schema::ParamKind::Bool { bool_value(value)? } else { value.to_string() };
                if !stored.is_empty() {
                    args.0.insert(key.to_string(), stored);
                }
            }
            Some((key, _)) if key.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !key.is_empty() => {
                let known: Vec<&str> = params.iter().map(|p| p.key).collect();
                return Err(format!("{} has no parameter {key:?} (it has: {})", cmd.label(), if known.is_empty() { "none".to_string() } else { known.join(", ") }));
            }
            _ => match params.iter().find(|p| p.kind == schema::ParamKind::Bool && p.key == word.as_str()) {
                Some(flag) => {
                    args.0.insert(flag.key.to_string(), BOOL_TRUE.to_string());
                }
                None => positional.push(word),
            },
        }
    }
    let named: Vec<&str> = args.0.keys().map(String::as_str).collect();
    let mut free = params.iter().filter(|p| p.kind != schema::ParamKind::Bool && !named.contains(&p.key)).collect::<Vec<_>>().into_iter();
    let mut filled: Vec<(String, String)> = Vec::new();
    for word in positional {
        let spec = free.next().ok_or_else(|| format!("{}: too many arguments ({word:?})", cmd.label()))?;
        filled.push((spec.key.to_string(), word.clone()));
    }
    args.0.extend(filled);
    Ok(args)
}

/// `true`/`yes`/`on`/`1` and `false`/`no`/`off`/`0` for a checkbox value.
fn bool_value(text: &str) -> Result<String, String> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(BOOL_TRUE.to_string()),
        "false" | "no" | "off" | "0" | "" => Ok(String::new()),
        other => Err(format!("expected true or false, got {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn funcs() -> Vec<(FunctionId, String)> {
        vec![("id-1".into(), "Backup Home".into())]
    }

    fn internal(line: &str) -> (InternalCmd, Vec<(String, String)>) {
        match parse(line, &funcs()).unwrap() {
            CliCommand::Internal { cmd, args } => (cmd, args.0.into_iter().collect()),
            other => panic!("not internal: {other:?}"),
        }
    }

    #[test]
    fn tokenize_handles_quotes_escapes_and_errors() {
        assert_eq!(tokenize(r#"a "b c" 'd e' f\ g"#).unwrap(), vec!["a", "b c", "d e", "f g"]);
        assert_eq!(tokenize(r#"x "" y"#).unwrap(), vec!["x", "", "y"]);
        assert_eq!(tokenize("  ").unwrap(), Vec::<String>::new());
        assert!(tokenize("a 'b").is_err());
        assert!(tokenize("a\\").is_err());
    }

    #[test]
    fn commands_are_found_by_name_label_and_alias() {
        for word in ["MakeDir", "makedir", "Make Directory", "make_directory", "md", "mkdir"] {
            assert_eq!(find_command(word), Some(InternalCmd::MakeDir), "{word}");
        }
        assert_eq!(find_command("cd"), Some(InternalCmd::ChangeDir));
        assert_eq!(find_command("toggle-hide"), Some(InternalCmd::ToggleHide));
        assert_eq!(find_command("nope"), None);
    }

    #[test]
    fn positional_and_named_arguments_fill_the_schema() {
        assert_eq!(internal("MakeDir backup"), (InternalCmd::MakeDir, vec![("name".into(), "backup".into())]));
        assert_eq!(internal("MakeDir name=\"my dir\""), (InternalCmd::MakeDir, vec![("name".into(), "my dir".into())]));
        assert_eq!(internal("cd ~/Documents").1, vec![("path".to_string(), "~/Documents".to_string())]);
        // The positional word goes to the first parameter not named.
        let (_, args) = internal("CopyAs include_glob=*.rs *.bak");
        assert!(args.contains(&("new_name".to_string(), "*.bak".to_string())));
        assert!(args.contains(&("include_glob".to_string(), "*.rs".to_string())));
    }

    #[test]
    fn checkboxes_take_a_bare_name_or_a_value() {
        assert_eq!(internal("Copy newer").1, vec![("newer".to_string(), "true".to_string())]);
        assert_eq!(internal("Copy update=yes move_when_same=off").1, vec![("update".to_string(), "true".to_string())]);
        assert!(parse("Copy newer=maybe", &[]).is_err());
    }

    #[test]
    fn placeholders_pass_through_untouched() {
        let (_, args) = internal("Confirm \"Delete {o}?\"");
        assert_eq!(args, vec![("message".to_string(), "Delete {o}?".to_string())]);
    }

    #[test]
    fn missing_required_parameters_and_bad_arguments_are_reported() {
        assert!(parse("MakeDir", &[]).unwrap_err().contains("missing required"));
        assert!(parse("MakeDir a b", &[]).unwrap_err().contains("too many"));
        let e = parse("MakeDir size=3", &[]).unwrap_err();
        assert!(e.contains("no parameter") && e.contains("name"), "{e}");
        assert!(parse("Copy", &[]).is_ok(), "no required parameters");
        assert!(parse("Hide extra", &[]).is_err());
    }

    #[test]
    fn shell_lines_and_saved_functions() {
        assert_eq!(parse("!ls -l {f}", &[]).unwrap(), CliCommand::Shell("ls -l {f}".into()));
        assert!(parse("!  ", &[]).is_err());
        assert_eq!(parse("backup home", &funcs()).unwrap(), CliCommand::Function("id-1".into()));
        assert!(parse("Backup Home now", &funcs()).is_err());
        assert_eq!(parse("\"Backup Home\"", &funcs()).unwrap(), CliCommand::Function("id-1".into()));
        assert!(parse("frobnicate", &funcs()).unwrap_err().contains("unknown command"));
        assert!(parse("", &[]).is_err());
    }

    #[test]
    fn steps_for_shell_and_internal_commands() {
        let steps = parse("!echo hi", &[]).unwrap().steps().unwrap();
        assert_eq!(steps, vec![FnStep::External { argv: vec!["sh".into(), "-c".into(), "echo hi".into()], cwd_source: true }]);
        assert!(matches!(parse("Hide", &[]).unwrap().steps().unwrap()[0], FnStep::Internal { cmd: InternalCmd::Hide, .. }));
        assert!(parse("backup home", &funcs()).unwrap().steps().is_none());
    }

    #[test]
    fn all_lists_every_command_once() {
        let mut seen = std::collections::HashSet::new();
        assert!(InternalCmd::ALL.iter().all(|c| seen.insert(format!("{c:?}"))));
    }
}
