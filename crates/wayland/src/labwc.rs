//! labwc's configuration, where OpusDM keeps the settings that a Wayland
//! client cannot set by itself (global hotkeys, input devices, keyboard
//! layouts). OpusDM only ever writes its own marked blocks: in `rc.xml`
//! (XML comments around the block) and in `environment` (shell comments);
//! the rest of the user's files is never touched. After a change labwc is
//! told to reload (`SIGHUP`, what `labwc --reconfigure` does), which rereads
//! both files.

use std::path::{Path, PathBuf};

/// Environment variable naming the `rc.xml` to write instead of labwc's
/// default one (the sandbox of `run-wayland.sh` uses its own). The
/// `environment` file is taken from the same folder.
pub const ENV_LABWC_RC: &str = "OPUSDM_LABWC_RC";

/// Where in `rc.xml` a block goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// At the end of `<keyboard>` (keybinds, key repeat).
    Keyboard,
    /// At the end of `<labwc_config>` (a whole element such as `<libinput>`).
    Root,
}

/// Whether this process runs under labwc (it sets `LABWC_PID` for its
/// children).
pub fn under_labwc() -> bool {
    std::env::var_os("LABWC_PID").is_some()
}

/// labwc's `rc.xml`: `$OPUSDM_LABWC_RC`, else `$XDG_CONFIG_HOME/labwc/rc.xml`.
pub fn rc_path() -> PathBuf {
    if let Some(path) = std::env::var_os(ENV_LABWC_RC).filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    config.join("labwc").join("rc.xml")
}

/// labwc's `environment` file, next to [`rc_path`].
pub fn environment_path() -> PathBuf {
    rc_path().with_file_name("environment")
}

/// The XML comments that open and close the OpusDM block `name`.
pub fn xml_markers(name: &str) -> (String, String) {
    (format!("<!-- OpusDM {name}: begin (written by OpusDM, do not edit) -->"), format!("<!-- OpusDM {name}: end -->"))
}

/// The shell comments that open and close the OpusDM block `name` (also
/// the markers of a block in Wayfire's INI file).
pub(crate) fn env_markers(name: &str) -> (String, String) {
    (format!("# OpusDM {name}: begin (written by OpusDM, do not edit)"), format!("# OpusDM {name}: end"))
}

/// `text` with the whole lines from the one holding `begin` to the one
/// holding `end` replaced by `replacement`. `None` when the markers are not
/// both there, in that order.
pub(crate) fn replace_marked(text: &str, begin: &str, end: &str, replacement: &str) -> Option<String> {
    let (b, e) = (text.find(begin)?, text.find(end)?);
    if b > e { return None; }
    let start = text[..b].rfind('\n').map_or(0, |i| i + 1);
    let stop = text[e..].find('\n').map_or(text.len(), |i| e + i + 1);
    Some(format!("{}{replacement}{}", &text[..start], &text[stop..]))
}

/// `rc` (the current `rc.xml`, `None` when there is none) with the OpusDM
/// block `name` holding `content` (lines indented for their place; empty
/// removes the block). An existing block is replaced in place; a new one
/// goes at the end of its `section`, and a missing `<keyboard>` (with
/// labwc's default keybinds) or file is created. Fails on a file with no
/// `labwc_config` element.
pub fn with_block(rc: Option<&str>, section: Section, name: &str, content: &str) -> Result<String, String> {
    let (begin, end) = xml_markers(name);
    let indent = if section == Section::Keyboard { "    " } else { "  " };
    let block = if content.is_empty() { String::new() } else { format!("{indent}{begin}\n{content}{indent}{end}\n") };
    let Some(rc) = rc else {
        if block.is_empty() { return Ok(String::new()); }
        return Ok(match section {
            Section::Keyboard => format!("<?xml version=\"1.0\"?>\n<labwc_config>\n  <keyboard>\n    <default />\n{block}  </keyboard>\n</labwc_config>\n"),
            Section::Root => format!("<?xml version=\"1.0\"?>\n<labwc_config>\n{block}</labwc_config>\n"),
        });
    };
    if let Some(replaced) = replace_marked(rc, &begin, &end, &block) { return Ok(replaced); }
    if block.is_empty() { return Ok(rc.to_string()); }
    let Some(close_root) = rc.rfind("</labwc_config>") else { return Err("not a labwc configuration (no labwc_config element)".to_string()) };
    let line_start = |at: usize| rc[..at].rfind('\n').map_or(at, |i| i + 1);
    match section {
        Section::Keyboard => match rc.find("</keyboard>") {
            Some(close) => {
                let at = line_start(close);
                Ok(format!("{}{block}{}", &rc[..at], &rc[at..]))
            }
            None => Ok(format!("{}  <keyboard>\n    <default />\n{block}  </keyboard>\n{}", &rc[..close_root], &rc[close_root..])),
        },
        Section::Root => {
            let at = line_start(close_root);
            Ok(format!("{}{block}{}", &rc[..at], &rc[at..]))
        }
    }
}

/// `env` (the current `environment` file, `None` when there is none) with
/// the OpusDM block `name` holding `lines` (`KEY=value` each; empty removes
/// the block), replaced in place or added at the end.
pub fn with_env_block(env: Option<&str>, name: &str, lines: &[String]) -> String {
    let (begin, end) = env_markers(name);
    let block = if lines.is_empty() { String::new() } else { format!("{begin}\n{}\n{end}\n", lines.join("\n")) };
    let env = env.unwrap_or("");
    if let Some(replaced) = replace_marked(env, &begin, &end, &block) { return replaced; }
    if block.is_empty() { return env.to_string(); }
    if env.is_empty() || env.ends_with('\n') { format!("{env}{block}") } else { format!("{env}\n{block}") }
}

/// The content of `path`, `None` when it does not exist.
pub fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Write `text` to `path` (atomically, creating the folder) when it differs
/// from `old`, the content read before. `true` when the file was written.
pub fn write_if_changed(path: &Path, old: Option<&str>, text: &str) -> Result<bool, String> {
    if old.unwrap_or("") == text { return Ok(false); }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("opusdm-tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))?;
    Ok(true)
}

/// Tell the labwc this process runs under to reload its configuration.
pub fn reload() -> Result<(), String> {
    let Some(pid) = std::env::var("LABWC_PID").ok().and_then(|p| p.parse::<libc::pid_t>().ok()) else {
        return Err("not running under labwc (no LABWC_PID)".to_string());
    };
    // SAFETY: `kill` takes plain integers and has no memory-safety contract.
    if unsafe { libc::kill(pid, libc::SIGHUP) } != 0 {
        return Err(format!("cannot ask labwc ({pid}) to reload: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// `text` escaped for an XML attribute value or element text.
pub fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyboard_block(content: &str) -> String {
        let (b, e) = xml_markers("test");
        format!("    {b}\n{content}    {e}\n")
    }

    #[test]
    fn a_block_goes_at_the_end_of_its_section() {
        let rc = "<labwc_config>\n  <keyboard>\n    <default />\n  </keyboard>\n</labwc_config>\n";
        let out = with_block(Some(rc), Section::Keyboard, "test", "    X\n").unwrap();
        assert_eq!(out, format!("<labwc_config>\n  <keyboard>\n    <default />\n{}  </keyboard>\n</labwc_config>\n", keyboard_block("    X\n")));
        let (b, e) = xml_markers("input");
        let out = with_block(Some(rc), Section::Root, "input", "  <libinput />\n").unwrap();
        assert!(out.ends_with(&format!("  </keyboard>\n  {b}\n  <libinput />\n  {e}\n</labwc_config>\n")));
    }

    #[test]
    fn an_existing_block_is_replaced_or_removed() {
        let rc = format!("<labwc_config>\n  <keyboard>\n{}    <keybind key=\"W-x\" />\n  </keyboard>\n</labwc_config>\n", keyboard_block("    old\n"));
        let out = with_block(Some(&rc), Section::Keyboard, "test", "    new\n").unwrap();
        assert_eq!(out, format!("<labwc_config>\n  <keyboard>\n{}    <keybind key=\"W-x\" />\n  </keyboard>\n</labwc_config>\n", keyboard_block("    new\n")));
        let gone = with_block(Some(&rc), Section::Keyboard, "test", "").unwrap();
        assert_eq!(gone, "<labwc_config>\n  <keyboard>\n    <keybind key=\"W-x\" />\n  </keyboard>\n</labwc_config>\n");
        let other = with_block(Some(&rc), Section::Keyboard, "other", "").unwrap();
        assert_eq!(other, rc, "another block is left alone");
    }

    #[test]
    fn missing_sections_and_files_are_created() {
        let out = with_block(Some("<labwc_config>\n</labwc_config>\n"), Section::Keyboard, "test", "    B\n").unwrap();
        assert_eq!(out, format!("<labwc_config>\n  <keyboard>\n    <default />\n{}  </keyboard>\n</labwc_config>\n", keyboard_block("    B\n")));
        assert!(with_block(None, Section::Keyboard, "test", "    B\n").unwrap().contains("<default />"));
        assert!(with_block(None, Section::Root, "test", "  B\n").unwrap().starts_with("<?xml"));
        assert_eq!(with_block(None, Section::Root, "test", "").unwrap(), "");
        assert!(with_block(Some("<openbox_config/>"), Section::Root, "test", "  B\n").is_err());
    }

    #[test]
    fn environment_blocks_are_kept_apart_from_the_users_lines() {
        let lines = vec!["XKB_DEFAULT_LAYOUT=it".to_string()];
        let out = with_env_block(Some("FOO=1"), "keyboard", &lines);
        assert_eq!(out, "FOO=1\n# OpusDM keyboard: begin (written by OpusDM, do not edit)\nXKB_DEFAULT_LAYOUT=it\n# OpusDM keyboard: end\n");
        let changed = with_env_block(Some(&out), "keyboard", &["XKB_DEFAULT_LAYOUT=us".to_string()]);
        assert!(changed.contains("LAYOUT=us") && !changed.contains("LAYOUT=it") && changed.starts_with("FOO=1\n"));
        assert_eq!(with_env_block(Some(&out), "keyboard", &[]), "FOO=1\n");
        assert_eq!(with_env_block(None, "keyboard", &[]), "");
    }

    #[test]
    fn files_are_written_only_when_they_change() {
        let dir = std::env::temp_dir().join(format!("opusdm-labwc-test-{}", std::process::id()));
        let path = dir.join("rc.xml");
        assert!(write_if_changed(&path, None, "a").unwrap());
        assert!(!write_if_changed(&path, Some("a"), "a").unwrap());
        assert_eq!(read(&path).unwrap().as_deref(), Some("a"));
        assert_eq!(read(&dir.join("none")).unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
