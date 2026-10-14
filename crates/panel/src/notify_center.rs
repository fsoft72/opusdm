//! The panel side of the notifications: the state `opusdm-notify` last sent
//! and the bells that show it. Owned by `attach`, so a rebuild of the bars
//! keeps the state.

use crate::notify_client::{self, Client, ClientEvent};
use crate::widgets::notifications::NotifyButton;
use gtk4 as gtk;
use gtk::glib;
use opusdm_ipc::notify::NotifyState;
use std::cell::RefCell;
use std::rc::Rc;

/// Shared state of the bells.
pub struct NotifyCenter {
    /// `None` while the server is not running.
    state: RefCell<Option<NotifyState>>,
    /// Strong, because nothing else owns a button; pruned once its widget
    /// leaves the bar.
    buttons: RefCell<Vec<Rc<NotifyButton>>>,
    client: Client,
}

impl NotifyCenter {
    /// Start listening to the server.
    pub fn new() -> Rc<Self> {
        let (client, rx) = notify_client::spawn();
        let center = Rc::new(Self { state: RefCell::new(None), buttons: RefCell::new(Vec::new()), client });
        {
            let center = center.clone();
            glib::spawn_future_local(async move {
                while let Ok(event) = rx.recv().await {
                    center.on_event(event);
                }
            });
        }
        center
    }

    /// The bell for one bar.
    pub fn button(self: &Rc<Self>) -> gtk::Widget {
        let button = NotifyButton::new(self);
        self.buttons.borrow_mut().push(button.clone());
        button.refresh(self);
        button.widget()
    }

    /// The last state; `None` while the server is not running.
    pub fn state(&self) -> Option<NotifyState> {
        self.state.borrow().clone()
    }

    /// The requests to the server.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Flip Do Not Disturb.
    pub fn toggle_dnd(&self) {
        let Some(state) = self.state() else { return };
        self.client.set_dnd(!state.dnd);
    }

    /// One event of the listener.
    fn on_event(self: &Rc<Self>, event: ClientEvent) {
        let state = match event {
            ClientEvent::State(s) => Some(s),
            ClientEvent::Gone => None,
        };
        *self.state.borrow_mut() = state;
        self.buttons.borrow_mut().retain(|b| b.is_attached());
        let live: Vec<Rc<NotifyButton>> = self.buttons.borrow().clone();
        for b in live {
            b.refresh(self);
        }
    }
}
