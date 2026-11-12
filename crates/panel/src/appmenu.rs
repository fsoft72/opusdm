//! The application menu: a `gtk::MenuButton` whose popover renders
//! `menu.json`'s tree of folders and apps, plus Settings / About / Logout /
//! Shutdown. `menu.json` is seeded (see `opusdm_apps::menu_defaults`) with
//! the installed apps grouped by freedesktop category on first run (or
//! whenever it is emptied), then edited with the Apps editor
//! (`opusdm-apps-editor`). The items are re-read every time the menu opens.

use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use crate::widgets;
use opusdm_apps::apps;
use opusdm_ipc::{MenuItem, PanelPosition};
use std::process::Command;

/// Width of the menu popover, in pixels.
const MENU_WIDTH: i32 = 560;

/// The settings application.
const SETTINGS_BIN: &str = "opusdm-settings";

/// The Apps editor.
const APPS_EDITOR_BIN: &str = "opusdm-apps-editor";

/// Prefix of the `.desktop` entries the Apps editor creates, the only ones
/// "Delete" removes from disk.
const OWN_ENTRY_PREFIX: &str = "opusdm-";

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

/// True when the search `filter` matches `name` or any of the `extra` texts
/// (case-insensitive substring); an empty filter matches everything.
fn matches_filter_any(name: &str, extra: &[String], filter: &str) -> bool {
    matches_filter(name, filter) || extra.iter().any(|text| matches_filter(text, filter))
}

/// The texts of `info` the menu filter looks at besides its name, the same
/// ones the Run window matches: generic name, keywords and program name.
fn filter_extra(info: &gio::DesktopAppInfo) -> Vec<String> {
    let mut extra: Vec<String> = info.generic_name().map(|s| s.to_string()).into_iter().collect();
    extra.extend(info.keywords().iter().map(|k| k.to_string()));
    extra.extend(opusdm_apps::entries::program_name(info));
    extra
}

/// A menu row for a custom-menu app entry, resolved by desktop file id (or
/// absolute path). `None` when `id` is not installed or neither its name nor
/// its generic name, keywords or program match `filter`, so the caller can
/// skip it rather than show a dead entry.
fn custom_app_row(id: &str, popover: &gtk::Popover, filter: &str) -> Option<gtk::Button> {
    let entry = apps::resolve(&[id.to_string()]).into_iter().next()?;
    if !matches_filter_any(entry.info.name().as_str(), &filter_extra(&entry.info), filter) { return None; }
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
    attach_context_menu(&btn, id, popover);
    Some(btn)
}

/// A flat, left-aligned item of the row context menu.
fn context_item(label: &str) -> gtk::Button {
    let item = gtk::Button::with_label(label);
    item.add_css_class("flat");
    if let Some(child) = item.child().and_downcast::<gtk::Label>() {
        child.set_xalign(0.0);
    }
    item
}

/// Right click on a menu row opens a context menu: "Add to quick apps" and
/// "Add to Dock" (each disabled when the app is already there), "Edit App"
/// (opens the Apps editor on this entry) and "Delete" (asks first, then
/// removes the entry from the menu). `menu` is the application menu popover,
/// closed on use.
fn attach_context_menu(btn: &gtk::Button, id: &str, menu: &gtk::Popover) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.add_css_class(crate::hub_look::MENU_CLASS);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let add = context_item("Add to quick apps");
    let dock = context_item("Add to Dock");
    let edit = context_item("Edit App");
    let delete = context_item("Delete");
    for item in [&add, &dock, &edit, &delete] {
        column.append(item);
    }
    popover.set_child(Some(&column));
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
    {
        let (popover, menu, id) = (popover.clone(), menu.clone(), id.to_string());
        dock.connect_clicked(move |_| {
            popover.popdown();
            menu.popdown();
            add_to_dock(&id);
        });
    }
    {
        let (popover, menu, id) = (popover.clone(), menu.clone(), id.to_string());
        edit.connect_clicked(move |_| {
            popover.popdown();
            menu.popdown();
            edit_app(&id);
        });
    }
    {
        let (popover, menu, id) = (popover.clone(), menu.clone(), id.to_string());
        delete.connect_clicked(move |_| {
            popover.popdown();
            menu.popdown();
            delete_app(&menu, &id);
        });
    }
    let (id, add, dock) = (id.to_string(), add.clone(), dock.clone());
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    click.connect_pressed(move |_, _, _, _| {
        add.set_sensitive(!widgets::quickapps::contains(&id));
        dock.set_sensitive(!dock_contains(&id));
        popover.popup();
    });
    btn.add_controller(click);
}

/// The apps pinned to the dock: `dock.json`, or the default set while no list
/// was ever chosen (what the dock itself shows in that case).
fn dock_app_ids() -> Vec<String> {
    opusdm_ipc::dock::load_dock_config().apps.unwrap_or_else(apps::default_app_ids)
}

/// Whether `id` is already pinned to the dock.
fn dock_contains(id: &str) -> bool {
    dock_app_ids().iter().any(|a| a == id)
}

/// Pin `id` to the dock (a no-op when already there). The dock watches
/// `dock.json` and follows the change.
fn add_to_dock(id: &str) {
    let result = opusdm_ipc::dock::update_dock_config(|cfg| {
        let mut ids = cfg.apps.take().unwrap_or_else(apps::default_app_ids);
        if !ids.iter().any(|a| a == id) {
            ids.push(id.to_string());
        }
        cfg.apps = Some(ids);
    });
    if let Err(e) = result {
        opusdm_ipc::err_log!("opusdm-panel", "cannot add {id} to the dock: {e}");
    }
}

/// Open the Apps editor on the entry `id`, from the folder of the panel first.
fn edit_app(id: &str) {
    let sibling = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(APPS_EDITOR_BIN)));
    let path = sibling.filter(|p| p.exists()).unwrap_or_else(|| std::path::PathBuf::from(APPS_EDITOR_BIN));
    if let Err(e) = Command::new(&path).arg(id).spawn() {
        opusdm_ipc::err_log!("opusdm-panel", "cannot start {}: {e}", path.display());
    }
}

/// Ask for confirmation, then remove the app `id` from the menu (and delete
/// its `.desktop` file when it is one the Apps editor created). `menu` is the
/// application menu popover, used to find the panel window that owns the
/// requester.
fn delete_app(menu: &gtk::Popover, id: &str) {
    let Some(window) = menu.parent().and_then(|p| p.root()).and_downcast::<gtk::ApplicationWindow>() else {
        opusdm_ipc::err_log!("opusdm-panel", "cannot ask to delete {id}: the menu has no panel window");
        return;
    };
    let name = apps::resolve(&[id.to_string()]).into_iter().next().map(|e| e.info.name().to_string()).unwrap_or_else(|| id.to_string());
    let id = id.to_string();
    confirm(&window, &format!("Delete \"{name}\"?"), "The application is removed from the menu.", move || remove_app(&id));
}

/// Remove every occurrence of `id` from `menu.json`; an own `.desktop` entry
/// (`opusdm-` prefix, created by the Apps editor) is deleted too. Failures
/// are logged.
fn remove_app(id: &str) {
    let Some(mut cfg) = opusdm_ipc::menu::load_menu_config() else {
        opusdm_ipc::err_log!("opusdm-panel", "cannot delete {id}: menu.json cannot be read");
        return;
    };
    if opusdm_ipc::menu::remove_app_everywhere(&mut cfg.items, id) {
        if let Err(e) = opusdm_ipc::menu::save_menu_config(&cfg) {
            opusdm_ipc::err_log!("panel", "cannot write menu.json: {e}");
            return;
        }
    }
    if id.starts_with('/') || !id.starts_with(OWN_ENTRY_PREFIX) { return; }
    let Some(path) = dirs::data_dir().map(|d| d.join("applications").join(id)) else { return };
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            opusdm_ipc::err_log!("opusdm-panel", "cannot delete {}: {e}", path.display());
        }
        _ => {}
    }
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
/// the apps whose name, generic name, keywords or program match `filter`.
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

    // Footer, apart from the app list: About and Settings on the left, Logout
    // and Shutdown on the right.
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
    // Settings, next to About.
    {
        let popover = popover.clone();
        let row = menu_row(Some("preferences-system"), "Settings");
        row.connect_clicked(move |_| {
            popover.popdown();
            open_settings();
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

/// Start `opusdm-settings`, from the folder of the panel first.
fn open_settings() {
    let sibling = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(SETTINGS_BIN)));
    let path = sibling.filter(|p| p.exists()).unwrap_or_else(|| std::path::PathBuf::from(SETTINGS_BIN));
    if let Err(e) = Command::new(&path).spawn() {
        opusdm_ipc::err_log!("opusdm-panel", "cannot start {}: {e}", path.display());
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
    use super::{matches_filter, matches_filter_any};

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

    /// The menu filter must find an app by its program name too, like the
    /// Run window does: "DreamOS Installer" runs `dreamos-install`, and typing
    /// the command name found nothing.
    #[test]
    fn filter_matches_the_program_name_and_keywords() {
        let extra = ["dreamos-install".to_string(), "catalog".to_string()];
        assert!(matches_filter_any("DreamOS Installer", &extra, "dreamos-install"));
        assert!(matches_filter_any("DreamOS Installer", &extra, "CATALOG"));
        assert!(matches_filter_any("DreamOS Installer", &extra, "installer"));
        assert!(!matches_filter_any("DreamOS Installer", &extra, "firefox"));
        assert!(matches_filter_any("DreamOS Installer", &[], ""));
    }
}
