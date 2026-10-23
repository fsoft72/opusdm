//! The panel side of the clipboard manager: it listens to the system
//! clipboard, keeps the history of the texts copied and puts an entry back on
//! the clipboard when the popover asks. The history lives in memory only (a
//! copied password must not reach the disk) and is shared by every bar, so a
//! monitor-hotplug rebuild of the bars never loses it.

use crate::clipboard_model::{History, Push, MAX_ENTRIES};
use crate::widgets::clipboard::ClipboardButton;
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-panel";
/// A password manager marks what it copies with this type (KDE convention,
/// also used by KeePassXC and Bitwarden): such a copy is never remembered.
const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";

/// Shared state of the clipboard manager.
pub struct ClipboardCenter {
    history: RefCell<History>,
    /// The system clipboard; `None` when there is no display.
    clipboard: Option<gdk::Clipboard>,
    /// Strong, because nothing else owns a `ClipboardButton`; pruned once its
    /// widget leaves the bar (a rebuild closes the old bar window).
    buttons: RefCell<Vec<Rc<ClipboardButton>>>,
}

impl ClipboardCenter {
    /// Start listening to the clipboard and read what it holds now.
    pub fn new() -> Rc<Self> {
        let clipboard = gdk::Display::default().map(|d| d.clipboard());
        if clipboard.is_none() {
            opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: no display, the history stays empty");
        }
        let center = Rc::new(Self {
            history: RefCell::new(History::new(MAX_ENTRIES)),
            clipboard: clipboard.clone(),
            buttons: RefCell::new(Vec::new()),
        });
        if let Some(clipboard) = clipboard {
            // The handler holds the center strongly and the center holds the
            // clipboard: a cycle on purpose, both live as long as the panel
            // (the clipboard belongs to the display). Like the printer tray.
            let for_signal = center.clone();
            clipboard.connect_changed(move |_| for_signal.read_clipboard());
            center.read_clipboard();
        }
        center
    }

    /// The tray widget for one bar. It follows the shared state while it is alive.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = ClipboardButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// The entries holding `query`, newest first, with their history index.
    pub fn entries(&self, query: &str) -> Vec<(usize, String)> {
        self.history.borrow().filtered(query).into_iter().map(|(i, e)| (i, e.to_string())).collect()
    }

    /// Number of remembered entries.
    pub fn len(&self) -> usize {
        self.history.borrow().entries().len()
    }

    /// Put the entry at `index` back on the clipboard (it moves to the top
    /// when the clipboard reports the change).
    pub fn select(self: &Rc<Self>, index: usize) {
        let Some(text) = self.history.borrow().entries().get(index).cloned() else {
            opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: entry {index} is gone");
            return;
        };
        let Some(clipboard) = &self.clipboard else {
            opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: no clipboard to put an entry on");
            return;
        };
        clipboard.set_text(&text);
        // The change signal does the same, but only if the clipboard really
        // changed: this keeps the list right when it did not.
        if self.history.borrow_mut().push(&text) != Push::Unchanged { self.notify(); }
    }

    /// Forget the entry at `index`.
    pub fn remove(self: &Rc<Self>, index: usize) {
        if !self.history.borrow_mut().remove(index) {
            opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: cannot remove entry {index}, it is gone");
            return;
        }
        self.notify();
    }

    /// Forget every entry. The clipboard itself keeps its content.
    pub fn clear(self: &Rc<Self>) {
        self.history.borrow_mut().clear();
        self.notify();
    }

    /// The clipboard changed: remember the text, unless it is not text or a
    /// password manager copied it.
    fn read_clipboard(self: &Rc<Self>) {
        let Some(clipboard) = &self.clipboard else { return };
        let formats = clipboard.formats();
        // An image or a file list is not an error: it is just not kept.
        if !formats.contains_type(glib::GString::static_type()) { return; }
        if formats.contain_mime_type(PASSWORD_HINT_MIME) { return; }
        let center = self.clone();
        clipboard.read_text_async(gtk::gio::Cancellable::NONE, move |res| match res {
            Ok(Some(text)) => {
                if center.history.borrow_mut().push(&text) != Push::Unchanged { center.notify(); }
            }
            Ok(None) => {}
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: cannot read the clipboard: {e}"),
        });
    }

    /// Redraw every live button.
    fn notify(self: &Rc<Self>) {
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<ClipboardButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}
