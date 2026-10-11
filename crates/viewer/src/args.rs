//! Command line of `opusdm-viewer`.

/// Usage text, shown on a bad command line.
pub const USAGE: &str = "usage: opusdm-viewer <file or folder>...";

/// Parse the arguments after the program name: one or more files and folders.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Vec<String>, String> {
    let mut targets = Vec::new();
    for arg in args {
        if arg.starts_with("--") { return Err(format!("unknown option {arg}\n{USAGE}")); }
        targets.push(arg);
    }
    if targets.is_empty() { return Err(USAGE.to_string()); }
    Ok(targets)
}

/// Fail early with a clear message when `target` is neither a regular file nor a folder.
pub fn check_target(target: &str) -> Result<(), String> {
    let path = std::path::Path::new(target);
    if !path.exists() { return Err(format!("{target}: no such file or folder")); }
    if !path.is_file() && !path.is_dir() { return Err(format!("{target}: not a regular file or a folder")); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Vec<String>, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn files_and_folders_in_order() {
        assert_eq!(p(&["a.png"]).unwrap(), ["a.png"]);
        assert_eq!(p(&["b.png", "/pics", "a.pdf"]).unwrap(), ["b.png", "/pics", "a.pdf"]);
    }

    #[test]
    fn errors_carry_the_usage() {
        assert_eq!(p(&[]).unwrap_err(), USAGE);
        assert!(p(&["--x", "a"]).unwrap_err().contains("unknown option --x"));
    }

    #[test]
    fn target_must_be_a_file_or_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        std::fs::write(&file, b"x").unwrap();
        assert!(check_target(file.to_str().unwrap()).is_ok());
        assert!(check_target(dir.path().to_str().unwrap()).is_ok());
        assert!(check_target(dir.path().join("nope").to_str().unwrap()).unwrap_err().contains("no such file"));
        assert!(check_target("/dev/null").unwrap_err().contains("not a regular file"));
    }
}
