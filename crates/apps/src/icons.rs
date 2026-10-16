//! Icon lookup shared by every process that shows an application icon: a
//! picture file, a theme icon name, or the icon of an installed
//! application named by its desktop file id.

use gtk4 as gtk;
use gtk::gio;
use gtk::gio::prelude::AppInfoExt;

/// Suffix of a desktop file id.
const DESKTOP_SUFFIX: &str = ".desktop";

/// The desktop file ids to try for `name`, in order: the name itself (with
/// `.desktop` added when missing), then its lowercase form when it differs
/// (`org.gnome.Nautilus`, or `foot` for `foot.desktop`).
fn desktop_ids(name: &str) -> Vec<String> {
    if name.is_empty() { return Vec::new(); }
    let id = if name.ends_with(DESKTOP_SUFFIX) { name.to_string() } else { format!("{name}{DESKTOP_SUFFIX}") };
    let lower = id.to_lowercase();
    if lower == id { vec![id] } else { vec![id, lower] }
}

/// The icon of the installed application whose desktop file is named by
/// `name` (a desktop file id with or without `.desktop`, or a Wayland app
/// id). `None` when no such application is installed or it has no icon.
pub fn desktop_icon(name: &str) -> Option<gio::Icon> {
    desktop_ids(name).iter().find_map(|id| gio::DesktopAppInfo::new(id)).and_then(|info| info.icon())
}

/// Whether the icon theme of the default display has an icon called `name`.
fn theme_has_icon(name: &str) -> bool {
    gtk::gdk::Display::default().is_some_and(|display| gtk::IconTheme::for_display(&display).has_icon(name))
}

/// `name` when the icon theme has it, else `fallback`.
pub fn themed_or<'a>(name: &'a str, fallback: &'a str) -> &'a str {
    if name.is_empty() { return fallback; }
    if theme_has_icon(name) { name } else { fallback }
}

/// An image for `icon`: the picture file it names when it is an absolute
/// path, else the theme icon of that name, else the icon of the application
/// whose desktop file it names. `None` when nothing matches, so the caller
/// picks its own fallback and pixel size.
pub fn app_image(icon: &str) -> Option<gtk::Image> {
    if icon.is_empty() { return None; }
    if icon.starts_with('/') {
        return match gtk::gdk::Texture::from_filename(icon) {
            Ok(texture) => Some(gtk::Image::from_paintable(Some(&texture))),
            Err(e) => {
                opusdm_ipc::debug_log!("opusdm-apps", "cannot load picture {icon}: {e}");
                None
            }
        };
    }
    if theme_has_icon(icon) { return Some(gtk::Image::from_icon_name(icon)); }
    desktop_icon(icon).map(|gicon| gtk::Image::from_gicon(&gicon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_ids_add_the_suffix_and_try_lowercase() {
        assert_eq!(desktop_ids("foot"), vec!["foot.desktop"]);
        assert_eq!(desktop_ids("foot.desktop"), vec!["foot.desktop"]);
        assert_eq!(desktop_ids("org.gnome.Nautilus"), vec!["org.gnome.Nautilus.desktop", "org.gnome.nautilus.desktop"]);
        assert!(desktop_ids("").is_empty());
    }

    #[test]
    fn an_empty_name_resolves_to_nothing() {
        assert!(app_image("").is_none());
        assert_eq!(themed_or("", "fallback"), "fallback");
    }
}
