//! Load the current theme for the config window, and apply edits: send to a
//! running hub over the socket, or write settings.json's `theme` field
//! directly when no hub is up.

use opusdm_ipc::{DesktopSettings, IconSetId, Settings, Theme};
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

/// Load the current `Settings` (defaults if missing/unparseable), return its
/// `theme`.
pub fn load() -> Theme {
    load_settings().theme
}

/// Load the full current `Settings` (defaults if missing/unparseable) -
/// unlike [`load`], keeps `appearance`/`behavior` too, needed to merge a
/// custom theme's partial overrides over them.
pub fn load_settings() -> Settings {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<Settings>(&c).ok())
        .unwrap_or_default()
}

/// Decide what a selected icon set needs applied: `IconSetId::None` needs
/// only the given `current` theme's other fields (colours/background as
/// already edited in the UI) with `icon_set = None`; `IconSetId::Custom(slug)`
/// merges that theme's `theme.json` over `current`/`current_desktop` via
/// [`crate::apply::merge_custom_theme`], returning a `DesktopSettings` to
/// send only when the theme actually touches desktop metrics.
pub fn apply_icon_set(
    icon_set: &IconSetId,
    current: &Settings,
    current_desktop: &DesktopSettings,
) -> (Theme, Option<DesktopSettings>) {
    match icon_set {
        IconSetId::None => {
            let mut theme = current.theme.clone();
            theme.icon_set = IconSetId::None;
            (theme, None)
        }
        IconSetId::Custom(slug) => {
            let data = opusdm_ipc::themes::load_theme_data(slug);
            let merged = crate::apply::merge_custom_theme(slug, &data, current, current_desktop);
            let desktop_changed = merged.desktop != *current_desktop;
            (merged.theme, desktop_changed.then_some(merged.desktop))
        }
    }
}

/// What [`apply`] did.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Sent to a running hub; it will persist and repaint.
    SentToHub,
    /// No hub running - wrote settings.json; effective on next hub start.
    WroteFile,
    /// Could not send and could not write.
    Failed(String),
}

/// Send `theme` to a running hub, or persist it into settings.json (creating
/// a fresh default `Settings` if none exists yet) if none is running.
pub fn apply(theme: &Theme) -> ApplyOutcome {
    let sock = opusdm_ipc::sockpath::hub_sock_path();
    if opusdm_ipc::sockpath::probe_live_hub(&sock) {
        match send_to_hub(&sock, theme) {
            Ok(()) => return ApplyOutcome::SentToHub,
            Err(e) => eprintln!("opusdm-theme: hub send failed, writing file: {e}"),
        }
    }
    match write_file(theme) {
        Ok(()) => ApplyOutcome::WroteFile,
        Err(e) => ApplyOutcome::Failed(e.to_string()),
    }
}

/// Connect to the hub socket and send one `SetTheme` frame.
fn send_to_hub(sock: &std::path::Path, theme: &Theme) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(sock)?;
    opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::SetTheme(theme.clone()))
}

/// Read the current settings.json (or a fresh default), overwrite its
/// `theme` field, and write it back.
fn write_file(theme: &Theme) -> std::io::Result<()> {
    let path = config_path();
    let mut settings: Settings = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    settings.theme = theme.clone();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)
}

/// Apply `theme`, and `desktop` if a custom theme's metrics changed it.
/// Same hub-or-file fallback as [`apply`], run for both messages.
pub fn apply_full(theme: &Theme, desktop: Option<&DesktopSettings>) -> ApplyOutcome {
    let outcome = apply(theme);
    if let (ApplyOutcome::SentToHub, Some(d)) = (&outcome, desktop) {
        let sock = opusdm_ipc::sockpath::hub_sock_path();
        if let Err(e) = send_desktop_to_hub(&sock, d) {
            eprintln!("opusdm-theme: desktop settings send failed: {e}");
        }
    } else if let (ApplyOutcome::WroteFile, Some(d)) = (&outcome, desktop) {
        if let Err(e) = write_desktop_file(d) {
            eprintln!("opusdm-theme: desktop settings write failed: {e}");
        }
    }
    outcome
}

fn send_desktop_to_hub(sock: &std::path::Path, settings: &DesktopSettings) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(sock)?;
    opusdm_ipc::frame::send(&mut stream, &opusdm_ipc::ToHub::SetDesktopSettings(settings.clone()))
}

fn write_desktop_file(settings: &DesktopSettings) -> std::io::Result<()> {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("desktop.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Guards tests that mutate the process-wide `XDG_CONFIG_HOME`/
    /// `OPUSDM_HUB_SOCK` env vars, which libtest would otherwise race across
    /// threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn apply_writes_the_theme_field_when_no_hub_is_running() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        std::env::set_var("OPUSDM_HUB_SOCK", td.path().join("definitely-absent.sock"));
        let mut t = Theme::default();
        t.icon_set = opusdm_ipc::IconSetId::Custom("breeze-dark".to_string());
        match apply(&t) {
            ApplyOutcome::WroteFile => {}
            other => panic!("expected WroteFile, got {other:?}"),
        }
        let written = std::fs::read_to_string(td.path().join("opusdm").join("settings.json")).unwrap();
        let back: Settings = serde_json::from_str(&written).unwrap();
        assert_eq!(back.theme.icon_set, opusdm_ipc::IconSetId::Custom("breeze-dark".to_string()));
    }

    #[test]
    fn apply_preserves_other_settings_fields_when_writing_the_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        std::env::set_var("OPUSDM_HUB_SOCK", td.path().join("definitely-absent.sock"));
        let mut existing = Settings::default();
        existing.appearance.monospace_list = true;
        std::fs::create_dir_all(td.path().join("opusdm")).unwrap();
        std::fs::write(
            td.path().join("opusdm").join("settings.json"),
            serde_json::to_string(&existing).unwrap(),
        )
        .unwrap();

        apply(&Theme::default());

        let written = std::fs::read_to_string(td.path().join("opusdm").join("settings.json")).unwrap();
        let back: Settings = serde_json::from_str(&written).unwrap();
        assert!(back.appearance.monospace_list, "unrelated settings must survive a theme-only write");
    }

    #[test]
    fn apply_icon_set_none_needs_no_desktop_merge() {
        let (theme, desktop) = apply_icon_set(
            &opusdm_ipc::IconSetId::None,
            &Settings::default(),
            &opusdm_ipc::DesktopSettings::default(),
        );
        assert_eq!(theme.icon_set, opusdm_ipc::IconSetId::None);
        assert!(desktop.is_none());
    }

    #[test]
    fn apply_icon_set_custom_merges_theme_json_over_current_settings() {
        let _guard = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        let dir = td.path().join("opusdm").join("themes").join("t");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"name":"t","label":"T"}"#).unwrap();
        std::fs::write(dir.join("theme.json"), r#"{"desktop":{"icon_size":100}}"#).unwrap();

        let mut current_desktop = opusdm_ipc::DesktopSettings::default();
        current_desktop.icon_size = 64;
        let (theme, desktop) = apply_icon_set(
            &opusdm_ipc::IconSetId::Custom("t".to_string()),
            &Settings::default(),
            &current_desktop,
        );
        assert_eq!(theme.icon_set, opusdm_ipc::IconSetId::Custom("t".to_string()));
        assert_eq!(desktop.unwrap().icon_size, 100);
    }
}
