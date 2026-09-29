//! File-name patterns shared by the hub, `opusdm-fsops` and the Lister:
//! a `*`/`?` matcher, a multi-pattern set for job filters, and the `*`
//! expansion used by the "Copy As..." naming dialog.

/// Separators accepted between the alternatives of a filter string.
const PATTERN_SEPARATORS: [char; 2] = [',', ';'];

/// Case-insensitive glob match supporting `*` (any run) and `?` (one char).
///
/// Two-pointer greedy match with backtracking to the most recent `*`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_match = 0usize;
    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            star_match = ti;
            pi += 1;
        } else if let Some(si) = star {
            pi = si + 1;
            star_match += 1;
            ti = star_match;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// One or more glob patterns; a name matches when any of them does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobSet {
    patterns: Vec<String>,
}

impl GlobSet {
    /// Parse a filter string such as `*.rs;*.toml` (`,` also separates).
    /// Fails on a string with no pattern at all, or a pattern with a `/`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let patterns: Vec<String> = text
            .split(PATTERN_SEPARATORS)
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect();
        if patterns.is_empty() {
            return Err(format!("filter pattern is empty: {text:?}"));
        }
        if let Some(bad) = patterns.iter().find(|p| p.contains('/')) {
            return Err(format!("filter pattern must match file names, not paths: {bad:?}"));
        }
        Ok(Self { patterns })
    }

    /// Whether `name` matches at least one pattern.
    pub fn is_match(&self, name: &str) -> bool {
        self.patterns.iter().any(|p| glob_match(p, name))
    }
}

/// Expand a "Copy As..." naming pattern: every `*` becomes `original`.
/// A pattern without `*` is used verbatim.
pub fn expand_new_name(pattern: &str, original: &str) -> String {
    pattern.replace('*', original)
}

/// Check a final target name: non-empty, no `/`, not `.` or `..`.
pub fn validate_new_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("new name is empty".into());
    }
    if name.contains('/') {
        return Err(format!("new name must not contain '/': {name:?}"));
    }
    if name == "." || name == ".." {
        return Err(format!("invalid new name: {name:?}"));
    }
    Ok(())
}

/// Expand `pattern` for every original name and check the result: every
/// name valid, and no two sources landing on the same name.
pub fn expand_all(pattern: &str, originals: &[String]) -> Result<Vec<String>, String> {
    let names: Vec<String> = originals.iter().map(|o| expand_new_name(pattern, o)).collect();
    for name in &names {
        validate_new_name(name)?;
    }
    let mut seen = std::collections::HashSet::new();
    match names.iter().find(|n| !seen.insert(n.as_str())) {
        Some(dup) => Err(format!("several sources would be named {dup:?}; use * in the pattern")),
        None => Ok(names),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_star_and_question_case_insensitively() {
        assert!(glob_match("*.rs", "Main.RS"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("*.rs", "main.rs.bak"));
    }

    #[test]
    fn set_accepts_alternatives() {
        let set = GlobSet::parse("*.rs; *.toml").unwrap();
        assert!(set.is_match("lib.rs"));
        assert!(set.is_match("Cargo.toml"));
        assert!(!set.is_match("notes.txt"));
    }

    #[test]
    fn set_rejects_empty_and_path_patterns() {
        assert!(GlobSet::parse("").is_err());
        assert!(GlobSet::parse(" ; , ").is_err());
        assert!(GlobSet::parse("src/*.rs").is_err());
    }

    #[test]
    fn expands_star_to_the_original_name() {
        assert_eq!(expand_new_name("*.bak", "a.txt"), "a.txt.bak");
        assert_eq!(expand_new_name("fixed", "a.txt"), "fixed");
    }

    #[test]
    fn validates_new_names() {
        assert!(validate_new_name("ok.txt").is_ok());
        assert!(validate_new_name("").is_err());
        assert!(validate_new_name("a/b").is_err());
        assert!(validate_new_name("..").is_err());
    }

    #[test]
    fn expand_all_checks_every_name_and_uniqueness() {
        let orig = vec!["a".to_string(), "b".to_string()];
        assert_eq!(expand_all("*.bak", &orig).unwrap(), vec!["a.bak", "b.bak"]);
        assert!(expand_all("same", &orig).is_err());
        assert!(expand_all("x/*", &orig).is_err());
        assert!(expand_all("", &orig).is_err());
        assert_eq!(expand_all("only", &orig[..1]).unwrap(), vec!["only"]);
    }
}
