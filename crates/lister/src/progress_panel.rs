//! A small progress panel shown inside the Lister window, centered over the
//! file area: a message, a progress bar and a Cancel button. It is a widget
//! of the window (an overlay child), so it has no border, cannot be moved and
//! follows the window. While it is shown the rest of the window is
//! insensitive; the key handlers check [`ProgressPanel::is_active`].
//!
//! Ownership: the `Ui` owns the panel for the whole run. The Cancel button
//! handler only holds the callback slot, never the `Ui`; the callback a
//! caller installs holds a `Weak<Ui>`.

use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What Cancel runs, installed by [`ProgressPanel::show`].
type CancelSlot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

/// Width of the panel in pixels.
const PANEL_WIDTH: i32 = 360;

/// The progress panel and the widget it mutes while shown.
pub struct ProgressPanel {
    /// The panel itself, the overlay child.
    container: gtk::Box,
    message: gtk::Label,
    bar: gtk::ProgressBar,
    /// The window content below the panel, insensitive while it is shown.
    muted: gtk::Widget,
    /// The widget that had the focus when the panel was shown.
    focus_before: RefCell<Option<gtk::Widget>>,
    active: Cell<bool>,
    on_cancel: CancelSlot,
}

impl ProgressPanel {
    /// Build the panel (hidden). `muted` is the content it covers; the caller
    /// adds [`ProgressPanel::widget`] as an overlay over it.
    pub fn new(muted: &impl IsA<gtk::Widget>) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        container.add_css_class("opus-progress");
        container.set_halign(gtk::Align::Center);
        container.set_valign(gtk::Align::Center);
        container.set_size_request(PANEL_WIDTH, -1);
        container.set_visible(false);
        let message = gtk::Label::new(None);
        message.set_xalign(0.0);
        message.set_wrap(true);
        message.set_max_width_chars(40);
        let bar = gtk::ProgressBar::new();
        let cancel_btn = gtk::Button::with_label("Cancel");
        cancel_btn.add_css_class("opus-btn");
        cancel_btn.set_halign(gtk::Align::End);
        container.append(&message);
        container.append(&bar);
        container.append(&cancel_btn);

        let on_cancel: CancelSlot = Rc::new(RefCell::new(None));
        {
            let slot = on_cancel.clone();
            cancel_btn.connect_clicked(move |_| _run_cancel(&slot));
        }
        Self {
            container,
            message,
            bar,
            muted: muted.clone().upcast(),
            focus_before: RefCell::new(None),
            active: Cell::new(false),
            on_cancel,
        }
    }

    /// The panel widget, to add as an overlay child.
    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    /// Whether the panel is on screen (the window is muted).
    pub fn is_active(&self) -> bool {
        self.active.get()
    }

    /// Show the panel with `message` and an empty bar, mute the window, and
    /// run `on_cancel` when the user presses Cancel (or Escape, see
    /// [`ProgressPanel::cancel`]). Showing it again replaces both.
    pub fn show(&self, message: &str, on_cancel: impl Fn() + 'static) {
        if !self.active.get() {
            let root = self.muted.root();
            *self.focus_before.borrow_mut() = root.and_then(|r| r.focus());
        }
        self.message.set_text(message);
        self.bar.set_fraction(0.0);
        *self.on_cancel.borrow_mut() = Some(Rc::new(on_cancel));
        self.active.set(true);
        self.muted.set_sensitive(false);
        self.container.set_visible(true);
        crate::dlog!("progress panel shown: {message}");
    }

    /// Change the message while the work goes on.
    pub fn set_message(&self, message: &str) {
        if self.message.text() != message {
            self.message.set_text(message);
        }
    }

    /// Set the bar to `fraction` (0..1), or make it pulse when the total is
    /// not known (`None`).
    pub fn set_fraction(&self, fraction: Option<f64>) {
        match fraction {
            Some(f) => self.bar.set_fraction(f.clamp(0.0, 1.0)),
            None => self.bar.pulse(),
        }
    }

    /// Hide the panel, give the window back its input and the focus to the
    /// widget that had it. Does nothing when hidden.
    pub fn hide(&self) {
        if !self.active.replace(false) { return; }
        self.on_cancel.borrow_mut().take();
        self.container.set_visible(false);
        self.muted.set_sensitive(true);
        if let Some(widget) = self.focus_before.borrow_mut().take() {
            widget.grab_focus();
        }
        crate::dlog!("progress panel hidden");
    }

    /// Run the Cancel action, as the button does (for Escape).
    pub fn cancel(&self) {
        _run_cancel(&self.on_cancel);
    }
}

/// Run the installed Cancel callback. It is cloned out first: the callback
/// usually hides the panel, which empties the slot.
fn _run_cancel(slot: &CancelSlot) {
    let callback = slot.borrow().clone();
    match callback {
        Some(callback) => callback(),
        None => opusdm_ipc::err_log!("opusdm-lister", "progress panel: Cancel pressed with nothing to cancel"),
    }
}
