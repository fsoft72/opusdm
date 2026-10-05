//! Desktop windows: one background window per monitor, the icon field on the
//! primary one, the wallpaper stylesheet, the desktop directory watch and the
//! hide / reveal state.

use crate::server::HubCmd;
use opusdm_ipc::bindings::ScriptEvent;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use super::toolbar::{request_quit, DesktopWindow, Widgets};
use super::{desktop_menu::*, groups_ui::*};

/// Foreground colour of the centred desktop placeholder label.
pub(crate) const DESKTOP_FG: &str = "#d3d7cf";

/// Background and border of the desktop right-click menu.
pub(crate) const DESKTOP_MENU_BG: &str = "#3c4043";

pub(crate) const DESKTOP_MENU_BORDER: &str = "#1b1d1e";

/// Delay before re-asserting a desktop window's geometry once, after the WM
/// has had a chance to run its placement policy (on first map) or after a
/// redundant `RaiseToolbar` finds a possibly stale window.
pub(crate) const WM_PLACEMENT_SETTLE_DELAY_MS: u64 = 80;

/// Delays for re-asserting a desktop window's geometry a couple of times
/// after a resolution change: the WM reacts to the new screen size on its
/// own (shifting the desktop down by the panel's strut), so this re-asserts
/// once it has settled.
pub(crate) const RESOLUTION_CHANGE_SETTLE_DELAYS_MS: [u64; 2] = [150, 600];

/// Re-place the desktop windows for a new monitor layout `rects`. Reuses the
/// existing windows (and the primary's icon field/watch) when the monitor
/// count hasn't changed - the common case, a plain resolution change - and
/// only tears everything down and rebuilds when monitors were added, removed,
/// or a mirror layout was toggled.
pub(crate) fn reconcile_desktop_windows(
    app: &gtk::Application,
    widgets: &Rc<Widgets>,
    mut rects: Vec<(i32, i32, i32, i32)>,
) {
    if rects.is_empty() {
        rects.push((0, 0, 1280, 800));
    }
    if widgets.windows.borrow().len() == rects.len() {
        reposition_desktop_windows(widgets, &rects);
    } else {
        full_rebuild_desktop_windows(app, widgets, rects);
    }
}

/// Re-pin every existing desktop window to its (possibly new) rect, matched
/// by index - index 0 stays the primary monitor's window.
pub(crate) fn reposition_desktop_windows(widgets: &Rc<Widgets>, rects: &[(i32, i32, i32, i32)]) {
    let mut windows = widgets.windows.borrow_mut();
    for (dw, &(x, y, w, h)) in windows.iter_mut().zip(rects.iter()) {
        crate::desktop::make_desktop_window(&dw.window, x, y, w, h);
        crate::desktop::pin_geometry(&dw.window, x, y, w, h);
        dw.x = x;
        dw.y = y;
        dw.w = w;
        dw.h = h;
        // The window manager reacts to the new screen size on its own - it
        // shifts the desktop down by the panel's strut - so re-assert the
        // geometry once it has settled, as the startup path does.
        let window = dw.window.clone();
        opusdm_ui::retry_after_map(&RESOLUTION_CHANGE_SETTLE_DELAYS_MS, move || {
            crate::desktop::pin_geometry(&window, x, y, w, h);
        });
    }
}

/// Tear down every desktop window (and the primary's icon field/watch) and
/// rebuild the whole set from `rects` - index 0 becomes the new primary.
pub(crate) fn full_rebuild_desktop_windows(
    app: &gtk::Application,
    widgets: &Rc<Widgets>,
    mut rects: Vec<(i32, i32, i32, i32)>,
) {
    for dw in widgets.windows.borrow_mut().drain(..) {
        dw.window.close();
    }
    *widgets.desktop.watch.borrow_mut() = None;
    *widgets.desktop.field.borrow_mut() = None;
    *widgets.desktop.dir.borrow_mut() = None;

    if rects.is_empty() {
        rects.push((0, 0, 1280, 800));
    }

    let icon_set = widgets.desktop.icon_set.borrow().clone();
    let desired_dir = widgets.desktop.desired_dir.borrow().clone();
    let built = rects
        .into_iter()
        .enumerate()
        .map(|(i, (x, y, w, h))| {
            let is_primary = i == 0;
            let window = build_one_desktop_window(
                app,
                widgets,
                x,
                y,
                w,
                h,
                is_primary,
                desired_dir.as_deref(),
                icon_set.clone(),
            );
            DesktopWindow { window, x, y, w, h }
        })
        .collect();
    *widgets.windows.borrow_mut() = built;
}

/// Build one desktop window at `(x, y)`, `w` x `h`. Only the primary monitor
/// (`is_primary`) gets the interactive icon field, its drag/drop wiring and
/// the inotify watch; the rest are background-only.
pub(crate) fn build_one_desktop_window(
    app: &gtk::Application,
    widgets: &Rc<Widgets>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    is_primary: bool,
    desktop_dir: Option<&Path>,
    icon_set: opusdm_ipc::IconSetId,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM")
        .decorated(false)
        // Resizable on the GTK side so the window follows the X geometry we
        // pin when the screen size changes; a non-resizable window would ask
        // the WM for its old natural size back and never shrink. The WM is
        // kept from resizing it by the min = max size hints in `desktop.rs`.
        .resizable(true)
        .default_width(w)
        .default_height(h)
        .build();

    // Turn the window into the WM desktop once its surface exists, then
    // re-assert the geometry after the WM has run its placement policy.
    window.connect_realize(move |win| crate::desktop::make_desktop_window(win, x, y, w, h));
    window.connect_map(move |win| {
        crate::desktop::make_desktop_window(win, x, y, w, h);
        let win = win.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(WM_PLACEMENT_SETTLE_DELAY_MS), move || {
            crate::desktop::pin_geometry(&win, x, y, w, h);
        });
    });

    let panel_position = widgets.desktop.settings.borrow().panel_position;
    let (desktop_area, field) = build_desktop_area(
        if is_primary { desktop_dir } else { None },
        icon_set,
        desktop_extras(widgets),
        panel_position,
        opusdm_hub::fullscreen::Rect { x, y, w, h },
    );
    window.set_child(Some(&desktop_area));

    // Right-click the desktop for the context menu (background, or - on the
    // primary monitor, where `field` is `Some` - a per-icon menu).
    attach_desktop_menu(&desktop_area, widgets, field.clone());

    // Window close (from the WM): always confirm before shutting down.
    {
        let widgets = widgets.clone();
        window.connect_close_request(move |_| {
            request_quit(&widgets);
            glib::Propagation::Stop
        });
    }

    if is_primary {
        *widgets.desktop.dir.borrow_mut() = desktop_dir.map(Path::to_path_buf);
        *widgets.desktop.field.borrow_mut() = field.clone();
        if let (Some(f), Some(dir)) = (field, desktop_dir) {
            let ds = widgets.desktop.settings.borrow();
            f.set_icon_size(ds.icon_size);
            f.set_grid_spacing(ds.grid_spacing);
            f.set_grid_gap(ds.grid_gap);
            drop(ds);
            wire_primary_icon_field(widgets, &f);
            spawn_desktop_watch(widgets, &f, dir);
        }
    }

    window.present();
    window
}

/// Wire the primary monitor's icon field: open on activate, copy/move on
/// drop, and spawn a new Lister on a background double-click.
pub(crate) fn wire_primary_icon_field(widgets: &Rc<Widgets>, field: &opusdm_iconview::IconField) {
    {
        let cmd_tx = widgets.cmd_tx.clone();
        let widgets = widgets.clone();
        let field_for_open = field.clone();
        field.connect_activate(move |i| {
            let Some(entry) = field_for_open.entry(i) else { return };
            if let Some(id) = opusdm_ipc::groups::group_id(&entry.path) {
                open_group(&widgets, id);
                return;
            }
            if entry.left_out && entry.kind == opusdm_iconview::IconKind::Broken {
                opusdm_ipc::err_log!("hub", "left-out target missing: {}", entry.path.display());
                return;
            }
            if opusdm_hub::leftout::opens_in_lister(&entry) {
                let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: Some(entry.path) });
            } else {
                let uri = format!("file://{}", entry.path.display());
                let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
            }
        });
    }
    {
        let cmd_tx = widgets.cmd_tx.clone();
        field.connect_background_activate(move || {
            let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: None });
            let _ = cmd_tx.send(HubCmd::EmitScriptEvent(ScriptEvent::DesktopDoubleClick));
        });
    }
    {
        let widgets = widgets.clone();
        let field_for_drop = field.clone();
        field.connect_drop_uris(move |srcs, target, shift| {
            if srcs.is_empty() {
                return;
            }
            // Dropped on a group icon: add the items to the group instead of
            // copying them into the desktop folder.
            let group_target = usize::try_from(target)
                .ok()
                .and_then(|i| field_for_drop.entry(i))
                .and_then(|e| opusdm_ipc::groups::group_id(&e.path).map(str::to_string));
            if let Some(id) = group_target {
                add_to_group(&widgets, &id, srcs);
                return;
            }
            let Some(dest_dir) = widgets.desktop.dir.borrow().clone() else { return };
            let op = if shift { opusdm_ipc::Op::Move } else { opusdm_ipc::Op::Copy };
            let _ = widgets.cmd_tx.send(HubCmd::RunJob { op, srcs, dest_dir });
        });
    }
    {
        let widgets = widgets.clone();
        field.connect_pinned_moved(move |moved| {
            let mut left_out_moved = false;
            let mut moved_groups = Vec::new();
            for (path, (x, y)) in &moved {
                match opusdm_ipc::groups::group_id(path) {
                    Some(id) => {
                        widgets.desktop.groups.borrow_mut().set_position(id, *x, *y);
                        moved_groups.push(id.to_string());
                    }
                    None => {
                        widgets.desktop.leftout.borrow_mut().set_position(path, *x, *y);
                        left_out_moved = true;
                    }
                }
            }
            if left_out_moved {
                save_leftout(&widgets.desktop.leftout.borrow());
            }
            for id in moved_groups {
                save_group(&widgets, &id);
            }
        });
    }
}

/// The full desktop stylesheet: one rule per background entry (the page
/// class from `background::page_class`), the fallback page rule, plus the
/// fixed context-menu rules.
pub(crate) fn desktop_rule_css(entries: &[opusdm_ipc::BackgroundEntry]) -> String {
    let pages: String = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| format!(".{} {{ {} }}\n", crate::background::page_class(i), opusdm_ipc::background_css(entry)))
        .collect();
    format!(
        "{pages}.{fallback} {{ {fallback_decl} }}\n\
         .opusdm-desktop-menu {{ background-color: {DESKTOP_MENU_BG}; color: {DESKTOP_FG}; \
             border: 1px solid {DESKTOP_MENU_BORDER}; border-radius: 4px; padding: 4px; }}\n\
         .opusdm-desktop-menu button {{ padding: 4px 12px; }}",
        fallback = crate::background::FALLBACK_CLASS,
        fallback_decl = opusdm_ipc::background_css(&opusdm_ipc::BackgroundEntry::default_solid()),
    )
}

thread_local! {
    /// The one desktop stylesheet provider, registered on the display once
    /// and reloaded in place on every theme change so providers never stack
    /// up. GTK runs single-threaded, so a thread-local suffices and avoids
    /// requiring `CssProvider: Sync`.
    static DESKTOP_CSS_PROVIDER: gtk::CssProvider = {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        provider
    };
}

/// Install (or replace) the desktop stylesheet for `entries`. Safe to call
/// repeatedly - it reloads the single shared provider's content rather than
/// registering a new one each time.
pub(crate) fn install_desktop_css(entries: &[opusdm_ipc::BackgroundEntry]) {
    DESKTOP_CSS_PROVIDER.with(|p| p.load_from_data(&desktop_rule_css(entries)));
}

/// Read `dir` into desktop entries, merged with the left-out icons in `left`,
/// on a background thread and invoke `on_done` with the result back on the
/// GTK main loop, so scanning a large or slow desktop directory (or stat-ing
/// a left-out target on a slow disk) never freezes the desktop and panel.
pub(crate) fn read_entries_async(
    dir: PathBuf,
    extras: opusdm_hub::leftout::DesktopExtras,
    on_done: impl Fn(std::io::Result<Vec<opusdm_iconview::IconEntry>>) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(opusdm_hub::leftout::read_desktop_entries(&dir, &extras));
    });
    glib::spawn_future_local(async move {
        if let Ok(result) = rx.recv().await {
            on_done(result);
        }
    });
}

/// Show `entries` in `field`, first handing it the saved positions of the
/// left-out icons so they land where the user dragged them.
pub(crate) fn show_entries(
    field: &opusdm_iconview::IconField,
    extras: &opusdm_hub::leftout::DesktopExtras,
    entries: &[opusdm_iconview::IconEntry],
) {
    field.set_pinned_positions(extras.positions());
    field.set_entries(entries);
}

/// Re-read the desktop directory and refresh the icon field with the current
/// left-out store merged in. Does nothing without a field or directory.
pub(crate) fn reload_desktop_field(widgets: &Rc<Widgets>) {
    let field = widgets.desktop.field.borrow().clone();
    let dir = widgets.desktop.dir.borrow().clone();
    let (Some(field), Some(dir)) = (field, dir) else { return };
    let ticket = widgets.desktop.reload_gate.begin();
    let extras = desktop_extras(widgets);
    let widgets = widgets.clone();
    read_entries_async(dir.clone(), extras, move |result| {
        if !widgets.desktop.reload_gate.is_latest(ticket) {
            return;
        }
        match result {
            // The store as it is now, not as it was when the read started: an
            // icon dragged in the meantime keeps its new spot.
            Ok(entries) => show_entries(&field, &desktop_extras(&widgets), &entries),
            Err(e) => opusdm_ipc::err_log!("hub", "cannot read desktop {}: {e}", dir.display()),
        }
    });
}

/// Text of the desktop menu's hide button for the current state.
pub(crate) fn hide_label(hidden: bool) -> &'static str {
    if hidden {
        "Reveal"
    } else {
        "Hide All"
    }
}

/// Hide (`true`) or show the panel, the dock and the group windows, and
/// relabel the desktop menu button. The desktop itself always stays, so it can
/// reveal the rest; the Lister windows react to their own `SetHidden`.
pub(crate) fn apply_hidden(widgets: &Rc<Widgets>, hidden: bool) {
    widgets.desktop.hidden.set(hidden);
    opusdm_panel::set_hidden(hidden);
    if let Some(app) = widgets.primary_window().application() {
        for window in app.windows() {
            let title = window.title();
            let is_chrome = matches!(
                title.as_deref(),
                Some(t) if t == opusdm_panel::WINDOW_TITLE || t == opusdm_dock::WINDOW_TITLE
            );
            if is_chrome {
                window.set_visible(!hidden);
            }
        }
    }
    widgets.desktop.group_windows.set_visible(!hidden);
    widgets.bank_windows.set_desktop_hidden(hidden);
    let mut buttons = widgets.desktop.hide_buttons.borrow_mut();
    buttons.retain(|b| b.root().is_some());
    for button in buttons.iter() {
        button.set_label(hide_label(hidden));
    }
}

/// Everything the desktop shows besides the Desktop folder, as it is now.
pub(crate) fn desktop_extras(widgets: &Widgets) -> opusdm_hub::leftout::DesktopExtras {
    opusdm_hub::leftout::DesktopExtras {
        left_out: widgets.desktop.leftout.borrow().clone(),
        groups: widgets.desktop.groups.borrow().clone(),
    }
}

/// Start a group member the way [`opusdm_hub::leftout::launch_action`] says.
/// Failures are logged.
pub(crate) fn launch_entry(widgets: &Rc<Widgets>, entry: &opusdm_iconview::IconEntry) {
    use opusdm_hub::leftout::LaunchAction;
    match opusdm_hub::leftout::launch_action(entry) {
        LaunchAction::Lister => {
            let _ = widgets.cmd_tx.send(HubCmd::SpawnRequested { start_path: Some(entry.path.clone()) });
        }
        LaunchAction::DesktopEntry => match gio::DesktopAppInfo::from_filename(&entry.path) {
            Some(info) => {
                if let Err(e) = info.launch(&[], gio::AppLaunchContext::NONE) {
                    opusdm_ipc::err_log!("hub", "cannot launch {}: {e}", entry.path.display());
                    show_group_error(widgets, &format!("Cannot launch {}: {e}", entry.path.display()));
                }
            }
            None => opusdm_ipc::err_log!("hub", "{} is not a valid .desktop file", entry.path.display()),
        },
        LaunchAction::Program => {
            let mut cmd = std::process::Command::new(&entry.path);
            if let Some(dir) = entry.path.parent() {
                cmd.current_dir(dir);
            }
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Err(e) = cmd.spawn() {
                opusdm_ipc::err_log!("hub", "cannot run {}: {e}", entry.path.display());
                show_group_error(widgets, &format!("Cannot run {}: {e}", entry.path.display()));
            }
        }
        LaunchAction::Default => {
            let uri = gio::File::for_path(&entry.path).uri();
            if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
                opusdm_ipc::err_log!("hub", "cannot open {}: {e}", entry.path.display());
                show_group_error(widgets, &format!("Cannot open {}: {e}", entry.path.display()));
            }
        }
    }
}

/// Persist the left-out store, logging (not failing) on error: the in-memory
/// state stays valid and the next change tries again.
pub(crate) fn save_leftout(left: &opusdm_ipc::LeftOut) {
    if let Err(e) = left.save() {
        opusdm_ipc::err_log!("hub", "cannot save {}: {e}", opusdm_ipc::leftout::leftout_path().display());
    }
}

/// Start (or restart) the inotify watch on `dir` and the local future that
/// reloads the desktop icon field when the directory changes. Replaces any
/// previous watch stored in `widgets.desktop.watch`.
pub(crate) fn spawn_desktop_watch(
    widgets: &Rc<Widgets>,
    field: &opusdm_iconview::IconField,
    dir: &Path,
) {
    // Drop the old watch first so its thread exits before we start another.
    *widgets.desktop.watch.borrow_mut() = None;
    match opusdm_hub::desktop_files::DesktopWatch::start(dir.to_path_buf()) {
        Ok((watch, rx)) => {
            let field = field.clone();
            let dir = dir.to_path_buf();
            let widgets_in = widgets.clone();
            glib::spawn_future_local(async move {
                let widgets = widgets_in;
                // Reloads run off-thread, so several can be in flight at
                // once; the shared reload gate drops any read superseded by a
                // later one (from here or from `reload_desktop_field`).
                while rx.recv().await.is_ok() {
                    let ticket = widgets.desktop.reload_gate.begin();
                    let field = field.clone();
                    let dir = dir.clone();
                    let extras = desktop_extras(&widgets);
                    let widgets = widgets.clone();
                    read_entries_async(dir.clone(), extras, move |result| {
                        if !widgets.desktop.reload_gate.is_latest(ticket) {
                            return;
                        }
                        match result {
                            Ok(entries) => show_entries(&field, &desktop_extras(&widgets), &entries),
                            Err(e) => {
                                opusdm_ipc::err_log!("hub", "desktop reload failed for {}: {e}", dir.display())
                            }
                        }
                    });
                }
            });
            *widgets.desktop.watch.borrow_mut() = Some(watch);
        }
        Err(e) => opusdm_ipc::err_log!("hub", "desktop watch unavailable: {e}"),
    }
}

/// Build the desktop area that fills the window below the toolbar, wrapped in
/// an overlay so the context menu can be laid on top of it. When `desktop_dir`
/// is `Some`, an icon field in `Free` mode fills it and its initial entries
/// are read. Otherwise the background stays plain. `monitor` is the screen
/// area the desktop window covers.
pub(crate) fn build_desktop_area(
    desktop_dir: Option<&Path>,
    icon_set: opusdm_ipc::IconSetId,
    extras: opusdm_hub::leftout::DesktopExtras,
    panel_position: opusdm_ipc::PanelPosition,
    monitor: opusdm_hub::fullscreen::Rect,
) -> (gtk::Overlay, Option<opusdm_iconview::IconField>) {
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bg.add_css_class("opusdm-desktop");
    bg.set_hexpand(true);
    bg.set_vexpand(true);

    let field = match desktop_dir {
        Some(dir) => {
            let f = opusdm_iconview::IconField::new();
            f.set_mode(opusdm_iconview::LayoutMode::Free);
            f.set_icon_set(icon_set);
            {
                let f = f.clone();
                let dir = dir.to_path_buf();
                let extras_for_show = extras.clone();
                read_entries_async(dir.clone(), extras, move |result| match result {
                    Ok(entries) => show_entries(&f, &extras_for_show, &entries),
                    Err(e) => opusdm_ipc::err_log!("hub", "cannot read desktop {}: {e}", dir.display()),
                });
            }
            f.widget().set_hexpand(true);
            f.widget().set_vexpand(true);
            keep_clear_of_panel(f.widget(), panel_position);
            bg.append(f.widget());
            Some(f)
        }
        None => None,
    };

    // The rotating background sits underneath; the icon area floats over it.
    overlay.set_child(Some(&crate::background::new_view(monitor)));
    overlay.add_overlay(&bg);
    (overlay, field)
}

/// The dock layout `ds` asks for: its own edge and icon size, plus the
/// panel's edge and height so it can keep clear of the panel.
pub(crate) fn dock_layout(ds: &opusdm_ipc::DesktopSettings) -> opusdm_dock::DockLayout {
    opusdm_dock::DockLayout {
        position: ds.dock_position,
        icon_size: ds.dock_icon_size,
        panel: ds.panel_position,
        panel_height: opusdm_panel::PANEL_HEIGHT,
    }
}

/// Keep the icon area clear of the panel dock: the background box still
/// fills the whole monitor, but the icons must stay inside the strip the
/// panel's strut leaves free instead of sitting under it. The margin goes on
/// the edge the panel is docked to and is cleared on the other.
pub(crate) fn keep_clear_of_panel(field: &impl IsA<gtk::Widget>, position: opusdm_ipc::PanelPosition) {
    let (top, bottom) = match position {
        opusdm_ipc::PanelPosition::Top => (opusdm_panel::PANEL_HEIGHT, 0),
        opusdm_ipc::PanelPosition::Bottom => (0, opusdm_panel::PANEL_HEIGHT),
    };
    field.set_margin_top(top);
    field.set_margin_bottom(bottom);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_rule_css_has_a_rule_per_entry() {
        let entries = vec![
            opusdm_ipc::BackgroundEntry::default_gradient(),
            opusdm_ipc::BackgroundEntry::Video { path: "/x/y.mp4".into(), fit: opusdm_ipc::ImageFit::Fill },
        ];
        let css = desktop_rule_css(&entries);
        assert!(css.contains(".opusdm-bg-0 { background: linear-gradient(180deg,"));
        assert!(css.contains(".opusdm-bg-1 { background: #000000; }"));
        assert!(css.contains(".opusdm-bg-fallback {"));
        assert!(css.contains(".opusdm-desktop-menu {")); // other rules kept
    }

    /// Parse the generated desktop stylesheet with the real GTK CSS parser for
    /// every entry kind. Self-skips when no display is available.
    #[test]
    fn desktop_rule_css_parses_without_error() {
        if gtk::init().is_err() {
            return;
        }
        let provider = gtk::CssProvider::new();
        let errors = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        {
            let errors = errors.clone();
            provider.connect_parsing_error(move |_, section, err| {
                errors.borrow_mut().push(format!("{section}: {err}"));
            });
        }
        let path = std::path::PathBuf::from("/tmp/opusdm-test bg.png");
        let entries = vec![
            opusdm_ipc::BackgroundEntry::default_solid(),
            opusdm_ipc::BackgroundEntry::default_gradient(),
            opusdm_ipc::BackgroundEntry::Image { path: path.clone(), fit: opusdm_ipc::ImageFit::Tile },
            opusdm_ipc::BackgroundEntry::Video { path, fit: opusdm_ipc::ImageFit::Fit },
        ];
        provider.load_from_data(&desktop_rule_css(&entries));
        assert!(errors.borrow().is_empty(), "{:?}", errors.borrow());
    }
}
