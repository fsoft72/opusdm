//! One terminal pane: a VTE widget running the user's shell (or a command)
//! next to its scrollbar, with link recognition, a context menu and drop
//! support for files dragged from a Lister or the desktop.

use crate::actions::Action;
use crate::config::{vte_scrollback, CursorShape, TerminalConfig};
use crate::links;
use crate::shared::Shared;
use crate::theme::{parse_hex, TerminalTheme};
use gtk4 as gtk;
use gtk::{gdk, gio, glib, pango};
use std::cell::RefCell;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use vte4::prelude::*;

/// CSS class of the box holding a terminal and its scrollbar.
pub const PANE_CLASS: &str = "term-pane";

/// CSS class added to the focused pane when its tab holds more than one.
pub const FOCUSED_CLASS: &str = "focused";

/// Name of the action group of a pane's context menu.
const ACTION_GROUP: &str = "term";

/// Zoom step applied by Zoom In / Zoom Out.
const ZOOM_STEP: f64 = 1.1;

/// Smallest and largest zoom factor.
const ZOOM_RANGE: (f64, f64) = (0.25, 4.0);

/// Create a terminal running `command` (the user's shell when `None`) in
/// `cwd` (the process directory when `None`), wrapped in its pane box. The
/// pane box is the terminal's parent.
pub fn new_terminal(shared: &Rc<Shared>, cwd: Option<&Path>, command: Option<&[String]>) -> vte4::Terminal {
    let term = vte4::Terminal::new();
    term.set_hexpand(true);
    term.set_vexpand(true);
    term.set_allow_hyperlink(true);
    term.set_mouse_autohide(true);
    apply_config(&term, &shared.config(), &shared.theme());

    let pane = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    pane.add_css_class(PANE_CLASS);
    let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, term.vadjustment().as_ref());
    pane.append(&term);
    pane.append(&scrollbar);

    attach_links(&term);
    let hovered_link = Rc::new(RefCell::new(None::<String>));
    attach_click(&term, shared, &hovered_link);
    attach_actions(&term, &pane, shared, &hovered_link);
    attach_drop(&term);
    attach_focus(&term, &pane, shared);

    {
        let shared = shared.clone();
        term.connect_window_title_changed(move |t| crate::window::refresh_titles(&shared, t));
    }
    {
        let shared = shared.clone();
        term.connect_child_exited(move |t, _| crate::layout::close_pane(&shared, t));
    }

    let argv = command.map(<[String]>::to_vec).unwrap_or_else(|| vec![detect_user_shell()]);
    spawn(&term, cwd, &argv);
    term
}

/// Apply the font, scrollback, cursor and colours of `config` and `theme`.
pub fn apply_config(term: &vte4::Terminal, config: &TerminalConfig, theme: &TerminalTheme) {
    term.set_font(Some(&pango::FontDescription::from_string(&config.font)));
    term.set_scrollback_lines(vte_scrollback(config.scrollback_lines) as _);
    term.set_cursor_shape(match config.cursor_shape {
        CursorShape::Block => vte4::CursorShape::Block,
        CursorShape::IBeam => vte4::CursorShape::Ibeam,
        CursorShape::Underline => vte4::CursorShape::Underline,
    });
    term.set_cursor_blink_mode(if config.cursor_blink { vte4::CursorBlinkMode::On } else { vte4::CursorBlinkMode::Off });
    term.set_audible_bell(config.audible_bell);
    term.set_bold_is_bright(config.bold_is_bright);
    apply_theme(term, theme);
}

/// Paint `term` with the colours of `theme`.
fn apply_theme(term: &vte4::Terminal, theme: &TerminalTheme) {
    let rgba = |hex: &str| parse_hex(hex).map(|c| gdk::RGBA::new(c.red, c.green, c.blue, 1.0));
    let optional = |hex: &Option<String>| hex.as_deref().and_then(rgba);
    let palette: Vec<gdk::RGBA> = theme.palette.iter().filter_map(|c| rgba(c)).collect();
    let palette_refs: Vec<&gdk::RGBA> = palette.iter().collect();
    term.set_colors(rgba(&theme.foreground).as_ref(), rgba(&theme.background).as_ref(), &palette_refs);
    term.set_color_cursor(optional(&theme.cursor).as_ref());
    term.set_color_cursor_foreground(optional(&theme.cursor_foreground).as_ref());
    term.set_color_highlight(optional(&theme.selection).as_ref());
    term.set_color_highlight_foreground(optional(&theme.selection_foreground).as_ref());
}

/// Start `argv` in `term`. A failure is logged and written into the terminal.
fn spawn(term: &vte4::Terminal, cwd: Option<&Path>, argv: &[String]) {
    let cwd = cwd.map(|p| p.to_string_lossy().into_owned());
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let program = argv.first().cloned().unwrap_or_default();
    let failed_term = term.clone();
    term.spawn_async(
        vte4::PtyFlags::DEFAULT,
        cwd.as_deref(),
        &argv_refs,
        &[],
        glib::SpawnFlags::SEARCH_PATH,
        || {},
        -1,
        None::<&gio::Cancellable>,
        move |result| {
            let Err(e) = result else { return };
            opusdm_ipc::err_log!("opusdm-terminal", "cannot start {program}: {e}");
            failed_term.feed(format!("opusdm-terminal: cannot start {program}: {e}\r\n").as_bytes());
        },
    );
}

/// Register the link patterns, shown with a hand cursor under the pointer.
fn attach_links(term: &vte4::Terminal) {
    for pattern in links::PATTERNS {
        match vte4::Regex::for_match(pattern, links::PCRE2_MULTILINE | links::PCRE2_CASELESS) {
            Ok(regex) => {
                let tag = term.match_add_regex(&regex, 0);
                term.match_set_cursor_name(tag, "pointer");
            }
            Err(e) => opusdm_ipc::err_log!("opusdm-terminal", "bad link pattern {pattern:?}: {e}"),
        }
    }
}

/// The link (OSC 8 hyperlink or recognised URL) at `(x, y)`, as a URI.
fn link_at(term: &vte4::Terminal, x: f64, y: f64) -> Option<String> {
    if let Some(uri) = term.check_hyperlink_at(x, y) { return Some(uri.to_string()); }
    term.check_match_at(x, y).0.map(|text| links::link_to_uri(&text))
}

/// Open `uri` with the default application.
pub fn open_uri(term: &vte4::Terminal, uri: &str) {
    let window = term.root().and_downcast::<gtk::Window>();
    let uri_owned = uri.to_string();
    gtk::UriLauncher::new(uri).launch(window.as_ref(), None::<&gio::Cancellable>, move |result| {
        if let Err(e) = result {
            opusdm_ipc::err_log!("opusdm-terminal", "cannot open {uri_owned}: {e}");
        }
    });
}

/// Ctrl+click opens the link under the pointer; a right click opens the
/// context menu (with the link actions when there is one).
fn attach_click(term: &vte4::Terminal, shared: &Rc<Shared>, hovered_link: &Rc<RefCell<Option<String>>>) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(0);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let term_weak = term.downgrade();
    let shared = shared.clone();
    let hovered_link = hovered_link.clone();
    gesture.connect_pressed(move |g, _, x, y| {
        let Some(term) = term_weak.upgrade() else { return };
        let ctrl = g.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
        match g.current_button() {
            gdk::BUTTON_PRIMARY if ctrl => {
                let Some(uri) = link_at(&term, x, y) else { return };
                g.set_state(gtk::EventSequenceState::Claimed);
                open_uri(&term, &uri);
            }
            gdk::BUTTON_SECONDARY => {
                g.set_state(gtk::EventSequenceState::Claimed);
                *hovered_link.borrow_mut() = link_at(&term, x, y);
                term.grab_focus();
                show_context_menu(&shared, &term, hovered_link.borrow().is_some(), x, y);
            }
            _ => {}
        }
    });
    term.add_controller(gesture);
}

/// A menu entry running the pane action `name`, showing `accel` when set.
fn menu_item(label: &str, name: &str, accel: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(&format!("{ACTION_GROUP}.{name}")));
    if !accel.is_empty() {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

/// Pop up the context menu of `term` at `(x, y)`.
fn show_context_menu(shared: &Rc<Shared>, term: &vte4::Terminal, has_link: bool, x: f64, y: f64) {
    let Some(pane) = term.parent() else { return };
    let config = shared.config();
    let section = |actions: &[Action]| {
        let menu = gio::Menu::new();
        for &a in actions {
            menu.append_item(&menu_item(a.label(), a.id(), &config.hotkey(a)));
        }
        menu
    };
    let menu = gio::Menu::new();
    if has_link {
        let link = gio::Menu::new();
        link.append_item(&menu_item("Open Link", "open-link", ""));
        link.append_item(&menu_item("Copy Link", "copy-link", ""));
        menu.append_section(None, &link);
    }
    menu.append_section(None, &section(&[Action::Copy, Action::Paste, Action::SelectAll]));
    menu.append_section(None, &section(&[Action::SplitHorizontal, Action::SplitVertical, Action::NewTab, Action::NewWindow]));
    menu.append_section(None, &section(&[Action::ZoomIn, Action::ZoomOut, Action::ZoomReset]));
    menu.append_section(None, &section(&[Action::Export, Action::ResetClear]));
    // A section of its own: a separator line above and below Preferences.
    menu.append_section(None, &section(&[Action::Preferences]));
    menu.append_section(None, &section(&[Action::ClosePane]));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(&pane);
    popover.set_has_arrow(false);
    let point = term.compute_point(&pane, &gtk::graphene::Point::new(x as f32, y as f32));
    let (px, py) = point.map_or((x as i32, y as i32), |p| (p.x() as i32, p.y() as i32));
    popover.set_pointing_to(Some(&gdk::Rectangle::new(px, py, 1, 1)));
    // Unparent after the menu action ran: the closed signal comes first.
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}

/// Install the context menu actions on `pane`.
fn attach_actions(term: &vte4::Terminal, pane: &gtk::Box, shared: &Rc<Shared>, hovered_link: &Rc<RefCell<Option<String>>>) {
    let group = gio::SimpleActionGroup::new();
    for action in Action::ALL {
        let simple = gio::SimpleAction::new(action.id(), None);
        let term_weak = term.downgrade();
        let shared = shared.clone();
        simple.connect_activate(move |_, _| {
            let Some(term) = term_weak.upgrade() else { return };
            crate::window::run_action(&shared, action, &term);
        });
        group.add_action(&simple);
    }
    let open = gio::SimpleAction::new("open-link", None);
    {
        let term_weak = term.downgrade();
        let link = hovered_link.clone();
        open.connect_activate(move |_, _| {
            let (Some(term), Some(uri)) = (term_weak.upgrade(), link.borrow().clone()) else { return };
            open_uri(&term, &uri);
        });
    }
    let copy = gio::SimpleAction::new("copy-link", None);
    {
        let term_weak = term.downgrade();
        let link = hovered_link.clone();
        copy.connect_activate(move |_, _| {
            let (Some(term), Some(uri)) = (term_weak.upgrade(), link.borrow().clone()) else { return };
            term.clipboard().set_text(&uri);
        });
    }
    group.add_action(&open);
    group.add_action(&copy);
    pane.insert_action_group(ACTION_GROUP, Some(&group));
}

/// Files and folders dropped on the terminal are typed at the prompt as
/// shell-quoted paths; dropped plain text is typed as it is.
fn attach_drop(term: &vte4::Terminal) {
    let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
    target.set_types(&[gdk::FileList::static_type(), glib::Type::STRING]);
    let term_weak = term.downgrade();
    target.connect_drop(move |_, value, _, _| {
        let Some(term) = term_weak.upgrade() else { return false };
        let text = if let Ok(files) = value.get::<gdk::FileList>() {
            let paths: Vec<PathBuf> = files.files().iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() { return false; }
            crate::quote::dropped_paths_text(&paths)
        } else if let Ok(text) = value.get::<String>() {
            text
        } else {
            return false;
        };
        term.feed_child(text.as_bytes());
        term.grab_focus();
        true
    });
    term.add_controller(target);
}

/// Track the focus: remember the terminal, mark its pane and update titles.
fn attach_focus(term: &vte4::Terminal, pane: &gtk::Box, shared: &Rc<Shared>) {
    let focus = gtk::EventControllerFocus::new();
    {
        let shared = shared.clone();
        let term_weak = term.downgrade();
        let pane = pane.clone();
        focus.connect_enter(move |_| {
            let Some(term) = term_weak.upgrade() else { return };
            shared.remember_focus(&term);
            let split = crate::layout::tab_root_of(term.upcast_ref())
                .is_some_and(|root| crate::layout::terminals_in(&root).len() > 1);
            if split { pane.add_css_class(FOCUSED_CLASS); }
            crate::window::refresh_titles(&shared, &term);
        });
    }
    {
        let pane = pane.clone();
        focus.connect_leave(move |_| pane.remove_css_class(FOCUSED_CLASS));
    }
    term.add_controller(focus);
}

/// Multiply the font scale of `term` by `factor` (1.0 resets it).
pub fn zoom(term: &vte4::Terminal, factor: Option<f64>) {
    let scale = match factor {
        Some(f) => (term.font_scale() * f).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1),
        None => 1.0,
    };
    term.set_font_scale(scale);
}

/// Zoom step for Zoom In (`true`) or Zoom Out (`false`).
pub fn zoom_factor(zoom_in: bool) -> f64 {
    if zoom_in { ZOOM_STEP } else { 1.0 / ZOOM_STEP }
}

/// The directory the shell of `term` is in: the one it reported (OSC 7)
/// or, failing that, the one of the foreground process of its pty.
pub fn working_directory(term: &vte4::Terminal) -> Option<PathBuf> {
    if let Some(path) = term.current_directory_uri().and_then(|uri| gio::File::for_uri(&uri).path()) {
        return Some(path);
    }
    let pty = term.pty()?;
    // SAFETY: tcgetpgrp only reads the process group of a valid open fd.
    let pgrp = unsafe { libc::tcgetpgrp(pty.fd().as_raw_fd()) };
    if pgrp <= 0 { return None; }
    std::fs::read_link(format!("/proc/{pgrp}/cwd")).ok()
}

/// The user's shell: `SHELL` when set, otherwise the passwd entry, otherwise
/// `/bin/sh`.
pub fn detect_user_shell() -> String {
    if let Some(shell) = std::env::var("SHELL").ok().filter(|s| !s.trim().is_empty()) {
        return shell.trim().to_string();
    }
    // SAFETY: getpwuid returns a pointer into static storage (or null) and
    // the string is copied out at once on the single GTK thread.
    let from_passwd = unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_shell.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr((*entry).pw_shell).to_string_lossy().into_owned())
        }
    };
    from_passwd.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "/bin/sh".to_string())
}
