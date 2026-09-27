//! The application menu: a `gtk::MenuButton` whose popover renders
//! `menu.json`'s tree of folders and apps, plus Settings / About / Logout /
//! Shutdown. `menu.json` is seeded with the installed `.desktop` entries
//! grouped by freedesktop category on first run (or whenever it is
//! emptied), then edited through the "Menu" tab in `opusdm-settings`.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_dock::apps;
use opusdm_ipc::{MenuItem, PanelPosition};
use std::process::Command;

use freedesktop_desktop_entry::{self as fde, DesktopEntry};

// Embedded at compile-time so the binary has zero runtime file dependencies.
const BALL_PNG: &[u8] = include_bytes!("../../../assets/ball.png");


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

    /// Human label shown on the expander.
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

/// One launchable application.
struct App {
    name: String,
    path: std::path::PathBuf,
}

/// Read and filter the installed applications, grouped into buckets in
/// [`Bucket::ORDER`], each sorted by name (case-insensitive).
fn load_apps() -> Vec<(Bucket, Vec<App>)> {
    let locales = fde::get_languages_from_env();
    let mut by_bucket: std::collections::HashMap<Bucket, Vec<App>> = std::collections::HashMap::new();

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
        by_bucket.entry(bucket).or_default().push(App { name, path: entry.path.clone() });
    }

    let mut out = Vec::new();
    for bucket in Bucket::ORDER {
        if let Some(mut apps) = by_bucket.remove(&bucket) {
            apps.sort_by_key(|a| a.name.to_lowercase());
            out.push((bucket, apps));
        }
    }
    out
}

/// A flat, left-aligned menu row with an icon.
fn menu_row(icon: Option<&str>, label: &str) -> gtk::Button {
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let image = match icon {
        Some(name) if name.starts_with('/') => gtk::Image::from_file(name),
        Some(name) => gtk::Image::from_icon_name(name),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(18);
    inner.append(&image);
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    inner.append(&text);

    let btn = gtk::Button::new();
    btn.set_child(Some(&inner));
    btn.add_css_class("flat");
    btn.set_halign(gtk::Align::Fill);
    btn
}

/// A menu row for a custom-menu app entry, resolved by desktop file id (or
/// absolute path). `None` when `id` is not installed, so the caller can skip
/// it rather than show a dead entry.
fn custom_app_row(id: &str, popover: &gtk::Popover) -> Option<gtk::Button> {
    let entry = apps::resolve(&[id.to_string()]).into_iter().next()?;
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let image = match entry.info.icon() {
        Some(icon) => gtk::Image::from_gicon(&icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(18);
    inner.append(&image);
    let text = gtk::Label::new(Some(entry.info.name().as_str()));
    text.set_xalign(0.0);
    inner.append(&text);

    let btn = gtk::Button::new();
    btn.set_child(Some(&inner));
    btn.add_css_class("flat");
    btn.set_halign(gtk::Align::Fill);

    let popover = popover.clone();
    let info = entry.info.clone();
    btn.connect_clicked(move |_| {
        popover.popdown();
        apps::launch(&info);
    });
    Some(btn)
}

/// The default custom menu: one folder per non-empty category, holding the
/// ids of its installed apps - what `menu.json` is seeded with on first run
/// (or whenever it is emptied), so `opusdm-settings`'s Menu tab always has
/// something to edit rather than a blank slate.
fn default_menu_items() -> Vec<MenuItem> {
    load_apps()
        .into_iter()
        .map(|(bucket, entries)| {
            let items = entries
                .iter()
                .filter_map(|app| apps::id_for_desktop_file(&app.path))
                .map(|id| MenuItem::App { id })
                .collect();
            MenuItem::Folder { name: bucket.label().to_string(), items }
        })
        .collect()
}

/// Render one level of the custom menu tree into `into`: apps as rows,
/// folders as expanders nesting their own items.
fn build_custom_items(items: &[MenuItem], into: &gtk::Box, popover: &gtk::Popover) {
    for item in items {
        match item {
            MenuItem::App { id } => {
                if let Some(row) = custom_app_row(id, popover) {
                    into.append(&row);
                }
            }
            MenuItem::Folder { name, items } => {
                let inner = gtk::Box::new(gtk::Orientation::Vertical, 1);
                inner.set_margin_start(12);
                build_custom_items(items, &inner, popover);
                let expander = gtk::Expander::new(Some(name));
                expander.set_child(Some(&inner));
                into.append(&expander);
            }
        }
    }
}

/// Build the menu button and its popover. The popover opens downwards from a
/// top panel and upwards from a bottom one (`position`).
pub fn build(parent: &gtk::ApplicationWindow, position: PanelPosition) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    let ball_bytes = glib::Bytes::from_static(BALL_PNG);
    let ball_texture = gdk::Texture::from_bytes(&ball_bytes).expect("valid ball.png asset");
    let ball_image = gtk::Image::from_paintable(Some(&ball_texture));
    ball_image.set_pixel_size(18);
    button.set_child(Some(&ball_image));
    button.add_css_class("flat");
    set_direction(&button, position);

    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(420);
    scroll.set_max_content_height(560);
    scroll.set_min_content_width(260);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_margin_top(4);
    list.set_margin_bottom(4);
    list.set_margin_start(4);
    list.set_margin_end(4);

    // menu.json, seeded from the installed apps grouped by category when
    // it is missing or was emptied, so the panel and the "Menu" tab in
    // `opusdm-settings` always share one real file to edit.
    let cfg = match opusdm_ipc::menu::load_menu_config() {
        Some(cfg) if !cfg.items.is_empty() => cfg,
        _ => {
            let cfg = opusdm_ipc::MenuConfig { items: default_menu_items() };
            if let Err(e) = opusdm_ipc::menu::save_menu_config(&cfg) {
                opusdm_ipc::err_log!("panel", "cannot write menu.json: {e}");
            }
            cfg
        }
    };
    build_custom_items(&cfg.items, &list, &popover);

    list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // About.
    {
        let popover = popover.clone();
        let parent = parent.clone();
        let row = menu_row(Some("help-about"), "About OpusDM");
        row.connect_clicked(move |_| {
            popover.popdown();
            show_about(&parent);
        });
        list.append(&row);
    }
    list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    // Logout.
    {
        let popover = popover.clone();
        let parent = parent.clone();
        let row = menu_row(Some("system-log-out"), "Logout");
        row.connect_clicked(move |_| {
            popover.popdown();
            confirm(&parent, "Log out?", "This ends the openbox session.", || {
                run_detached("openbox", &["--exit"]);
            });
        });
        list.append(&row);
    }
    // Shutdown.
    {
        let popover = popover.clone();
        let parent = parent.clone();
        let row = menu_row(Some("system-shutdown"), "Shutdown");
        row.connect_clicked(move |_| {
            popover.popdown();
            confirm(&parent, "Shut down?", "This powers the machine off now.", || {
                run_detached("systemctl", &["poweroff"]);
            });
        });
        list.append(&row);
    }

    scroll.set_child(Some(&list));
    popover.set_child(Some(&scroll));
    button.set_popover(Some(&popover));
    button
}

/// Point `button`'s popover away from the panel edge: down from a top panel,
/// up from a bottom one. MenuButton derives the popover's side from its
/// arrow direction.
pub fn set_direction(button: &gtk::MenuButton, position: PanelPosition) {
    button.set_direction(match position {
        PanelPosition::Top => gtk::ArrowType::Down,
        PanelPosition::Bottom => gtk::ArrowType::Up,
    });
}

/// Spawn a detached command; a missing binary is logged, not fatal.
fn run_detached(bin: &str, args: &[&str]) {
    if let Err(e) = Command::new(bin).args(args).spawn() {
        eprintln!("opusdm-panel: cannot run {bin}: {e}");
    }
}

/// A yes/no requester that runs `action` on confirm.
fn confirm(
    parent: &gtk::ApplicationWindow,
    message: &str,
    detail: &str,
    action: impl Fn() + 'static,
) {
    opusdm_ui::requester(
        parent,
        "Request",
        message,
        Some(detail),
        &[("Yes", true), ("Cancel", false)],
        move |ok| {
            if ok {
                action();
            }
        },
    );
}

fn show_about(parent: &gtk::ApplicationWindow) {
    opusdm_ui::requester_rich(
        parent,
        "About OpusDM",
        &format!(
            "<span size='xx-large' weight='bold'>OpusDM</span>\n\
             <i>A Linux tribute to Amiga Directory Opus 5</i>\n\n\
             Version {version}\n\n\
             Written by Fabio Rotondo\n\
             <a href='https://github.com/fsof72'>https://github.com/fsof72</a>\n\n\
             <small>This software is Open Source</small>",
            version = opusdm_ui::VERSION,
        ),
        &[("OK", ())],
        |_| {},
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use freedesktop_desktop_entry::DesktopEntry;

    fn entry(pairs: &[(&str, &str)]) -> DesktopEntry {
        let mut e = DesktopEntry::from_appid("test.app".to_string());
        for (k, v) in pairs {
            e.add_desktop_entry(k.to_string(), v.to_string());
        }
        e
    }

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
