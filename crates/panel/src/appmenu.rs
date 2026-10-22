//! The application menu: a `gtk::MenuButton` whose popover renders
//! `menu.json`'s tree of folders and apps, plus Settings / About / Logout /
//! Shutdown. `menu.json` is seeded (see `opusdm_apps::menu_defaults`) with
//! the installed apps grouped by freedesktop category on first run (or
//! whenever it is emptied), then edited with the Apps editor
//! (`opusdm-apps-editor`). The items are re-read every time the menu opens.

use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use crate::widgets;
use opusdm_apps::apps;
use opusdm_ipc::{MenuItem, PanelPosition};
use std::process::Command;

/// Width of the menu popover, in pixels.
const MENU_WIDTH: i32 = 560;

/// Pixel size of the icon of a menu row.
const MENU_ICON_SIZE: i32 = 24;

/// CSS class of the application menu popover (bigger text and rows, see
/// `install_css`).
const START_MENU_CLASS: &str = "opusdm-start-menu";

/// A flat, left-aligned menu row with an icon.
fn menu_row(icon: Option<&str>, label: &str) -> gtk::Button {
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let image = match icon {
        Some(name) if name.starts_with('/') => gtk::Image::from_file(name),
        Some(name) => gtk::Image::from_icon_name(name),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(MENU_ICON_SIZE);
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

/// True when `name` matches the search `filter` (case-insensitive substring);
/// an empty filter matches everything.
fn matches_filter(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase())
}

/// A menu row for a custom-menu app entry, resolved by desktop file id (or
/// absolute path). `None` when `id` is not installed or its name does not match
/// `filter`, so the caller can skip it rather than show a dead entry.
fn custom_app_row(id: &str, popover: &gtk::Popover, filter: &str) -> Option<gtk::Button> {
    let entry = apps::resolve(&[id.to_string()]).into_iter().next()?;
    if !matches_filter(entry.info.name().as_str(), filter) { return None; }
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let image = match entry.info.icon() {
        Some(icon) => gtk::Image::from_gicon(&icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(MENU_ICON_SIZE);
    inner.append(&image);
    let text = gtk::Label::new(Some(entry.info.name().as_str()));
    text.set_xalign(0.0);
    inner.append(&text);

    let btn = gtk::Button::new();
    btn.set_child(Some(&inner));
    btn.add_css_class("flat");
    btn.set_halign(gtk::Align::Fill);

    let menu = popover.clone();
    let info = entry.info.clone();
    btn.connect_clicked(move |_| {
        menu.popdown();
        apps::launch(&info);
    });
    attach_quick_apps_menu(&btn, id, popover);
    Some(btn)
}

/// Right click on a menu row opens "Add to quick apps" (disabled when the app
/// is already one); `menu` is the application menu popover, closed on use.
fn attach_quick_apps_menu(btn: &gtk::Button, id: &str, menu: &gtk::Popover) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.add_css_class(crate::hub_look::MENU_CLASS);
    let add = gtk::Button::with_label("Add to quick apps");
    add.add_css_class("flat");
    popover.set_child(Some(&add));
    popover.set_parent(btn);
    {
        let popover = popover.clone();
        btn.connect_destroy(move |_| popover.unparent());
    }
    {
        let (popover, menu, id) = (popover.clone(), menu.clone(), id.to_string());
        add.connect_clicked(move |_| {
            popover.popdown();
            menu.popdown();
            widgets::quickapps::add_app(&id);
        });
    }
    let (id, add) = (id.to_string(), add.clone());
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    click.connect_pressed(move |_, _, _, _| {
        add.set_sensitive(!widgets::quickapps::contains(&id));
        popover.popup();
    });
    btn.add_controller(click);
}

/// Render one level of the custom menu tree into `into`: apps as rows,
/// folders as expanders nesting their own items. Only apps matching `filter`
/// are shown; a folder left with no rows is not shown at all, and while a
/// filter is active the surviving folders are opened.
fn build_custom_items(items: &[MenuItem], into: &gtk::Box, popover: &gtk::Popover, filter: &str) {
    for item in items {
        match item {
            MenuItem::App { id } => {
                if let Some(row) = custom_app_row(id, popover, filter) {
                    into.append(&row);
                }
            }
            MenuItem::Folder { name, items } => {
                let inner = gtk::Box::new(gtk::Orientation::Vertical, 1);
                inner.set_margin_start(12);
                build_custom_items(items, &inner, popover, filter);
                if inner.first_child().is_none() { continue; }
                let expander = gtk::Expander::new(Some(name));
                expander.set_expanded(!filter.is_empty());
                expander.set_child(Some(&inner));
                into.append(&expander);
            }
        }
    }
}

/// The menu tree from `menu.json`, seeded from the installed apps grouped by
/// category when the file is missing or was emptied, so the panel and the
/// Apps editor always share one real file.
fn load_items() -> Vec<MenuItem> {
    match opusdm_ipc::menu::load_menu_config() {
        Some(cfg) if !cfg.items.is_empty() => cfg.items,
        _ => {
            let cfg = opusdm_ipc::MenuConfig { items: opusdm_apps::menu_defaults::default_menu_items() };
            if let Err(e) = opusdm_ipc::menu::save_menu_config(&cfg) {
                opusdm_ipc::err_log!("panel", "cannot write menu.json: {e}");
            }
            cfg.items
        }
    }
}

/// Replace the content of `custom` with the current menu tree, narrowed to
/// the apps whose name matches `filter`.
fn fill_custom(custom: &gtk::Box, popover: &gtk::Popover, filter: &str) {
    while let Some(child) = custom.first_child() {
        custom.remove(&child);
    }
    build_custom_items(&load_items(), custom, popover, filter);
}

/// Build the menu button and its popover. The popover opens downwards from a
/// top panel and upwards from a bottom one (`position`).
pub fn build(parent: &gtk::ApplicationWindow, position: PanelPosition) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_child(Some(&crate::hub_look::new_menu_image()));
    button.add_css_class("flat");
    button.add_css_class("opusdm-panel-menu");
    set_direction(&button, position);

    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.add_css_class(START_MENU_CLASS);
    popover.add_css_class(crate::hub_look::MENU_CLASS);

    // Search box on top: filters the apps by name as the user types.
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Filter applications"));
    search.set_margin_top(6);
    search.set_margin_bottom(2);
    search.set_margin_start(6);
    search.set_margin_end(6);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(420);
    scroll.set_max_content_height(560);
    scroll.set_min_content_width(MENU_WIDTH);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_margin_top(4);
    list.set_margin_bottom(4);
    list.set_margin_start(4);
    list.set_margin_end(4);

    // The custom items sit in their own box, refilled from menu.json every
    // time the menu opens, so the Apps editor's changes show up at once.
    let custom = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.append(&custom);
    fill_custom(&custom, &popover, "");
    {
        let (custom, menu, search) = (custom.clone(), popover.clone(), search.clone());
        popover.connect_show(move |_| {
            search.set_text("");
            fill_custom(&custom, &menu, "");
            search.grab_focus();
        });
    }
    {
        let (custom, menu) = (custom.clone(), popover.clone());
        search.connect_search_changed(move |entry| fill_custom(&custom, &menu, entry.text().as_str()));
    }

    // Footer, apart from the app list: About on the left, Logout and
    // Shutdown on the right.
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    footer.set_margin_top(4);
    footer.set_margin_bottom(4);
    footer.set_margin_start(4);
    footer.set_margin_end(4);

    // About.
    {
        let popover = popover.clone();
        let parent = parent.clone();
        let row = menu_row(Some("help-about"), "About");
        row.connect_clicked(move |_| {
            popover.popdown();
            show_about(&parent);
        });
        footer.append(&row);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    // Logout.
    {
        let popover = popover.clone();
        let parent = parent.clone();
        let row = menu_row(Some("system-log-out"), "Logout");
        row.connect_clicked(move |_| {
            popover.popdown();
            confirm(&parent, "Log out?", "This ends the OpusDM session.", logout);
        });
        footer.append(&row);
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
        footer.append(&row);
    }

    scroll.set_child(Some(&list));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_size_request(MENU_WIDTH, -1);
    content.append(&search);
    content.append(&scroll);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&footer);
    popover.set_child(Some(&content));
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
        opusdm_ipc::err_log!("opusdm-panel", "cannot run {bin}: {e}");
    }
}

/// End the session: the Wayland compositor is told to quit, on X11 openbox.
fn logout() {
    if !opusdm_ipc::session::is_wayland() { return run_detached("openbox", &["--exit"]); }
    if let Err(e) = opusdm_wayland::compositor::exit() {
        opusdm_ipc::err_log!("opusdm-panel", "cannot log out: {e}");
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
    opusdm_ui::show_about(parent);
}

#[cfg(test)]
mod tests {
    use super::matches_filter;

    #[test]
    fn empty_filter_matches_everything() {
        assert!(matches_filter("Firefox", ""));
    }

    #[test]
    fn filter_is_a_case_insensitive_substring() {
        assert!(matches_filter("Firefox", "fire"));
        assert!(matches_filter("Firefox", "FOX"));
        assert!(!matches_filter("Firefox", "chrome"));
    }
}
