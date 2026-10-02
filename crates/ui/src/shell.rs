//! The shared skeleton of the larger OpusDM dialogs: a modal window with a
//! padded vertical content area and a button row along the bottom. It exists
//! so the half-dozen tool dialogs stop re-deriving margins, spacing and the
//! close-button wiring on their own.

use gtk4 as gtk;
use gtk::prelude::*;

/// Padding around the content area and the button row.
const DIALOG_MARGIN: i32 = 12;

/// Gap between the content area and the button row.
const BUTTON_ROW_MARGIN_TOP: i32 = 8;

/// Gap between sibling widgets inside the content area and the button row.
const WIDGET_SPACING: i32 = 8;

/// A modal dialog window with a content area and a bottom button row.
///
/// Build it, fill [`content`](Self::content), add buttons, then
/// [`present`](Self::present) it - the widget tree is only assembled at that
/// point, so the layout options can be set in any order.
pub struct DialogShell {
    window: gtk::Window,
    content: gtk::Box,
    buttons: gtk::Box,
    scrolled: bool,
}

impl DialogShell {
    /// Create a modal dialog titled `title`, transient for `parent`.
    pub fn new(parent: &impl IsA<gtk::Window>, title: &str) -> Self {
        let window = gtk::Window::builder()
            .title(title)
            .transient_for(parent)
            .modal(true)
            .build();
        window.add_css_class(crate::REQUESTER_CLASS);
        Self::with_window(window)
    }

    /// Wrap `window` with the padded content area and the button row.
    fn with_window(window: gtk::Window) -> Self {
        let content = gtk::Box::new(gtk::Orientation::Vertical, WIDGET_SPACING);
        content.set_margin_start(DIALOG_MARGIN);
        content.set_margin_end(DIALOG_MARGIN);
        content.set_margin_top(DIALOG_MARGIN);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, WIDGET_SPACING);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_start(DIALOG_MARGIN);
        buttons.set_margin_end(DIALOG_MARGIN);
        buttons.set_margin_bottom(DIALOG_MARGIN);
        buttons.set_margin_top(BUTTON_ROW_MARGIN_TOP);

        Self { window, content, buttons, scrolled: false }
    }

    /// Create a dialog with no parent window, owned by `app`, for tools that
    /// run on their own (the application exits when it is closed).
    pub fn standalone(app: &impl IsA<gtk::Application>, title: &str) -> Self {
        let window = gtk::Window::builder().title(title).application(app).build();
        window.add_css_class(crate::REQUESTER_CLASS);
        Self::with_window(window)
    }

    /// Make the dialog modal (the default) or not. A non-modal dialog leaves
    /// the other windows usable, for dialogs that act on them (Find).
    pub fn modal(self, modal: bool) -> Self {
        self.window.set_modal(modal);
        self
    }

    /// Set the default window size.
    pub fn size(self, width: i32, height: i32) -> Self {
        self.window.set_default_size(width, height);
        self
    }

    /// Allow or forbid resizing (dialogs are resizable by default).
    pub fn resizable(self, resizable: bool) -> Self {
        self.window.set_resizable(resizable);
        self
    }

    /// Put the content area inside a scroller, keeping the button row fixed.
    pub fn scrolled(mut self) -> Self {
        self.scrolled = true;
        self
    }

    /// The dialog window, for callers that need to close it or attach
    /// controllers of their own.
    pub fn window(&self) -> &gtk::Window {
        &self.window
    }

    /// The vertical box holding the dialog body.
    pub fn content(&self) -> &gtk::Box {
        &self.content
    }

    /// Append a button to the bottom row and return it for wiring.
    pub fn button(&self, label: &str) -> gtk::Button {
        let btn = gtk::Button::with_label(label);
        self.buttons.append(&btn);
        btn
    }

    /// Append a button that closes the dialog when clicked.
    pub fn close_button(&self, label: &str) -> gtk::Button {
        let btn = self.button(label);
        let window = self.window.clone();
        btn.connect_clicked(move |_| window.close());
        btn
    }

    /// Push the buttons added from here on to the right edge, leaving the
    /// earlier ones on the left.
    pub fn button_spacer(&self) {
        self.buttons.set_halign(gtk::Align::Fill);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        self.buttons.append(&spacer);
    }

    /// Assemble the widget tree and show the dialog.
    pub fn present(self) -> gtk::Window {
        // A dialog with no buttons at all (the command palette) must not keep
        // the empty row's padding at the bottom.
        let has_buttons = self.buttons.first_child().is_some();
        if !has_buttons {
            self.content.set_margin_bottom(DIALOG_MARGIN);
        }

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        if self.scrolled {
            let scroller = gtk::ScrolledWindow::builder()
                .child(&self.content)
                .vexpand(true)
                .hexpand(true)
                .build();
            root.append(&scroller);
        } else {
            root.append(&self.content);
        }
        if has_buttons {
            root.append(&self.buttons);
        }

        self.window.set_child(Some(&root));
        self.window.present();
        self.window
    }
}
