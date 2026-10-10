//! Shell quoting of the paths dropped on a terminal, so a dropped file lands
//! on the command line as a single, safe word.

use std::path::Path;

/// Characters that never need quoting in a POSIX shell word.
fn is_safe(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-./,:@%+=".contains(c)
}

/// Quote `text` for a POSIX shell: left alone when every character is safe,
/// otherwise wrapped in single quotes with embedded quotes escaped.
pub fn shell_quote(text: &str) -> String {
    if !text.is_empty() && text.chars().all(is_safe) { return text.to_string(); }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The text typed into the terminal for dropped `paths`: each one quoted,
/// separated by spaces, with a trailing space ready for the next word.
pub fn dropped_paths_text(paths: &[impl AsRef<Path>]) -> String {
    paths
        .iter()
        .map(|p| shell_quote(&p.as_ref().to_string_lossy()) + " ")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_are_left_alone() {
        assert_eq!(shell_quote("/home/user/file-1.txt"), "/home/user/file-1.txt");
    }

    #[test]
    fn spaces_and_quotes_are_quoted() {
        assert_eq!(shell_quote("/tmp/my file"), "'/tmp/my file'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn dropped_paths_are_joined_with_a_trailing_space() {
        let text = dropped_paths_text(&["/a", "/b c"]);
        assert_eq!(text, "/a '/b c' ");
    }
}
