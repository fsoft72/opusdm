//! Quake-style dropdown embedded terminal drawer (F4 / `).

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

pub struct TerminalDrawer {
    pub container: gtk::Box,
    pub title_label: gtk::Label,
    pub text_view: gtk::TextView,
    pub entry: gtk::Entry,
    pub prompt_label: gtk::Label,
    #[allow(dead_code)]
    pub scroller: gtk::ScrolledWindow,
    pub cwd: RefCell<PathBuf>,
    pub history: RefCell<Vec<String>>,
    pub history_idx: RefCell<Option<usize>>,
}

impl TerminalDrawer {
    pub fn new() -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_size_request(-1, 200);
        container.add_css_class("terminal-drawer");
        container.set_visible(false);

        // Header bar
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_start(6);
        header.set_margin_end(6);
        header.set_margin_top(2);
        header.set_margin_bottom(2);
        header.add_css_class("terminal-header");

        let title_label = gtk::Label::new(Some("💻 Terminal: /"));
        title_label.set_xalign(0.0);
        title_label.set_hexpand(true);
        title_label.add_css_class("terminal-title");
        header.append(&title_label);

        let clear_btn = gtk::Button::with_label("Clear");
        clear_btn.add_css_class("flat");
        clear_btn.set_tooltip_text(Some("Clear console"));

        let xterm_btn = gtk::Button::with_label("External");
        xterm_btn.add_css_class("flat");
        xterm_btn.set_tooltip_text(Some("Open in external terminal (xterm)"));

        let close_btn = gtk::Button::with_label("✕");
        close_btn.add_css_class("flat");
        close_btn.set_tooltip_text(Some("Close terminal (F4 / `)"));

        header.append(&clear_btn);
        header.append(&xterm_btn);
        header.append(&close_btn);
        container.append(&header);

        // Terminal output TextView
        let text_view = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::Char)
            .build();
        text_view.add_css_class("terminal-view");
        text_view.set_left_margin(8);
        text_view.set_right_margin(8);
        text_view.set_top_margin(6);
        text_view.set_bottom_margin(6);

        let buffer = text_view.buffer();
        buffer.set_text("OpusDM Embedded Terminal — Type commands or 'cd <dir>' to navigate (Esc/F4 to hide)\n");

        let scroller = gtk::ScrolledWindow::builder()
            .child(&text_view)
            .vexpand(true)
            .hexpand(true)
            .build();
        container.append(&scroller);

        // Prompt row
        let prompt_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        prompt_box.set_margin_start(6);
        prompt_box.set_margin_end(6);
        prompt_box.set_margin_top(4);
        prompt_box.set_margin_bottom(4);

        let prompt_label = gtk::Label::new(Some("$ "));
        prompt_label.add_css_class("terminal-prompt");
        prompt_box.append(&prompt_label);

        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.add_css_class("terminal-entry");
        entry.set_placeholder_text(Some("Run shell command or cd..."));
        prompt_box.append(&entry);

        container.append(&prompt_box);

        let drawer = Rc::new(Self {
            container,
            title_label,
            text_view,
            entry,
            prompt_label,
            scroller,
            cwd: RefCell::new(PathBuf::from("/")),
            history: RefCell::new(Vec::new()),
            history_idx: RefCell::new(None),
        });

        // Clear button
        {
            let tv = drawer.text_view.clone();
            clear_btn.connect_clicked(move |_| {
                tv.buffer().set_text("");
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

    /// Sync the drawer working directory with the Lister CWD.
    pub fn sync_cwd(&self, new_cwd: &Path) {
        *self.cwd.borrow_mut() = new_cwd.to_path_buf();
        self.title_label
            .set_text(&format!("💻 Terminal: {}", new_cwd.display()));
        let dir_name = new_cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into());
        self.prompt_label.set_text(&format!("{dir_name} $ "));
    }

    /// Toggle drawer visibility and focus.
    pub fn toggle(&self) {
        let is_vis = self.container.is_visible();
        self.container.set_visible(!is_vis);
        if !is_vis {
            self.entry.grab_focus();
        }
    }

    /// Append text to terminal output and scroll to end.
    pub fn append_output(&self, text: &str) {
        let buf = self.text_view.buffer();
        let mut iter = buf.end_iter();
        buf.insert(&mut iter, text);
        // Scroll to end
        let mark = buf.create_mark(None, &iter, false);
        self.text_view.scroll_to_mark(&mark, 0.0, true, 0.0, 1.0);
    }

    /// Wire command execution and input history.
    pub fn attach_events(self: &Rc<Self>, ui: &Rc<Ui>) {
        let drawer_for_act = self.clone();
        let ui_clone = ui.clone();

        self.entry.connect_activate(move |entry| {
            let cmd_str = entry.text().trim().to_string();
            if cmd_str.is_empty() {
                return;
            }
            entry.set_text("");

            // Add to history
            {
                let mut hist = drawer_for_act.history.borrow_mut();
                if hist.last() != Some(&cmd_str) {
                    hist.push(cmd_str.clone());
                }
                *drawer_for_act.history_idx.borrow_mut() = None;
            }

            let current_dir = drawer_for_act.cwd.borrow().clone();
            drawer_for_act.append_output(&format!("\n$ {cmd_str}\n"));

            // Check for built-in 'cd' command
            if cmd_str == "cd" || cmd_str.starts_with("cd ") {
                let target_arg = if cmd_str == "cd" {
                    std::env::var("HOME").unwrap_or_else(|_| "/".into())
                } else {
                    cmd_str[3..].trim().to_string()
                };

                let target_path = resolve_path(&current_dir, &target_arg);
                if target_path.is_dir() {
                    drawer_for_act.sync_cwd(&target_path);
                    ui_clone.navigate_to(&target_path);
                    drawer_for_act.append_output(&format!("↳ navigated to {}\n", target_path.display()));
                } else {
                    drawer_for_act.append_output(&format!("cd: no such file or directory: {target_arg}\n"));
                }
                return;
            }

            // Built-in 'clear'
            if cmd_str == "clear" {
                drawer_for_act.text_view.buffer().set_text("");
                return;
            }

            // Asynchronous command execution
            let drawer_clone = drawer_for_act.clone();
            let (tx, rx) = async_channel::unbounded::<String>();

            std::thread::spawn(move || {
                let shell = detect_user_shell();
                let mut cmd = Command::new(&shell);
                // Pass `-l` (login shell) so that user profile/environment (/etc/profile, ~/.bash_profile,
                // ~/.bash_login, or ~/.profile) is sourced, ensuring the full user environment is present.
                cmd.arg("-l")
                    .arg("-c")
                    .arg(&cmd_str)
                    .current_dir(&current_dir)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());

                let child = cmd.spawn();

                match child {
                    Ok(mut process) => {
                        if let Some(stdout) = process.stdout.take() {
                            let tx_out = tx.clone();
                            std::thread::spawn(move || {
                                let reader = BufReader::new(stdout);
                                for line in reader.lines().map_while(Result::ok) {
                                    let _ = tx_out.send_blocking(format!("{line}\n"));
                                }
                            });
                        }
                        if let Some(stderr) = process.stderr.take() {
                            let tx_err = tx.clone();
                            std::thread::spawn(move || {
                                let reader = BufReader::new(stderr);
                                for line in reader.lines().map_while(Result::ok) {
                                    let _ = tx_err.send_blocking(format!("{line}\n"));
                                }
                            });
                        }
                        let status = process.wait();
                        match status {
                            Ok(code) => {
                                if !code.success() {
                                    let _ = tx.send_blocking(format!("[exited with status: {code}]\n"));
                                }
                            }
                            Err(e) => {
                                let _ = tx.send_blocking(format!("[error waiting for process: {e}]\n"));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send_blocking(format!("failed to execute process: {e}\n"));
                    }
                }
            });

            glib::spawn_future_local(async move {
                while let Ok(msg) = rx.recv().await {
                    drawer_clone.append_output(&msg);
                }
            });
        });

        // Key handler for Entry (Up/Down history, Esc / ` to hide)
        {
            let drawer_clone = self.clone();
            let key_ctrl = gtk::EventControllerKey::new();
            key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
                match keyval {
                    gdk::Key::Escape => {
                        drawer_clone.container.set_visible(false);
                        glib::Propagation::Stop
                    }
                    gdk::Key::grave | gdk::Key::F4 => {
                        if drawer_clone.entry.text().is_empty() {
                            drawer_clone.container.set_visible(false);
                            glib::Propagation::Stop
                        } else {
                            glib::Propagation::Proceed
                        }
                    }
                    gdk::Key::Up => {
                        let hist = drawer_clone.history.borrow();
                        if hist.is_empty() {
                            return glib::Propagation::Stop;
                        }
                        let new_idx = match *drawer_clone.history_idx.borrow() {
                            None => hist.len().saturating_sub(1),
                            Some(idx) => idx.saturating_sub(1),
                        };
                        drop(hist);
                        *drawer_clone.history_idx.borrow_mut() = Some(new_idx);
                        if let Some(cmd) = drawer_clone.history.borrow().get(new_idx) {
                            drawer_clone.entry.set_text(cmd);
                            drawer_clone.entry.set_position(-1);
                        }
                        glib::Propagation::Stop
                    }
                    gdk::Key::Down => {
                        let hist = drawer_clone.history.borrow();
                        if hist.is_empty() {
                            return glib::Propagation::Stop;
                        }
                        if let Some(idx) = *drawer_clone.history_idx.borrow() {
                            if idx + 1 < hist.len() {
                                let next_idx = idx + 1;
                                drop(hist);
                                *drawer_clone.history_idx.borrow_mut() = Some(next_idx);
                                if let Some(cmd) = drawer_clone.history.borrow().get(next_idx) {
                                    drawer_clone.entry.set_text(cmd);
                                    drawer_clone.entry.set_position(-1);
                                }
                            } else {
                                drop(hist);
                                *drawer_clone.history_idx.borrow_mut() = None;
                                drawer_clone.entry.set_text("");
                            }
                        }
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            self.entry.add_controller(key_ctrl);
        }
    }
}

/// Resolve a directory argument against the current directory, supporting `~` and `..`.
pub fn resolve_path(cwd: &Path, arg: &str) -> PathBuf {
    let trimmed = arg.trim();
    if trimmed.is_empty() || trimmed == "~" {
        return std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
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

    // Query user database via libc getpwuid_r
    let uid = unsafe { libc::getuid() };
    let mut passwd_buf = vec![0u8; 1024];
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut passwd_ptr = std::ptr::null_mut();

    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            &mut passwd,
            passwd_buf.as_mut_ptr() as *mut libc::c_char,
            passwd_buf.len(),
            &mut passwd_ptr,
        )
    };

    if ret == 0 && !passwd_ptr.is_null() && !unsafe { (*passwd_ptr).pw_shell }.is_null() {
        if let Ok(shell_str) = unsafe { std::ffi::CStr::from_ptr((*passwd_ptr).pw_shell) }.to_str() {
            let trimmed = shell_str.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    "/bin/sh".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path() {
        let cwd = Path::new("/home/user/project");
        assert_eq!(resolve_path(cwd, "src"), PathBuf::from("/home/user/project/src"));
        assert_eq!(resolve_path(cwd, "/etc"), PathBuf::from("/etc"));
        assert_eq!(resolve_path(cwd, ".."), PathBuf::from("/home/user/project/.."));
    }

    #[test]
    fn test_detect_user_shell() {
        let shell = detect_user_shell();
        assert!(!shell.is_empty());
        assert!(shell.starts_with('/') || shell.contains("sh"));
    }
}

