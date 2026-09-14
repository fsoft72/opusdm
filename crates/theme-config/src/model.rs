//! Load the current settings for the config window, and apply edits: send
//! the whole `Settings` to a running hub over the socket atomically, or
//! write settings.json directly when no hub is up.

use opusdm_ipc::{DesktopSettings, IconSetId, Settings};
use std::path::PathBuf;

/// Path to `~/.config/opusdm/settings.json`. Same resolution as the hub.
pub fn config_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("settings.json");
    path
}

/// Load the full current `Settings` (defaults if missing/unparseable).
pub fn load_settings() -> Settings {
    load_settings_from(&config_path())
}

/// Testable core of [`load_settings`].
pub fn load_settings_from(path: &std::path::Path) -> Settings {
    if !path.exists() {
        return Settings::default();
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("opusdm-theme: cannot read settings from {}: {e}", path.display());
            return Settings::default();
        }
    };
    match serde_json::from_str::<Settings>(&content) {
        Ok(settings) => settings,
        Err(e) => {
            eprintln!("opusdm-theme: malformed settings in {}: {e}", path.display());
            Settings::default()
        }
    }
}

/// What applying a selected icon set produces: the full `Settings` and
/// `DesktopSettings` to apply. For `IconSetId::None` these are just `ui`/
/// `ui_desktop` with the icon set forced to `None`; for `IconSetId::Custom`,
/// the theme's `theme.json` fields are merged over `ui`/`ui_desktop` (only
/// the fields the theme actually sets are overwritten).
pub struct IconSetApplication {
    pub settings: Settings,
    pub desktop: DesktopSettings,
}

/// Decide what a selected icon set needs applied. `ui`/`ui_desktop` are the
/// window's current widget state (colours, background, appearance,
/// behavior, desktop metrics) - everything a custom theme's `theme.json`
/// doesn't explicitly override is taken from here, so an in-progress edit
/// is never silently discarded by picking a theme.
pub fn apply_icon_set(icon_set: &IconSetId, ui: &Settings, ui_desktop: &DesktopSettings) -> IconSetApplication {
    match icon_set {
        IconSetId::None => {
            let mut settings = ui.clone();
            settings.theme.icon_set = IconSetId::None;
            IconSetApplication { settings, desktop: ui_desktop.clone() }
        }
        IconSetId::Custom(slug) => {
            let data = opusdm_ipc::themes::load_theme_data(slug);
            let merged = crate::apply::merge_custom_theme(slug, &data, ui, ui_desktop);
            let settings = Settings { theme: merged.theme, appearance: merged.appearance, behavior: merged.behavior };
            IconSetApplication { settings, desktop: merged.desktop }
        }
    }
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
            Err(e) => eprintln!("opusdm-theme: hub send failed, writing files: {e}"),
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
    let settings_path = config_path();
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&settings_path, serde_json::to_string_pretty(settings)?)?;

    let mut desktop_path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    desktop_path.push("opusdm");
    desktop_path.push("desktop.json");
    std::fs::write(&desktop_path, serde_json::to_string_pretty(desktop)?)
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
        let mut desktop = DesktopSettings::default();
        desktop.icon_size = 96;

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
    fn apply_icon_set_none_keeps_the_ui_state_as_is() {
        let mut ui = Settings::default();
        ui.theme.colors.file_fg = "#123456".to_string();
        ui.appearance.monospace_list = true;
        let ui_desktop = DesktopSettings::default();

        let result = apply_icon_set(&opusdm_ipc::IconSetId::None, &ui, &ui_desktop);
        assert_eq!(result.settings.theme.icon_set, opusdm_ipc::IconSetId::None);
        assert_eq!(result.settings.theme.colors.file_fg, "#123456");
        assert!(result.settings.appearance.monospace_list);
        assert_eq!(result.desktop, ui_desktop);
    }

    #[test]
    fn apply_icon_set_custom_merges_theme_json_over_ui_state() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let dir = td.path().join("opusdm").join("themes").join("t");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"name":"t","label":"T"}"#).unwrap();
        std::fs::write(dir.join("theme.json"), r#"{"desktop":{"icon_size":100}}"#).unwrap();

        let mut ui = Settings::default();
        ui.appearance.monospace_list = true;
        let mut ui_desktop = DesktopSettings::default();
        ui_desktop.icon_size = 64;

        let result = apply_icon_set(&opusdm_ipc::IconSetId::Custom("t".to_string()), &ui, &ui_desktop);
        assert_eq!(result.settings.theme.icon_set, opusdm_ipc::IconSetId::Custom("t".to_string()));
        assert_eq!(result.desktop.icon_size, 100);
        // theme.json didn't touch appearance - the UI's live edit survives.
        assert!(result.settings.appearance.monospace_list);
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
