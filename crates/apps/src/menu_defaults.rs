//! The default application menu: one folder per freedesktop category,
//! holding the installed apps that belong to it. Shared by `opusdm-panel`
//! (seeds `menu.json` on first run, or whenever it is emptied) and
//! `opusdm-settings`'s Menu tab ("Reset Default").

use freedesktop_desktop_entry::{self as fde, DesktopEntry};
use opusdm_ipc::MenuItem;

/// The fixed set of top-level menu buckets, in display order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Bucket {
    Development,
    Graphics,
    Internet,
    Multimedia,
    Office,
    Games,
    System,
    Settings,
    Utility,
    Other,
}

impl Bucket {
    /// Every bucket, in the order they appear in the menu.
    pub const ORDER: [Bucket; 10] = [
        Bucket::Development,
        Bucket::Graphics,
        Bucket::Internet,
        Bucket::Multimedia,
        Bucket::Office,
        Bucket::Games,
        Bucket::System,
        Bucket::Settings,
        Bucket::Utility,
        Bucket::Other,
    ];

    /// Human label shown as the folder name.
    pub fn label(self) -> &'static str {
        match self {
            Bucket::Development => "Development",
            Bucket::Graphics => "Graphics",
            Bucket::Internet => "Internet",
            Bucket::Multimedia => "Multimedia",
            Bucket::Office => "Office",
            Bucket::Games => "Games",
            Bucket::System => "System",
            Bucket::Settings => "Settings",
            Bucket::Utility => "Utility",
            Bucket::Other => "Other",
        }
    }
}

/// Map a `.desktop` `Categories` list to a single bucket. The first category
/// that maps wins; anything unrecognised falls through to `Other`.
pub fn category_bucket(categories: &[&str]) -> Bucket {
    for c in categories {
        match *c {
            "Development" => return Bucket::Development,
            "Graphics" => return Bucket::Graphics,
            "Network" | "WebBrowser" | "Email" | "InstantMessaging" => return Bucket::Internet,
            "AudioVideo" | "Audio" | "Video" | "Player" | "Music" => return Bucket::Multimedia,
            "Office" | "Spreadsheet" | "WordProcessor" | "Presentation" => return Bucket::Office,
            "Game" => return Bucket::Games,
            "Settings" => return Bucket::Settings,
            "System" => return Bucket::System,
            "Utility" | "Accessories" => return Bucket::Utility,
            _ => {}
        }
    }
    Bucket::Other
}

/// Whether an entry belongs in the menu: an application, not hidden, not
/// `NoDisplay`, with something to launch.
pub fn entry_visible(entry: &DesktopEntry) -> bool {
    if entry.type_().unwrap_or("Application") != "Application" {
        return false;
    }
    if entry.no_display() {
        return false;
    }
    if entry.desktop_entry("Hidden") == Some("true") {
        return false;
    }
    entry.exec().is_some()
}

/// The default custom menu: one folder per non-empty category, in
/// [`Bucket::ORDER`], holding the ids (as [`crate::apps::id_for_desktop_file`]
/// resolves them) of its installed apps, sorted by name (case-insensitive).
pub fn default_menu_items() -> Vec<MenuItem> {
    let locales = fde::get_languages_from_env();
    let mut by_bucket: std::collections::HashMap<Bucket, Vec<(String, std::path::PathBuf)>> =
        std::collections::HashMap::new();

    for entry in fde::desktop_entries(&locales) {
        if !entry_visible(&entry) {
            continue;
        }
        let cats = entry.categories().unwrap_or_default();
        let bucket = category_bucket(&cats);
        let name = entry
            .name(&locales)
            .map(|c| c.into_owned())
            .unwrap_or_else(|| entry.id().to_string());
        by_bucket.entry(bucket).or_default().push((name, entry.path.clone()));
    }

    let mut out = Vec::new();
    for bucket in Bucket::ORDER {
        let Some(mut apps) = by_bucket.remove(&bucket) else { continue };
        apps.sort_by_key(|(name, _)| name.to_lowercase());
        let items = apps
            .iter()
            .filter_map(|(_, path)| crate::apps::id_for_desktop_file(path))
            .map(|id| MenuItem::App { id })
            .collect();
        out.push(MenuItem::Folder { name: bucket.label().to_string(), items });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_bucket_takes_the_first_recognised() {
        assert_eq!(category_bucket(&["X-Foo", "Network", "GTK"]), Bucket::Internet);
        assert_eq!(category_bucket(&["Development", "IDE"]), Bucket::Development);
        assert_eq!(category_bucket(&["AudioVideo"]), Bucket::Multimedia);
    }

    #[test]
    fn category_bucket_unknown_is_other() {
        assert_eq!(category_bucket(&["X-Nonsense"]), Bucket::Other);
        assert_eq!(category_bucket(&[]), Bucket::Other);
    }

    fn entry(pairs: &[(&str, &str)]) -> DesktopEntry {
        let mut e = DesktopEntry::from_appid("test.app".to_string());
        for (k, v) in pairs {
            e.add_desktop_entry(k.to_string(), v.to_string());
        }
        e
    }

    #[test]
    fn entry_visible_filters_nodisplay_and_hidden() {
        assert!(entry_visible(&entry(&[("Type", "Application"), ("Exec", "foo")])));
        assert!(!entry_visible(&entry(&[
            ("Type", "Application"),
            ("Exec", "foo"),
            ("NoDisplay", "true"),
        ])));
        assert!(!entry_visible(&entry(&[
            ("Type", "Application"),
            ("Exec", "foo"),
            ("Hidden", "true"),
        ])));
    }

    #[test]
    fn entry_visible_needs_an_application_with_exec() {
        assert!(!entry_visible(&entry(&[("Type", "Link"), ("Exec", "foo")])));
        assert!(!entry_visible(&entry(&[("Type", "Application")])));
    }

    #[test]
    fn bucket_order_covers_every_variant_once() {
        assert_eq!(Bucket::ORDER.len(), 10);
    }
}
