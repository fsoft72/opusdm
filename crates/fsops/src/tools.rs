//! Finding the external archive tools in `PATH`.

use std::ffi::OsStr;
use std::path::PathBuf;

/// Names under which the 7-Zip command line is installed, in order of preference.
pub const SEVEN_Z_TOOLS: &[&str] = &["7z", "7zz", "7za"];

/// Whether an executable file called `name` exists in a `PATH` directory.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    find_in_path_var(&std::env::var_os("PATH")?, name)
}

/// Like [`find_in_path`], searching the directories listed in `path_var`.
pub fn find_in_path_var(path_var: &OsStr, name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// The first 7-Zip executable found in `PATH`, if any.
pub fn seven_z_tool() -> Option<&'static str> {
    seven_z_tool_in(&std::env::var_os("PATH").unwrap_or_default())
}

/// Like [`seven_z_tool`], searching the directories listed in `path_var`.
pub fn seven_z_tool_in(path_var: &OsStr) -> Option<&'static str> {
    SEVEN_Z_TOOLS.iter().copied().find(|t| find_in_path_var(path_var, t).is_some())
}

/// The 7-Zip program to run: the one found in `PATH`, else plain `7z` so the
/// spawn error names the usual program.
pub fn seven_z_program() -> &'static str {
    seven_z_tool().unwrap_or(SEVEN_Z_TOOLS[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn make_exe(dir: &std::path::Path, name: &str, mode: u32) {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_system_with_only_7zz_finds_7zz() {
        let td = tempfile::tempdir().unwrap();
        make_exe(td.path(), "7zz", 0o755);
        assert_eq!(seven_z_tool_in(td.path().as_os_str()), Some("7zz"));
    }

    #[test]
    fn the_preferred_name_wins_and_non_executables_are_skipped() {
        let td = tempfile::tempdir().unwrap();
        make_exe(td.path(), "7za", 0o755);
        make_exe(td.path(), "7z", 0o644);
        assert_eq!(seven_z_tool_in(td.path().as_os_str()), Some("7za"), "7z is not executable");
        make_exe(td.path(), "7z", 0o755);
        assert_eq!(seven_z_tool_in(td.path().as_os_str()), Some("7z"));
    }

    #[test]
    fn nothing_found_is_none() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(seven_z_tool_in(td.path().as_os_str()), None);
    }
}
