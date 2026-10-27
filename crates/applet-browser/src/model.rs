//! The texts shown by the Applet Browser, built from the `AppletStatus`
//! entries of the applets socket. Pure: no GTK, no socket.

use opusdm_applets::protocol::AppletStatus;
use std::path::Path;

/// Shown instead of the list when the applets socket does not answer.
pub const HOST_UNREACHABLE_TEXT: &str = "The applets process is not running";

/// The name shown for `status`: its manifest name, or the folder id when the
/// name is missing or blank.
pub fn display_name(status: &AppletStatus) -> &str {
    match status.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name,
        _ => &status.id,
    }
}

/// The title of the row of `status`: its name and, when known, its version.
pub fn title(status: &AppletStatus) -> String {
    let name = display_name(status);
    if status.version.trim().is_empty() { return name.to_string(); }
    format!("{name} {}", status.version.trim())
}

/// One-line summary of `status`: name, version, description and, for a
/// broken applet, its error.
pub fn describe(status: &AppletStatus) -> String {
    let mut out = title(status);
    let description = status.description.trim();
    if !description.is_empty() {
        out.push_str(" - ");
        out.push_str(description);
    }
    if let Some(error) = &status.error {
        out.push_str(&format!(" (broken: {error})"));
    }
    out
}

/// The message shown instead of the list when the socket does not answer.
pub fn host_unreachable_text() -> &'static str {
    HOST_UNREACHABLE_TEXT
}

/// The hint shown for an empty applets folder `dir`; a `dir` under `home`
/// is written with `~`.
pub fn empty_text(dir: &Path, home: Option<&Path>) -> String {
    let short = home
        .and_then(|h| dir.strip_prefix(h).ok())
        .map(|rest| format!("~/{}", rest.display()));
    format!("No applets in {}", short.unwrap_or_else(|| dir.display().to_string()))
}

/// What a manifest permission lets the applet do, in words for a person; an
/// unknown name is shown as is.
pub fn permission_label(name: &str) -> &str {
    match name {
        "net" => "Network access",
        "fs" => "Read and write files",
        "exec" => "Run commands",
        other => other,
    }
}

/// The permissions an applet asks for, comma separated, or "none".
pub fn permissions_text(permissions: &[String]) -> String {
    if permissions.is_empty() { return "none".to_string(); }
    permissions.iter().map(|p| permission_label(p)).collect::<Vec<_>>().join(", ")
}

/// The question of the import confirmation: the applet, by its manifest name
/// and version (the folder id when the name is blank).
pub fn import_question(id: &str, name: &str, version: &str) -> String {
    let name = if name.trim().is_empty() { id } else { name.trim() };
    match version.trim() {
        "" => format!("Import {name}?"),
        v => format!("Import {name} {v}?"),
    }
}

/// The detail of the import confirmation: the permissions asked and, when an
/// installed applet with the same id would be replaced, a warning.
pub fn import_detail(permissions: &[String], replaces: bool) -> String {
    let mut out = format!("Permissions requested: {}.", permissions_text(permissions));
    if permissions.iter().any(|p| p == "exec") {
        out.push_str("\nThe applet can run commands on this computer.");
    }
    if replaces { out.push_str("\nAn installed applet with the same folder name will be replaced."); }
    out.push_str("\nThe applet is added disabled.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A status for `id` with the given name and error.
    fn status(id: &str, name: Option<&str>, error: Option<&str>) -> AppletStatus {
        AppletStatus {
            id: id.into(),
            name: name.map(str::to_string),
            description: "Shows the time".into(),
            version: "1.2".into(),
            enabled: true,
            running: error.is_none(),
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn describe_marks_a_broken_applet() {
        let ok = describe(&status("clock", Some("Clock"), None));
        assert_eq!(ok, "Clock 1.2 - Shows the time");
        let broken = describe(&status("clock", Some("Clock"), Some("main.js: syntax error")));
        assert_eq!(broken, "Clock 1.2 - Shows the time (broken: main.js: syntax error)");
    }

    #[test]
    fn describe_uses_the_folder_id_when_there_is_no_name() {
        assert_eq!(describe(&status("my-clock", None, None)), "my-clock 1.2 - Shows the time");
        assert_eq!(display_name(&status("my-clock", Some("  "), None)), "my-clock");
        assert_eq!(display_name(&status("my-clock", Some("Clock"), None)), "Clock");
    }

    #[test]
    fn describe_skips_an_empty_version_and_description() {
        let mut s = status("a", Some("A"), None);
        s.version = String::new();
        s.description = String::new();
        assert_eq!(describe(&s), "A");
    }

    #[test]
    fn host_unreachable_text_says_the_process_is_not_running() {
        assert_eq!(host_unreachable_text(), "The applets process is not running");
    }

    #[test]
    fn empty_text_shortens_the_home_folder() {
        let dir = Path::new("/home/u/.config/opusdm/applets");
        assert_eq!(empty_text(dir, Some(Path::new("/home/u"))), "No applets in ~/.config/opusdm/applets");
        assert_eq!(empty_text(dir, None), "No applets in /home/u/.config/opusdm/applets");
        assert_eq!(empty_text(dir, Some(Path::new("/other"))), "No applets in /home/u/.config/opusdm/applets");
    }

    #[test]
    fn permissions_are_written_in_words() {
        assert_eq!(permissions_text(&[]), "none");
        assert_eq!(permissions_text(&["net".into(), "fs".into(), "exec".into()]), "Network access, Read and write files, Run commands");
        assert_eq!(permission_label("other"), "other");
    }

    #[test]
    fn the_import_question_names_the_applet() {
        assert_eq!(import_question("clock", "Clock", "1.2"), "Import Clock 1.2?");
        assert_eq!(import_question("clock", "Clock", " "), "Import Clock?");
        assert_eq!(import_question("clock", "  ", ""), "Import clock?");
    }

    #[test]
    fn the_import_detail_lists_permissions_and_warnings() {
        let none = import_detail(&[], false);
        assert_eq!(none, "Permissions requested: none.\nThe applet is added disabled.");
        let exec = import_detail(&["net".into(), "exec".into()], true);
        assert!(exec.starts_with("Permissions requested: Network access, Run commands."), "{exec}");
        assert!(exec.contains("can run commands on this computer"));
        assert!(exec.contains("will be replaced"));
        assert!(!import_detail(&["net".into()], false).contains("replaced"));
    }
}
