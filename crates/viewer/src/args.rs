//! Command line of `opusdm-viewer`.

use opusdm_ipc::sidecar::SIDECAR_FLAG;

/// Usage text, shown on a bad command line.
pub const USAGE: &str = "usage: opusdm-viewer <file or folder>...\n       opusdm-viewer --sidecar [file or folder]...  (follow the paths written on stdin)";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    /// Follow the updates a Lister writes on stdin.
    pub sidecar: bool,
    /// Files and folders to show first.
    pub targets: Vec<String>,
}

/// Parse the arguments after the program name: one or more files and folders,
/// or `--sidecar` with any number of them.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut parsed = Args { sidecar: false, targets: Vec::new() };
    for arg in args {
        if arg == SIDECAR_FLAG { parsed.sidecar = true; continue; }
        if arg.starts_with("--") { return Err(format!("unknown option {arg}\n{USAGE}")); }
        parsed.targets.push(arg);
    }
    if parsed.targets.is_empty() && !parsed.sidecar { return Err(USAGE.to_string()); }
    Ok(parsed)
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

    fn p(a: &[&str]) -> Result<Args, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn files_and_folders_in_order() {
        assert_eq!(p(&["a.png"]).unwrap().targets, ["a.png"]);
        assert_eq!(p(&["b.png", "/pics", "a.pdf"]).unwrap().targets, ["b.png", "/pics", "a.pdf"]);
        assert!(!p(&["a.png"]).unwrap().sidecar);
    }

    #[test]
    fn errors_carry_the_usage() {
        assert_eq!(p(&[]).unwrap_err(), USAGE);
        assert!(p(&["--x", "a"]).unwrap_err().contains("unknown option --x"));
    }

    #[test]
    fn a_sidecar_needs_no_target() {
        assert_eq!(p(&["--sidecar"]).unwrap(), Args { sidecar: true, targets: Vec::new() });
        assert_eq!(p(&["a.png", "--sidecar"]).unwrap(), Args { sidecar: true, targets: vec!["a.png".to_string()] });
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
