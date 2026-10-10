//! Wayfire's configuration file, where OpusDM keeps the settings that a
//! Wayland client cannot set by itself (input devices, keyboard layouts).
//! OpusDM only ever writes its own marked blocks (`#` comments around the
//! block, at the end of their INI section); the rest of the file is never
//! touched. Wayfire watches the file and reloads it by itself after a change,
//! and when a key appears twice in a section the last one wins, so the
//! OpusDM block overrides the user's own lines above it.

use std::path::PathBuf;

use crate::labwc::{env_markers, replace_marked};

/// Environment variable Wayfire sets for its children: the configuration
/// file it runs with (`wayfire -c` or the default one).
pub const ENV_CONFIG_FILE: &str = "WAYFIRE_CONFIG_FILE";

/// The `wayfire.ini` Wayfire runs with: `$WAYFIRE_CONFIG_FILE`, else
/// `$XDG_CONFIG_HOME/wayfire.ini`.
pub fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os(ENV_CONFIG_FILE).filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    config.join("wayfire.ini")
}

/// Whether `line` is the header of section `section` (`[input]`).
fn is_header(line: &str, section: &str) -> bool {
    line.trim() == format!("[{section}]")
}

/// Byte offset where a new block goes inside the section whose header
/// starts at `header`: after its last non blank line, before the next
/// section header.
fn section_end(ini: &str, header: usize) -> usize {
    let body = ini[header..].find('\n').map_or(ini.len(), |i| header + i + 1);
    let mut end = body;
    let mut at = body;
    for line in ini[body..].split_inclusive('\n') {
        if line.trim_start().starts_with('[') { break; }
        at += line.len();
        if !line.trim().is_empty() { end = at; }
    }
    end
}

/// `ini` (the current `wayfire.ini`, `None` when there is none) with the
/// OpusDM block `name` holding `lines` (`key = value` each; empty removes the
/// block). An existing block is replaced in place; a new one goes at the end
/// of `[section]`, which is added at the end of the file when missing.
pub fn with_block(ini: Option<&str>, section: &str, name: &str, lines: &[String]) -> String {
    let (begin, end) = env_markers(name);
    let block = if lines.is_empty() { String::new() } else { format!("{begin}\n{}\n{end}\n", lines.join("\n")) };
    let ini = ini.unwrap_or("");
    if let Some(replaced) = replace_marked(ini, &begin, &end, &block) { return replaced; }
    if block.is_empty() { return ini.to_string(); }
    let mut offset = 0;
    for line in ini.split_inclusive('\n') {
        if is_header(line, section) {
            let at = section_end(ini, offset);
            let sep = if ini[..at].ends_with('\n') { "" } else { "\n" };
            return format!("{}{sep}{block}{}", &ini[..at], &ini[at..]);
        }
        offset += line.len();
    }
    let sep = match ini {
        "" => "",
        _ if ini.ends_with("\n\n") => "",
        _ if ini.ends_with('\n') => "\n",
        _ => "\n\n",
    };
    format!("{ini}{sep}[{section}]\n{block}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(lines: &str) -> String {
        let (b, e) = env_markers("test");
        format!("{b}\n{lines}\n{e}\n")
    }

    #[test]
    fn a_block_goes_at_the_end_of_its_section() {
        let ini = "[core]\nvwidth = 4\n\n[input]\nxkb_layout = us\n\n[place]\nmode = cascade\n";
        let out = with_block(Some(ini), "input", "test", &["kb_repeat_rate = 40".to_string()]);
        assert_eq!(out, format!("[core]\nvwidth = 4\n\n[input]\nxkb_layout = us\n{}\n[place]\nmode = cascade\n", block("kb_repeat_rate = 40")));
        let last = with_block(Some("[input]\nxkb_layout = us"), "input", "test", &["a = 1".to_string()]);
        assert_eq!(last, format!("[input]\nxkb_layout = us\n{}", block("a = 1")));
    }

    #[test]
    fn a_missing_section_or_file_is_added() {
        let out = with_block(Some("[core]\nvwidth = 4\n"), "input", "test", &["a = 1".to_string()]);
        assert_eq!(out, format!("[core]\nvwidth = 4\n\n[input]\n{}", block("a = 1")));
        assert_eq!(with_block(None, "input", "test", &["a = 1".to_string()]), format!("[input]\n{}", block("a = 1")));
        assert_eq!(with_block(None, "input", "test", &[]), "");
    }

    #[test]
    fn an_existing_block_is_replaced_or_removed() {
        let ini = format!("[input]\nxkb_layout = us\n{}\n[place]\n", block("a = 1"));
        let out = with_block(Some(&ini), "input", "test", &["a = 2".to_string()]);
        assert_eq!(out, format!("[input]\nxkb_layout = us\n{}\n[place]\n", block("a = 2")));
        assert_eq!(with_block(Some(&ini), "input", "test", &[]), "[input]\nxkb_layout = us\n\n[place]\n");
        assert_eq!(with_block(Some(&ini), "input", "other", &[]), ini, "another block is left alone");
    }
}
