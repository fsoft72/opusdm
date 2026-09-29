//! Type-to-find: typing letters over the file list jumps to the first name
//! that starts with what was typed.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
use std::time::Duration;

/// Idle time after which the typed text is forgotten.
const IDLE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Prefix of the status line message that shows the typed text.
const MESSAGE_PREFIX: &str = "Find: ";

/// True when `c` may open the type-to-find buffer. Once the buffer is open any
/// printable character is accepted, so `my-file` can be typed in full; opening
/// is restricted so `+ - * ~ / space` keep their meaning on an idle list.
pub fn is_start_char(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '_'
}

/// True when `c` may be appended to an already open buffer.
pub fn is_buffer_char(c: char) -> bool {
    !c.is_control()
}

/// Index of the first name that starts with `prefix`, ignoring case.
pub fn find_prefix<I, S>(names: I, prefix: &str) -> Option<usize>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let wanted = prefix.to_lowercase();
    names.into_iter().position(|n| n.as_ref().to_lowercase().starts_with(&wanted))
}

impl Ui {
    /// Feed a key to an already open type-to-find buffer. Returns true when
    /// the key was consumed. Does nothing (false) while no buffer is open.
    pub fn typeahead_feed(self: &Rc<Self>, key: gdk::Key, modified: bool) -> bool {
        if self.typeahead_buffer.borrow().is_empty() {
            return false;
        }
        if modified {
            self.typeahead_close();
            return false;
        }
        match key {
            gdk::Key::Escape => {
                self.typeahead_close();
                true
            }
            gdk::Key::BackSpace => {
                self.typeahead_buffer.borrow_mut().pop();
                if self.typeahead_buffer.borrow().is_empty() {
                    self.typeahead_close();
                } else {
                    self.typeahead_search();
                }
                true
            }
            _ => match key.to_unicode() {
                Some(c) if is_buffer_char(c) => {
                    self.typeahead_push(c);
                    true
                }
                // Return, arrows, Tab, F-keys: leave the buffer and let the
                // key do its normal job.
                _ => {
                    self.typeahead_close();
                    false
                }
            },
        }
    }

    /// Open the buffer if `key` is a valid first character. Returns true when
    /// the key was consumed.
    pub fn typeahead_begin(self: &Rc<Self>, key: gdk::Key) -> bool {
        match key.to_unicode() {
            Some(c) if is_start_char(c) => {
                self.typeahead_push(c);
                true
            }
            _ => false,
        }
    }

    /// Append `c` to the buffer if some name still matches, then move there.
    fn typeahead_push(self: &Rc<Self>, c: char) {
        self.typeahead_buffer.borrow_mut().push(c);
        if self.typeahead_search() {
            return;
        }
        // No name starts with that: take the character back out.
        let typed = {
            let mut buf = self.typeahead_buffer.borrow_mut();
            buf.pop();
            buf.clone()
        };
        if typed.is_empty() {
            self.typeahead_close();
        } else {
            self.status_bar.job_line.set_text(&format!("{MESSAGE_PREFIX}{typed} (no match)"));
        }
    }

    /// Jump to the first row matching the buffer. False when none does.
    fn typeahead_search(self: &Rc<Self>) -> bool {
        let typed = self.typeahead_buffer.borrow().clone();
        let n = self.view.selection.n_items();
        let names = (0..n).map(|i| self.row_at(i).map(|r| r.name.clone()).unwrap_or_default());
        let Some(pos) = find_prefix(names, &typed) else { return false };
        self.jump_to_row(pos as u32);
        self.status_bar.job_line.set_text(&format!("{MESSAGE_PREFIX}{typed}"));
        self.typeahead_restart_timer();
        true
    }

    /// Forget the typed text and clear its message.
    pub fn typeahead_close(&self) {
        self.typeahead_buffer.borrow_mut().clear();
        if let Some(id) = self.typeahead_timer.borrow_mut().take() {
            id.remove();
        }
        if self.status_bar.job_line.text().starts_with(MESSAGE_PREFIX) {
            self.status_bar.job_line.set_text("");
        }
    }

    /// Re-arm the idle timeout.
    fn typeahead_restart_timer(self: &Rc<Self>) {
        if let Some(id) = self.typeahead_timer.borrow_mut().take() {
            id.remove();
        }
        let ui = self.clone();
        let id = glib::timeout_add_local_once(IDLE_TIMEOUT, move || {
            // The source is gone once it fires; drop the stale id first.
            ui.typeahead_timer.borrow_mut().take();
            ui.typeahead_close();
        });
        *self.typeahead_timer.borrow_mut() = Some(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn car_finds_cargo_toml() {
        let names = ["build.rs", "Cargo.lock", "Cargo.toml", "src"];
        assert_eq!(find_prefix(names, "car"), Some(1));
        assert_eq!(find_prefix(names, "cargo.t"), Some(2));
    }

    #[test]
    fn no_match_is_none() {
        assert_eq!(find_prefix(["a", "b"], "zz"), None);
    }

    #[test]
    fn matching_ignores_case_both_ways() {
        assert_eq!(find_prefix(["README"], "rea"), Some(0));
        assert_eq!(find_prefix(["readme"], "REA"), Some(0));
    }

    #[test]
    fn only_name_like_characters_open_the_buffer() {
        for c in ['a', 'Z', '7', '.', '_', 'é'] {
            assert!(is_start_char(c), "{c:?}");
        }
        for c in ['+', '-', '*', '~', '/', ' ', '`'] {
            assert!(!is_start_char(c), "{c:?}");
        }
    }

    #[test]
    fn an_open_buffer_takes_dashes_and_spaces() {
        assert!(is_buffer_char('-'));
        assert!(is_buffer_char(' '));
        assert!(!is_buffer_char('\n'));
    }
}
