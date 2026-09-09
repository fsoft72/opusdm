//! The application menu: installed `.desktop` entries grouped by freedesktop
//! category, plus Settings / About / Logout / Shutdown. A `gtk::MenuButton`
//! whose popover lists one expander per non-empty category.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use std::process::Command;

use freedesktop_desktop_entry::{self as fde, DesktopEntry};

use crate::PanelHooks;

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
    icon: Option<String>,
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
        by_bucket.entry(bucket).or_default().push(App {
            name,
            icon: entry.icon().map(str::to_string),
            path: entry.path.clone(),
        });
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

/// Build the menu button and its popover.
pub fn build(hooks: PanelHooks, parent: &gtk::ApplicationWindow) -> gtk::Widget {
    let button = gtk::MenuButton::new();
    button.set_label("Applications");
    button.add_css_class("flat");

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

    for (bucket, apps) in load_apps() {
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 1);
        inner.set_margin_start(12);
        for app in apps {
            let row = menu_row(app.icon.as_deref(), &app.name);
            let path = app.path.clone();
            let popover = popover.clone();
            row.connect_clicked(move |_| {
                popover.popdown();
                launch(&path);
            });
            inner.append(&row);
        }
        let expander = gtk::Expander::new(Some(bucket.label()));
        expander.set_child(Some(&inner));
        list.append(&expander);
    }

    list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Settings -> hub dialog.
    {
        let on_settings = hooks.on_settings;
        let popover = popover.clone();
        let row = menu_row(Some("preferences-system"), "Settings");
        row.connect_clicked(move |_| {
            popover.popdown();
            on_settings();
        });
        list.append(&row);
    }
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
    button.upcast()
}

/// Launch a `.desktop` file through GIO so field codes and terminal handling
/// are honoured. Errors are logged, not fatal.
fn launch(path: &std::path::Path) {
    let Some(info) = gio::DesktopAppInfo::from_filename(path) else {
        eprintln!("opusdm-panel: cannot read {}", path.display());
        return;
    };
    if let Err(e) = info.launch(&[], gio::AppLaunchContext::NONE) {
        eprintln!("opusdm-panel: launch failed for {}: {e}", path.display());
    }
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
        "<span size='xx-large' weight='bold'>OpusDM</span>\n\
         <i>A Linux tribute to Amiga Directory Opus 5</i>\n\n\
         Written by Fabio Rotondo\n\
         <a href='https://github.com/fsof72'>https://github.com/fsof72</a>\n\n\
         <small>This software is Open Source</small>",
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
