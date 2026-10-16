//! The icon shared by every OpusDM window.

use gtk4::prelude::*;

/// Name of the icon installed in the hicolor theme (`opusdm.png`).
pub const APP_ICON_NAME: &str = "opusdm";

/// Make `opusdm` the icon of every window of this process. Call it once GTK
/// is initialised (e.g. from `connect_startup`). The icon itself is installed
/// by `install.sh` under `share/icons/hicolor`.
pub fn set_app_icon() {
    gtk4::Window::set_default_icon_name(APP_ICON_NAME);
}
