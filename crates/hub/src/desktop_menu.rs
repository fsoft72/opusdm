//! The desktop right-click menus (background and icon), the double click
//! script events and the About dialog.

use crate::server::HubCmd;
use opusdm_ipc::bindings::ScriptEvent;
use gtk4 as gtk;
use gtk::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;
use super::toolbar::{request_quit, Widgets};
use super::{desktop_windows::*, groups_ui::*};

/// Approximate menu width/height, reserved when clamping the menu inside the
/// desktop so a right-click near an edge still shows the whole menu.
pub(crate) const DESKTOP_MENU_W: i32 = 180;

pub(crate) const DESKTOP_MENU_H: i32 = 300;

/// Resolve the `opus-desktop-config` binary: preferably a sibling of the
/// running hub executable (so a `cargo run` / installed pair stay together),
/// otherwise the bare name for a `PATH` lookup.
/// Resolve the `opusdm-settings` binary: preferably a sibling of the running
/// hub executable (so a `cargo run` / installed pair stay together),
/// otherwise the bare name for a `PATH` lookup.
pub(crate) fn theme_config_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("opusdm-settings");
        }
    }
    PathBuf::from("opusdm-settings")
}

/// Spawn the Lister binary (next to the hub executable, else on `PATH`) with
/// only the File Types editor open.
pub(crate) fn launch_filetypes_editor() {
    spawn_lister_tool("--edit-filetypes");
}

/// Spawn the Lister binary with only the Manage Functions window open.
pub(crate) fn launch_functions_manager() {
    spawn_lister_tool("--manage-functions");
}

/// Spawn the Lister binary with only the Edit Hotkeys window open.
pub(crate) fn launch_hotkey_editor() {
    spawn_lister_tool("--edit-hotkeys");
}

/// Spawn the Lister binary (next to the hub executable, else on `PATH`) with
/// `flag`, which makes it open one tool window instead of a file window.
fn spawn_lister_tool(flag: &str) {
    let bin = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("opusdm-lister")))
        .unwrap_or_else(|| PathBuf::from("opusdm-lister"));
    if let Err(e) = std::process::Command::new(&bin).arg(flag).spawn() {
        opusdm_ipc::err_log!("hub", "cannot launch {}: {e}", bin.display());
    }
}

/// Spawn the theme configuration app, detached.
pub(crate) fn launch_theme_config() {
    spawn_theme_config(&[]);
}

/// Spawn the theme configuration app with the Theme Editor open on top.
pub(crate) fn launch_theme_editor() {
    spawn_theme_config(&["--edit-theme"]);
}

/// Spawn the theme configuration app with `args`, detached.
fn spawn_theme_config(args: &[&str]) {
    let bin = theme_config_bin();
    if let Err(e) = std::process::Command::new(&bin).args(args).spawn() {
        opusdm_ipc::err_log!("hub", "cannot launch {}: {e}", bin.display());
    }
}

/// Approximate icon menu width/height, reserved when clamping it inside the
/// desktop the same way as [`DESKTOP_MENU_W`]/[`DESKTOP_MENU_H`].
pub(crate) const ICON_MENU_W: i32 = 160;

pub(crate) const ICON_MENU_H: i32 = 140;

/// Natural size of `menu` without its margins. GTK4 measures a widget
/// including its margins, and here the margins carry the click position, so
/// they must be taken out or the menu would be pushed too far up and left.
fn content_size(menu: &gtk::Box) -> (i32, i32) {
    let (_, natural) = menu.preferred_size();
    let w = natural.width() - menu.margin_start() - menu.margin_end();
    let h = natural.height() - menu.margin_top() - menu.margin_bottom();
    (w.max(0), h.max(0))
}

/// Place `menu` at `(x, y)` inside `overlay`, clamped so it never runs past
/// the overlay's edge, and show it. `w`/`h` are fallbacks, used only when the
/// menu cannot report its own natural size.
pub(crate) fn popup_at(menu: &gtk::Box, overlay: &gtk::Overlay, x: f64, y: f64, w: i32, h: i32) {
    let (mw, mh) = content_size(menu);
    let (mw, mh) = (mw.max(w), mh.max(h));
    let max_x = (overlay.width() - mw).max(0);
    let max_y = (overlay.height() - mh).max(0);
    menu.set_margin_start((x as i32).clamp(0, max_x));
    menu.set_margin_top((y as i32).clamp(0, max_y));
    menu.set_visible(true);
}

/// Re-clamp an already placed `menu` so it fits inside `overlay` using its
/// current natural size. Called after the menu grows (e.g. the Desktop
/// expander opens) so it never runs off the bottom/right edge of the screen.
pub(crate) fn keep_menu_on_screen(menu: &gtk::Box, overlay: &gtk::Overlay) {
    let (mw, mh) = content_size(menu);
    let max_x = (overlay.width() - mw).max(0);
    let max_y = (overlay.height() - mh).max(0);
    menu.set_margin_start(menu.margin_start().clamp(0, max_x));
    menu.set_margin_top(menu.margin_top().clamp(0, max_y));
}

/// Emit `DesktopMiddleDoubleClick` and `DesktopRightDoubleClick` for double
/// clicks on the desktop overlay. The gestures run in the capture phase and
/// never claim the sequence, so the icon field and the context menu keep
/// working. The right button already opens the context menu on its first
/// press, so a right double click also shows the menu (accepted).
pub(crate) fn attach_desktop_double_click_events(overlay: &gtk::Overlay, cmd_tx: &std::sync::mpsc::Sender<HubCmd>) {
    for (button, event) in [
        (gtk::gdk::BUTTON_MIDDLE, ScriptEvent::DesktopMiddleDoubleClick),
        (gtk::gdk::BUTTON_SECONDARY, ScriptEvent::DesktopRightDoubleClick),
    ] {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(button);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let cmd_tx = cmd_tx.clone();
        gesture.connect_pressed(move |_, n_press, _, _| {
            if n_press == 2 {
                let _ = cmd_tx.send(HubCmd::EmitScriptEvent(event));
            }
        });
        overlay.add_controller(gesture);
    }
}

/// Wire the right-click context menus onto the desktop overlay: the
/// background menu (System actions, New Folder/File, Paste, Terminal), and -
/// when `field` is `Some` (the primary monitor only) - a per-icon menu with
/// the same file operations a Lister offers on a row (Open, Cut, Copy,
/// Rename, Delete). The desktop window never holds the input focus, so a
/// `Popover` parented to it can never grab; both menus are plain boxes laid
/// on the overlay instead, positioned with margins at the click point.
pub(crate) fn attach_desktop_menu(overlay: &gtk::Overlay, widgets: &Rc<Widgets>, field: Option<opusdm_iconview::IconField>) {
    attach_desktop_double_click_events(overlay, &widgets.cmd_tx);
    let bg_menu = build_desktop_menu(widgets);
    bg_menu.set_halign(gtk::Align::Start);
    bg_menu.set_valign(gtk::Align::Start);
    bg_menu.set_visible(false);
    overlay.add_overlay(&bg_menu);

    let icon_menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    icon_menu.add_css_class("opusdm-desktop-menu");
    icon_menu.set_halign(gtk::Align::Start);
    icon_menu.set_valign(gtk::Align::Start);
    icon_menu.set_visible(false);
    overlay.add_overlay(&icon_menu);

    match field {
        // Primary monitor: the field's own secondary-click controller already
        // hit-tests icons vs. background (see `IconField::connect_context_menu`),
        // so route through it instead of a second overlay-level gesture.
        Some(field) => {
            let widgets = widgets.clone();
            let overlay = overlay.clone();
            let bg_menu = bg_menu.clone();
            let icon_menu = icon_menu.clone();
            let field_for_cb = field.clone();
            field.connect_context_menu(move |idx, x, y| {
                let field = &field_for_cb;
                let (x, y) = field.translate_from_canvas(&overlay, x, y).unwrap_or((x, y));
                if idx >= 0 {
                    let Some(entry) = field.entry(idx as usize) else { return };
                    fill_icon_menu(&icon_menu, &widgets, &entry);
                    bg_menu.set_visible(false);
                    popup_at(&icon_menu, &overlay, x, y, ICON_MENU_W, ICON_MENU_H);
                } else {
                    icon_menu.set_visible(false);
                    popup_at(&bg_menu, &overlay, x, y, DESKTOP_MENU_W, DESKTOP_MENU_H);
                }
            });
        }
        // No icon field on this monitor (secondary monitor, or no desktop
        // directory resolved): background menu only.
        None => {
            let open = gtk::GestureClick::new();
            open.set_button(gtk::gdk::BUTTON_SECONDARY);
            {
                let bg_menu = bg_menu.clone();
                let overlay = overlay.clone();
                open.connect_pressed(move |gesture, n_press, x, y| {
                    if n_press != 1 {
                        return;
                    }
                    popup_at(&bg_menu, &overlay, x, y, DESKTOP_MENU_W, DESKTOP_MENU_H);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                });
            }
            overlay.add_controller(open);
        }
    }

    // Primary click outside whichever menu is open: dismiss it. Clicks on the
    // menu buttons are handled by the buttons themselves.
    let dismiss = gtk::GestureClick::new();
    dismiss.set_button(gtk::gdk::BUTTON_PRIMARY);
    {
        let overlay = overlay.clone();
        dismiss.connect_pressed(move |_, _, x, y| {
            for menu in [&bg_menu, &icon_menu] {
                if !menu.get_visible() {
                    continue;
                }
                if let Some(b) = menu.compute_bounds(&overlay) {
                    let (x, y) = (x as f32, y as f32);
                    let inside = x >= b.x()
                        && x < b.x() + b.width()
                        && y >= b.y()
                        && y < b.y() + b.height();
                    if inside {
                        continue;
                    }
                }
                menu.set_visible(false);
            }
        });
    }
    overlay.add_controller(dismiss);
}

/// The per-icon context menu, rebuilt into `menu` on every click since its
/// content (name, file vs. directory) depends on the clicked entry: Open,
/// Cut, Copy, Rename, Delete. Every item hides the menu before running its
/// action.
pub(crate) fn fill_icon_menu(menu: &gtk::Box, widgets: &Rc<Widgets>, entry: &opusdm_iconview::IconEntry) {
    while let Some(child) = menu.first_child() {
        menu.remove(&child);
    }

    if entry.group {
        fill_group_menu(menu, widgets, entry);
        return;
    }

    // Open
    {
        let path = entry.path.clone();
        let opens_in_lister = opusdm_hub::leftout::opens_in_lister(entry);
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Open");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            if opens_in_lister {
                let _ = widgets.cmd_tx.send(HubCmd::SpawnRequested { start_path: Some(path.clone()) });
            } else {
                crate::desktop_windows::open_file(&widgets.cmd_tx, &path);
            }
        });
        menu.append(&btn);
    }

    // Put Away (left-out icons only): forget the icon, never touch the file.
    if entry.left_out {
        let path = entry.path.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Put Away");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            {
                let mut left = widgets.desktop.leftout.borrow_mut();
                left.remove(std::slice::from_ref(&path));
                save_leftout(&left);
            }
            reload_desktop_field(&widgets);
        });
        menu.append(&btn);
    }

    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Cut, Rename and Delete act on the real file, which for a left-out icon
    // lives somewhere else and is not what the user sees: not offered there.
    // Cut
    if !entry.left_out {
        let path = entry.path.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Cut");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            crate::desktop_ops::copy_or_cut(std::slice::from_ref(&path), true);
        });
        menu.append(&btn);
    }

    // Copy
    {
        let path = entry.path.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Copy");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            crate::desktop_ops::copy_or_cut(std::slice::from_ref(&path), false);
        });
        menu.append(&btn);
    }

    // Rename...
    if !entry.left_out {
        let path = entry.path.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Rename...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            crate::desktop_ops::rename(&widgets.primary_window(), path.clone(), &name);
        });
        menu.append(&btn);
    }

    // Delete...
    if !entry.left_out {
        menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let path = entry.path.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Delete...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let Some(dest_dir) = widgets.desktop.dir.borrow().clone() else { return };
            crate::desktop_ops::confirm_delete(
                &widgets.primary_window(),
                &widgets.cmd_tx,
                &format!("Delete \"{name}\" permanently?"),
                vec![path.clone()],
                dest_dir,
            );
        });
        menu.append(&btn);
    }
}

/// Flat, left-aligned menu button, matching the Lister context menu look.
pub(crate) fn menu_button(title: &str) -> gtk::Button {
    let btn = gtk::Button::with_label(title);
    btn.add_css_class("flat");
    btn.set_halign(gtk::Align::Fill);
    if let Some(child) = btn.child().and_downcast::<gtk::Label>() {
        child.set_xalign(0.0);
    }
    btn
}

/// Append a flat button titled `title` to `parent`. Clicking it hides `menu`
/// first, then runs `action`.
fn add_item(parent: &gtk::Box, menu: &gtk::Box, title: &str, action: impl Fn() + 'static) -> gtk::Button {
    let menu_ref = menu.clone();
    let btn = menu_button(title);
    btn.connect_clicked(move |_| {
        menu_ref.set_visible(false);
        action();
    });
    parent.append(&btn);
    btn
}

/// Run `action` with the desktop folder; nothing happens while there is none.
fn with_desktop_dir(widgets: &Rc<Widgets>, action: impl FnOnce(PathBuf)) {
    let Some(dir) = widgets.desktop.dir.borrow().clone() else { return };
    action(dir);
}

/// Start the sibling tool `bin` and log a failure to start it.
fn launch_tool(bin: &str) {
    if let Err(e) = opusdm_hub::children::spawn_tool(bin) {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start {bin}: {e}");
    }
}

/// The top part of the menu: New Lister, Paste, New Folder, New File, Hide
/// All, New Group, Button Banks and Open Terminal Here.
fn add_file_items(menu: &gtk::Box, widgets: &Rc<Widgets>) {
    {
        let widgets = widgets.clone();
        add_item(menu, menu, "New Lister", move || {
            let _ = widgets.cmd_tx.send(HubCmd::SpawnRequested { start_path: None });
        });
    }

    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Paste: whatever a Cut/Copy (here or in a Lister) last put on the
    // clipboard, into the desktop directory. A no-op if there is nothing
    // pasteable, so it is always shown rather than probed asynchronously.
    {
        let widgets = widgets.clone();
        add_item(menu, menu, "Paste", move || {
            with_desktop_dir(&widgets, |dest_dir| crate::desktop_ops::paste(&widgets.cmd_tx, dest_dir));
        });
    }

    {
        let widgets = widgets.clone();
        add_item(menu, menu, "New Folder...", move || {
            with_desktop_dir(&widgets, |dest_dir| {
                crate::desktop_ops::prompt_new(
                    &widgets.primary_window(),
                    "New folder name",
                    &widgets.cmd_tx,
                    dest_dir,
                    |name| opusdm_ipc::Op::MakeDir { name },
                );
            });
        });
    }

    {
        let widgets = widgets.clone();
        add_item(menu, menu, "New File...", move || {
            with_desktop_dir(&widgets, |dest_dir| {
                crate::desktop_ops::prompt_new(
                    &widgets.primary_window(),
                    "New file name",
                    &widgets.cmd_tx,
                    dest_dir,
                    |name| opusdm_ipc::Op::NewFile { name },
                );
            });
        });
    }

    // Hide All / Reveal: the label follows the state, so the button is kept.
    {
        let cmd_tx = widgets.cmd_tx.clone();
        let btn = add_item(menu, menu, hide_label(widgets.desktop.hidden.get()), move || {
            let _ = cmd_tx.send(HubCmd::ToggleHidden);
        });
        widgets.desktop.hide_buttons.borrow_mut().push(btn);
    }

    {
        let widgets = widgets.clone();
        add_item(menu, menu, "New Group...", move || new_group_dialog(&widgets));
    }

    {
        let widgets = widgets.clone();
        add_item(menu, menu, "Button Banks...", move || open_bank_manager(&widgets));
    }

    {
        let widgets = widgets.clone();
        add_item(menu, menu, "Open Terminal Here", move || {
            with_desktop_dir(&widgets, |dir| crate::desktop_ops::open_terminal_in(&dir));
        });
    }
}

/// The "Desktop" expander: an expander so the hand-rolled flyout can still
/// nest without a real GtkPopoverMenu (the desktop window cannot grab focus).
fn build_desktop_submenu(menu: &gtk::Box, widgets: &Rc<Widgets>) -> gtk::Expander {
    let sub = gtk::Box::new(gtk::Orientation::Vertical, 2);
    sub.set_margin_start(12);

    // Rearrange icons: re-read the desktop, sort dirs-first by name, and
    // hand the sorted list back so the field auto-slots it into the grid
    // (set_entries clears any manual positions).
    {
        let widgets = widgets.clone();
        add_item(&sub, menu, "Rearrange icons", move || reload_desktop_field(&widgets));
    }
    add_item(&sub, menu, "Edit Hotkeys", launch_hotkey_editor);
    add_item(&sub, menu, "Manage Functions", launch_functions_manager);
    add_item(&sub, menu, "Edit File Types", launch_filetypes_editor);
    add_item(&sub, menu, "Edit Apps", || launch_tool("opusdm-apps-editor"));
    add_item(&sub, menu, "Edit Theme", launch_theme_editor);
    // Settings: the standalone opusdm-settings app. Icon grid metrics and the
    // desktop folder override live there too.
    add_item(&sub, menu, "Settings", launch_theme_config);
    // Input: mouse, touchpad and keyboard settings.
    add_item(&sub, menu, "Input", || launch_tool("opusdm-input-config"));
    // Screens: monitor layout, resolution, mirroring.
    add_item(&sub, menu, "Screens", || launch_tool("opusdm-screen"));

    let expander = gtk::Expander::new(Some("Desktop"));
    expander.set_child(Some(&sub));
    // Opening the expander grows the menu: pull it back on screen.
    let menu_ref = menu.clone();
    expander.connect_expanded_notify(move |_| {
        let menu = menu_ref.clone();
        gtk::glib::idle_add_local_once(move || {
            let Some(overlay) = menu.parent().and_then(|p| p.downcast::<gtk::Overlay>().ok()) else { return };
            keep_menu_on_screen(&menu, &overlay);
        });
    });
    expander
}

/// The desktop context menu box: New Lister / --- / Paste, New Folder, New
/// File, Open Terminal Here / --- / About / --- / Desktop submenu / --- /
/// Quit. Every item hides the menu before running its action.
pub(crate) fn build_desktop_menu(widgets: &Rc<Widgets>) -> gtk::Box {
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.add_css_class("opusdm-desktop-menu");

    add_file_items(&menu, widgets);
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    {
        let widgets = widgets.clone();
        add_item(&menu, &menu, "About", move || show_about_dialog(&widgets.primary_window()));
    }
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    menu.append(&build_desktop_submenu(&menu, widgets));
    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Quit always asks for confirmation before closing everything.
    {
        let widgets = widgets.clone();
        add_item(&menu, &menu, "Quit", move || request_quit(&widgets));
    }

    menu
}

pub(crate) fn show_about_dialog(parent: &gtk::ApplicationWindow) {
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
