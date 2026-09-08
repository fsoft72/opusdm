//! Load desktop settings for the config window, and apply edits: send them to
//! a running hub over the socket, or write `desktop.json` directly when no hub
//! is up.

use gtk4 as gtk;
use opusdm_ipc::DesktopSettings;
use std::path::{Path, PathBuf};

/// Path to `~/.config/opusdm/desktop.json`. Same resolution as the hub.
pub fn config_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    path.push("opusdm");
    path.push("desktop.json");
    path
}

/// Load current desktop settings, or defaults if the file is missing or
/// unparseable. Always clamped.
pub fn load() -> DesktopSettings {
    let path = config_path();
    let parsed = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<DesktopSettings>(&c).ok());
    parsed.unwrap_or_default().sanitized()
}

/// What [`apply`] did.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Sent to a running hub; it will persist and repaint.
    SentToHub,
    /// No hub running - wrote `desktop.json`; effective on next hub start.
    WroteFile,
    /// Could not send and could not write.
    Failed(String),
}

/// Send `settings` to a running hub, or persist to disk if none is running.
pub fn apply(settings: &DesktopSettings) -> ApplyOutcome {
    let sock = opusdm_ipc::sockpath::hub_sock_path();
    if opusdm_ipc::sockpath::probe_live_hub(&sock) {
        match send_to_hub(&sock, settings) {
            Ok(()) => return ApplyOutcome::SentToHub,
            Err(e) => eprintln!("opus-desktop-config: hub send failed, writing file: {e}"),
        }
    }
    match write_file(settings) {
        Ok(()) => ApplyOutcome::WroteFile,
        Err(e) => ApplyOutcome::Failed(e.to_string()),
    }
}

/// Connect to the hub socket and send one `SetDesktopSettings` frame.
fn send_to_hub(sock: &Path, settings: &DesktopSettings) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(sock)?;
    opusdm_ipc::frame::send(
        &mut stream,
        &opusdm_ipc::ToHub::SetDesktopSettings(settings.clone()),
    )
}

/// Write `settings` to `desktop.json`, creating the config directory.
fn write_file(settings: &DesktopSettings) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(settings)?)
}

/// `gdk::RGBA` -> `#rrggbb` (alpha dropped).
pub fn rgba_to_hex(c: &gtk::gdk::RGBA) -> String {
    let to_u8 = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", to_u8(c.red()), to_u8(c.green()), to_u8(c.blue()))
}

/// `#rrggbb` (or any `gdk::RGBA::parse`-able string) -> `gdk::RGBA`.
/// Unparseable input yields opaque black.
pub fn hex_to_rgba(hex: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(hex).unwrap_or_else(|_| gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips_through_rgba() {
        let c = hex_to_rgba("#3c8ad9");
        assert_eq!(rgba_to_hex(&c), "#3c8ad9");
    }

    #[test]
    fn bad_hex_is_opaque_black() {
        assert_eq!(rgba_to_hex(&hex_to_rgba("not-a-colour")), "#000000");
    }

    #[test]
    fn apply_writes_the_file_when_no_hub_is_running() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        std::env::set_var("OPUSDM_HUB_SOCK", td.path().join("definitely-absent.sock"));
        let mut d = DesktopSettings::default();
        d.icon_size = 88;
        match apply(&d) {
            ApplyOutcome::WroteFile => {}
            other => panic!("expected WroteFile, got {other:?}"),
        }
        let written =
            std::fs::read_to_string(td.path().join("opusdm").join("desktop.json")).unwrap();
        assert!(written.contains("\"icon_size\": 88"));
    }
}
