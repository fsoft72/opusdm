//! Command line of `opusdm-viewer`.

/// Usage text, shown on a bad command line.
pub const USAGE: &str = "usage: opusdm-viewer <picture or video file>";

/// Parse the arguments after the program name: exactly one file.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<String, String> {
    let mut target = None;
    for arg in args {
        if arg.starts_with("--") { return Err(format!("unknown option {arg}\n{USAGE}")); }
        if target.replace(arg).is_some() { return Err(format!("only one file can be opened\n{USAGE}")); }
    }
    target.ok_or_else(|| USAGE.to_string())
}

/// Fail early with a clear message when `target` is not a regular file.
pub fn check_target(target: &str) -> Result<(), String> {
    let path = std::path::Path::new(target);
    if !path.exists() { return Err(format!("{target}: no such file")); }
    if !path.is_file() { return Err(format!("{target}: not a regular file")); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<String, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn a_file_alone() {
        assert_eq!(p(&["a.png"]).unwrap(), "a.png");
    }

    #[test]
    fn errors_carry_the_usage() {
        assert_eq!(p(&[]).unwrap_err(), USAGE);
        assert!(p(&["a", "b"]).unwrap_err().contains("only one file"));
        assert!(p(&["--x", "a"]).unwrap_err().contains("unknown option --x"));
    }

    #[test]
    fn target_must_be_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        std::fs::write(&file, b"x").unwrap();
        assert!(check_target(file.to_str().unwrap()).is_ok());
        assert!(check_target(dir.path().to_str().unwrap()).unwrap_err().contains("not a regular file"));
        assert!(check_target(dir.path().join("nope").to_str().unwrap()).unwrap_err().contains("no such file"));
    }
}
