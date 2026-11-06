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
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-panel";
/// A password manager marks what it copies with this type (KDE convention,
/// also used by KeePassXC and Bitwarden): such a copy is never remembered.
const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";

/// Wait before the image is put back on an empty clipboard, in milliseconds.
const RESTORE_DELAY_MS: u64 = 300;

/// Shared state of the clipboard manager.
pub struct ClipboardCenter {
    history: RefCell<History>,
    /// The last image copied (a screenshot, say), kept in memory only. It is
    /// put back on the clipboard when its owner quits and nothing else was
    /// copied meanwhile, because an image on the clipboard vanishes with the
    /// program that copied it.
    image: RefCell<Option<gdk::Texture>>,
    /// Whether the image is the latest thing copied (no text came after it).
    image_current: Cell<bool>,
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
            image: RefCell::new(None),
            image_current: Cell::new(false),
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

    /// Number of remembered entries (the image counts as one).
    pub fn len(&self) -> usize {
        self.history.borrow().entries().len() + usize::from(self.image.borrow().is_some())
    }

    /// Width and height of the remembered image, if there is one.
    pub fn image_size(&self) -> Option<(i32, i32)> {
        self.image.borrow().as_ref().map(|t| (t.width(), t.height()))
    }

    /// Put the remembered image back on the clipboard.
    pub fn select_image(self: &Rc<Self>) {
        let Some(texture) = self.image.borrow().clone() else {
            opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: the image is gone");
            return;
        };
        let Some(clipboard) = &self.clipboard else {
            opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: no clipboard to put the image on");
            return;
        };
        self.image_current.set(true);
        clipboard.set_texture(&texture);
    }

    /// Forget the remembered image.
    pub fn remove_image(self: &Rc<Self>) {
        self.image.borrow_mut().take();
        self.image_current.set(false);
        self.notify();
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
        self.image.borrow_mut().take();
        self.image_current.set(false);
        self.notify();
    }

    /// The clipboard changed: remember the text, unless it is not text or a
    /// password manager copied it.
    fn read_clipboard(self: &Rc<Self>) {
        let Some(clipboard) = &self.clipboard else { return };
        let formats = clipboard.formats();
        if formats.contain_mime_type(PASSWORD_HINT_MIME) { return; }
        if formats.contains_type(gdk::Texture::static_type()) { return self.read_image(clipboard); }
        // The owner of the image quit and left the clipboard empty: put the
        // image back, as long as it is the latest thing copied.
        if formats.mime_types().is_empty() {
            if !self.image_current.get() { return; }
            // The clipboard also reads empty for a moment when another
            // program takes it over, before its content is known: wait, and
            // look again, so a new copy is never overwritten.
            let center = self.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(RESTORE_DELAY_MS), move || center.restore_image());
            return;
        }
        // Anything else on the clipboard is newer than the image. Said here,
        // not when the text is read: the owner of a one-shot copy can quit
        // before the read ends, and the empty clipboard must not bring the
        // image back over it.
        self.image_current.set(false);
        // A file list is not an error: it is just not kept.
        if !formats.contains_type(glib::GString::static_type()) { return; }
        let center = self.clone();
        clipboard.read_text_async(gtk::gio::Cancellable::NONE, move |res| match res {
            Ok(Some(text)) => {
                if center.history.borrow_mut().push(&text) != Push::Unchanged { center.notify(); }
            }
            Ok(None) => {}
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: cannot read the clipboard: {e}"),
        });
    }

    /// Put the image back if the clipboard is still empty and the image is
    /// still the latest thing copied.
    fn restore_image(&self) {
        let Some(clipboard) = &self.clipboard else { return };
        if !self.image_current.get() || !clipboard.formats().mime_types().is_empty() { return; }
        let Some(texture) = self.image.borrow().clone() else { return };
        clipboard.set_texture(&texture);
    }

    /// The clipboard holds an image: remember it. What this panel put there
    /// itself is skipped, it is the remembered image already.
    fn read_image(self: &Rc<Self>, clipboard: &gdk::Clipboard) {
        if clipboard.is_local() { return; }
        let center = self.clone();
        clipboard.read_texture_async(gtk::gio::Cancellable::NONE, move |res| match res {
            Ok(Some(texture)) => {
                *center.image.borrow_mut() = Some(texture);
                center.image_current.set(true);
                center.notify();
            }
            Ok(None) => {}
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "clipboard manager: cannot read the image: {e}"),
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
