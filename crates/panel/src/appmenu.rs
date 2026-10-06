//! The application menu: a `gtk::MenuButton` whose popover renders
//! `menu.json`'s tree of folders and apps, plus Settings / About / Logout /
//! Shutdown. `menu.json` is seeded (see `opusdm_dock::menu_defaults`) with
//! the installed apps grouped by freedesktop category on first run (or
//! whenever it is emptied), then edited with the Apps editor
//! (`opusdm-apps-editor`). The items are re-read every time the menu opens.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use crate::widgets;
use opusdm_dock::apps;
use opusdm_ipc::{MenuItem, PanelPosition};
use std::process::Command;

// Embedded at compile-time so the binary has zero runtime file dependencies.
const BALL_PNG: &[u8] = include_bytes!("../../../assets/ball.png");

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

    let menu = popover.clone();
    let info = entry.info.clone();
    btn.connect_clicked(move |_| {
        menu.popdown();
        apps::launch(&info);
    });
    attach_quick_apps_menu(&btn, id, &popover);
    Some(btn)
}

/// Right click on a menu row opens "Add to quick apps" (disabled when the app
/// is already one); `menu` is the application menu popover, closed on use.
fn attach_quick_apps_menu(btn: &gtk::Button, id: &str, menu: &gtk::Popover) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
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

/// The menu tree from `menu.json`, seeded from the installed apps grouped by
/// category when the file is missing or was emptied, so the panel and the
/// Apps editor always share one real file.
fn load_items() -> Vec<MenuItem> {
    match opusdm_ipc::menu::load_menu_config() {
        Some(cfg) if !cfg.items.is_empty() => cfg.items,
        _ => {
            let cfg = opusdm_ipc::MenuConfig { items: opusdm_dock::menu_defaults::default_menu_items() };
            if let Err(e) = opusdm_ipc::menu::save_menu_config(&cfg) {
                opusdm_ipc::err_log!("panel", "cannot write menu.json: {e}");
            }
            cfg.items
        }
    }
}

/// Replace the content of `custom` with the current menu tree.
fn fill_custom(custom: &gtk::Box, popover: &gtk::Popover) {
    while let Some(child) = custom.first_child() {
        custom.remove(&child);
    }
    build_custom_items(&load_items(), custom, popover);
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

    // The custom items sit in their own box, refilled from menu.json every
    // time the menu opens, so the Apps editor's changes show up at once.
    let custom = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.append(&custom);
    fill_custom(&custom, &popover);
    {
        let (custom, menu) = (custom.clone(), popover.clone());
        popover.connect_show(move |_| fill_custom(&custom, &menu));
    }

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
        opusdm_ipc::err_log!("opusdm-panel", "cannot run {bin}: {e}");
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
