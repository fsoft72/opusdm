//! Load the current settings for the config window, and apply edits: send
//! the whole `Settings` to a running hub over the socket atomically, or
//! write settings.json directly when no hub is up.

use opusdm_ipc::{DesktopSettings, IconSetId, Settings};

/// Load the full current `Settings` (defaults if missing/unparseable).
pub fn load_settings() -> Settings {
    opusdm_ipc::settings::load_settings("opusdm-settings")
}

/// Testable core of [`load_settings`].
#[cfg(test)]
pub fn load_settings_from(path: &std::path::Path) -> Settings {
    opusdm_ipc::settings::load_settings_from(path, "opusdm-settings")
}

/// The settings that applying theme `icon_set` produces: `prefs` (the user's
/// preferences, usually the window's widget state) with the look resolved
/// from the theme's `theme.json` (see [`crate::apply::resolve_look`]).
pub fn resolve_selected(icon_set: &IconSetId, prefs: &Settings) -> Settings {
    let IconSetId::Custom(slug) = icon_set;
    crate::apply::resolve_look(slug, &opusdm_ipc::themes::load_theme_data(slug), prefs)
}

/// What [`apply_full`] did.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Sent to a running hub; it will persist and repaint.
    SentToHub,
    /// No hub running - wrote settings.json; effective on next hub start.
    WroteFile,
    /// Could not send and could not write.
    Failed(String),
}

/// Send `settings` and `desktop` to a running hub atomically (one
/// `SetSettings` plus one `SetDesktopSettings` frame), or persist both
/// directly to disk if no hub is running.
pub fn apply_full(settings: &Settings, desktop: &DesktopSettings) -> ApplyOutcome {
    let sock = opusdm_ipc::sockpath::hub_sock_path();
    if opusdm_ipc::sockpath::probe_live_hub(&sock) {
        match send_to_hub(&sock, settings, desktop) {
            Ok(()) => return ApplyOutcome::SentToHub,
            Err(e) => opusdm_ipc::err_log!("opusdm-settings", "hub send failed, writing files: {e}"),
        }
    }
    match write_files(settings, desktop) {
        Ok(()) => ApplyOutcome::WroteFile,
        Err(e) => ApplyOutcome::Failed(e.to_string()),
    }
}

/// Connect to the hub socket and send one `SetSettings` frame followed by
/// one `SetDesktopSettings` frame.
fn send_to_hub(sock: &std::path::Path, settings: &Settings, desktop: &DesktopSettings) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(sock)?;
    opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::SetSettings(settings.clone()))?;
    opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::SetDesktopSettings(desktop.clone()))
}

/// Overwrite settings.json and desktop.json with the given, already-complete
/// values.
fn write_files(settings: &Settings, desktop: &DesktopSettings) -> std::io::Result<()> {
    opusdm_ipc::settings::save_settings(settings)?;
    opusdm_ipc::settings::save_desktop_settings(desktop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    #[test]
    fn apply_full_writes_both_files_when_no_hub_is_running() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        std::env::set_var("OPUSDM_HUB_SOCK", td.path().join("definitely-absent.sock"));

        let mut settings = Settings::default();
        settings.theme.icon_set = opusdm_ipc::IconSetId::Custom("breeze-dark".to_string());
        settings.appearance.monospace_list = true;
        let desktop = DesktopSettings {
            icon_size: 96,
            ..DesktopSettings::default()
        };

        match apply_full(&settings, &desktop) {
            ApplyOutcome::WroteFile => {}
            other => panic!("expected WroteFile, got {other:?}"),
        }

        let written_settings =
            std::fs::read_to_string(td.path().join("opusdm").join("settings.json")).unwrap();
        let back: Settings = serde_json::from_str(&written_settings).unwrap();
        assert_eq!(back.theme.icon_set, opusdm_ipc::IconSetId::Custom("breeze-dark".to_string()));
        assert!(back.appearance.monospace_list);

        let written_desktop =
            std::fs::read_to_string(td.path().join("opusdm").join("desktop.json")).unwrap();
        let back_desktop: DesktopSettings = serde_json::from_str(&written_desktop).unwrap();
        assert_eq!(back_desktop.icon_size, 96);
    }

    #[test]
    fn resolve_selected_reads_the_installed_theme_json() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let dir = td.path().join("opusdm").join("themes").join("t");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"name":"t","label":"T"}"#).unwrap();
        std::fs::write(dir.join("theme.json"), r##"{"colors":{"file_fg":"#abcdef"},"desktop":{"icon_size":100}}"##).unwrap();

        let mut prefs = Settings::default();
        prefs.appearance.monospace_list = true;
        prefs.behavior.recursive_filter = true;

        let s = resolve_selected(&opusdm_ipc::IconSetId::Custom("t".to_string()), &prefs);
        assert_eq!(s.theme.icon_set, opusdm_ipc::IconSetId::Custom("t".to_string()));
        assert_eq!(s.theme.colors.file_fg, "#abcdef");
        assert!(!s.appearance.monospace_list, "the theme sets no appearance: default");
        assert!(s.behavior.recursive_filter, "behaviour is a preference");
    }

    #[test]
    fn load_settings_from_missing_file_is_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        assert_eq!(load_settings_from(&path), Settings::default());
    }

    #[test]
    fn load_settings_from_malformed_file_reports_and_returns_default() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("settings.json");
        std::fs::write(&path, b"invalid json").unwrap();
        assert_eq!(load_settings_from(&path), Settings::default());
    }
}
