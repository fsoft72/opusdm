//! Quake-style dropdown terminal drawer (F4 / `), backed by a real VTE
//! terminal running the user's login shell.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use vte4::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

/// Minimum drawer height: header row plus ~2 terminal rows. Enforced via
/// `set_size_request` and `Paned::set_shrink_end_child(false)` in `build.rs`
/// so GTK's own paned handle refuses to drag the VTE below this.
pub const MIN_DRAWER_HEIGHT: i32 = 90;

/// Default drawer height on first show.
pub const DEFAULT_DRAWER_HEIGHT: i32 = 260;

pub struct TerminalDrawer {
    pub container: gtk::Box,
    pub title_label: gtk::Label,
    /// Slot holding the live `vte4::Terminal` (moved out on detach).
    term_slot: gtk::Box,
    /// The live terminal widget, `None` before the first spawn, after the
    /// shell exited, and after a detach.
    terminal: RefCell<Option<vte4::Terminal>>,
    /// Last known Lister directory: the spawn cwd for the next shell.
    cwd: RefCell<PathBuf>,
}

impl TerminalDrawer {
    pub fn new() -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_size_request(-1, MIN_DRAWER_HEIGHT);
        container.add_css_class("terminal-drawer");
        container.set_visible(false);

        // Height is resized via the standard GtkPaned handle the caller
        // wraps this container in (see `build.rs`); no custom drag logic
        // lives here.

        // Header bar
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_start(6);
        header.set_margin_end(6);
        header.set_margin_top(2);
        header.set_margin_bottom(2);
        header.add_css_class("terminal-header");

        let title_label = gtk::Label::new(Some("Terminal: /"));
        title_label.set_xalign(0.0);
        title_label.set_hexpand(true);
        title_label.add_css_class("terminal-title");
        header.append(&title_label);

        let clear_btn = gtk::Button::with_label("Clear");
        clear_btn.add_css_class("flat");
        clear_btn.set_tooltip_text(Some("Clear console"));

        let detach_btn = gtk::Button::with_label("Detach");
        detach_btn.add_css_class("flat");
        detach_btn.set_tooltip_text(Some("Move the shell into its own window"));

        let xterm_btn = gtk::Button::with_label("External");
        xterm_btn.add_css_class("flat");
        xterm_btn.set_tooltip_text(Some("Open in external terminal (xterm)"));

        let close_btn = gtk::Button::with_label("✕");
        close_btn.add_css_class("flat");
        close_btn.set_tooltip_text(Some("Close terminal (F4)"));

        header.append(&clear_btn);
        header.append(&detach_btn);
        header.append(&xterm_btn);
        header.append(&close_btn);
        container.append(&header);

        // Slot for the VTE widget.
        let term_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        term_slot.set_vexpand(true);
        container.append(&term_slot);

        let drawer = Rc::new(Self {
            container,
            title_label,
            term_slot,
            terminal: RefCell::new(None),
            cwd: RefCell::new(PathBuf::from("/")),
        });

        // Clear button
        {
            let drawer = drawer.clone();
            clear_btn.connect_clicked(move |_| {
                if let Some(term) = drawer.terminal.borrow().as_ref() {
                    term.reset(true, true);
                }
            });
        }

        // Detach button: move the live VTE widget into its own window. The
        // widget IS the shell session, so scrollback and process survive.
        {
            let drawer = drawer.clone();
            detach_btn.connect_clicked(move |_| {
                let Some(term) = drawer.terminal.borrow_mut().take() else {
                    return;
                };
                drawer.term_slot.remove(&term);
                drawer.container.set_visible(false);

                let win = gtk::Window::builder()
                    .title("OpusDM Terminal")
                    .default_width(900)
                    .default_height(500)
                    .build();
                term.connect_window_title_changed(move |t| {
                    if let Some(w) = t.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
                        if let Some(title) = t.window_title() {
                            w.set_title(Some(&title));
                        }
                    }
                });
                win.set_child(Some(&term));
                win.present();
                term.grab_focus();
            });
        }

        // Close button
        {
            let cont = drawer.container.clone();
            close_btn.connect_clicked(move |_| {
                cont.set_visible(false);
            });
        }

        // External terminal button
        {
            let drawer_clone = drawer.clone();
            xterm_btn.connect_clicked(move |_| {
                let dir = drawer_clone.cwd.borrow().clone();
                let _ = Command::new("xterm")
                    .current_dir(&dir)
                    .spawn();
            });
        }

        drawer
    }

    /// Wire window-level integration: F4 toggling while the terminal has
    /// focus (VTE swallows function keys before bubble-phase controllers see
    /// them, so this must run in the capture phase).
    pub fn attach(self: &Rc<Self>, ui: &Rc<Ui>) {
        let ui = ui.clone();
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let ui_keys = ui.clone();
        key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gdk::Key::F4 {
                ui_keys.toggle_terminal();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        ui.window.add_controller(key_ctrl);
    }

    /// Create a fresh VTE running the user's login shell in `cwd`, wire its
    /// lifecycle signals and drop it into the slot.
    fn spawn_terminal(self: &Rc<Self>) {
        let cwd = self.cwd.borrow().clone();
        let term = vte4::Terminal::new();
        term.set_scrollback_lines(10_000);
        term.set_vexpand(true);
        term.set_hexpand(true);

        // Shell exit -> tear the drawer down; the next F4 respawns fresh.
        let drawer = self.clone();
        term.connect_child_exited(move |_, _| {
            drawer.term_slot_remove();
            *drawer.terminal.borrow_mut() = None;
            drawer.container.set_visible(false);
        });

        term.spawn_async(
            vte4::PtyFlags::DEFAULT,
            Some(&cwd.to_string_lossy()),
            &[&detect_user_shell(), "-l"],
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gtk::gio::Cancellable>,
            |_| {},
        );

        self.term_slot.append(&term);
        *self.terminal.borrow_mut() = Some(term);
    }

    /// Remove the current terminal widget from the slot, if present.
    fn term_slot_remove(&self) {
        if let Some(term) = self.terminal.borrow().as_ref() {
            self.term_slot.remove(term);
        }
    }

    /// Sync the drawer working directory with the Lister CWD. Only affects
    /// the title and the spawn directory of the NEXT shell: a running shell
    /// is left alone.
    pub fn sync_cwd(&self, new_cwd: &Path) {
        *self.cwd.borrow_mut() = new_cwd.to_path_buf();
        self.title_label
            .set_text(&format!("Terminal: {}", new_cwd.display()));
    }

    /// Toggle drawer visibility and focus. Spawns the shell lazily on first
    /// show (or after the previous one exited / was detached).
    pub fn toggle(self: &Rc<Self>) {
        let is_vis = self.container.is_visible();
        self.container.set_visible(!is_vis);
        if !is_vis {
            if self.terminal.borrow().is_none() {
                self.spawn_terminal();
            }
            if let Some(term) = self.terminal.borrow().as_ref() {
                term.grab_focus();
            }
        }
    }
}

/// Detect the user's preferred shell:
/// 1. `SHELL` environment variable if set and non-empty.
/// 2. User's shell from `/etc/passwd` via libc `getpwuid_r`.
/// 3. Fallback to `/bin/sh`.
pub fn detect_user_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        let trimmed = shell.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let uid = unsafe { libc::getuid() };
    if let Some(entry) = crate::passwd::lookup(uid) {
        let trimmed = entry.shell.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    "/bin/sh".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_user_shell() {
        let shell = detect_user_shell();
        assert!(!shell.is_empty());
        assert!(shell.starts_with('/') || shell.contains("sh"));
    }
}
