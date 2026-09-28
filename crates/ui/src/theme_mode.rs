//! GTK-wide dark/light theme preference.

use gtk4 as gtk;
use opusdm_ipc::ThemeMode;

/// Ask GTK for the dark or light variant of the active theme
/// (`gtk-application-prefer-dark-theme`), so every widget - buttons,
/// entries, dialog chrome included - is themed consistently by the real
/// theme engine, rather than only the specific classes OpusDM hand-paints
/// in its own CSS (file list rows, status bar, and - when enabled - the
/// window background).
pub fn apply_theme_mode(mode: ThemeMode) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(mode == ThemeMode::Dark);
    }
}
