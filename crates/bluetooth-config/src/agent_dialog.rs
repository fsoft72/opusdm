//! The dialogs of the pairing agent: the PIN or passkey to type, the code to
//! confirm, the device to allow. Shared by the Bluetooth tool and the panel.
//! A dialog answers once; closing it rejects the request.

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_bluetooth::{AgentEvent, AgentKind, AgentReply, BtState};
use std::cell::RefCell;
use std::rc::Rc;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-bluetooth";
/// Pixels around and between the widgets.
const MARGIN: i32 = 16;
const SPACING: i32 = 10;
/// Width of a dialog, in pixels.
const DIALOG_WIDTH: i32 = 380;
/// The largest passkey BlueZ takes (6 digits).
const PASSKEY_MAX: u32 = 999_999;

/// A passkey as 6 digits, with leading zeros.
pub fn passkey_text(key: u32) -> String {
    format!("{key:06}")
}

/// The passkey typed by the user, when it is 1 to 6 digits.
pub fn parse_passkey(text: &str) -> Option<u32> {
    let t = text.trim();
    if t.is_empty() || t.len() > 6 || !t.chars().all(|c| c.is_ascii_digit()) { return None; }
    t.parse().ok().filter(|k| *k <= PASSKEY_MAX)
}

/// A short name for the common service UUIDs, else the UUID.
pub fn service_name(uuid: &str) -> String {
    let short = uuid.get(4..8).unwrap_or("").to_ascii_lowercase();
    let name = match short.as_str() {
        "110a" | "110b" | "110d" => "audio (A2DP)",
        "110c" | "110e" | "110f" => "media remote control",
        "1108" | "1112" | "111e" | "111f" => "headset / hands-free",
        "1124" | "1812" => "keyboard / mouse (HID)",
        "1105" | "1106" => "file transfer",
        "1115" | "1116" => "network (PAN)",
        "112f" => "phone book",
        _ => return uuid.to_string(),
    };
    name.to_string()
}

/// The question or the instruction of a request of `kind` from `name`.
pub fn prompt(kind: &AgentKind, name: &str) -> String {
    match kind {
        AgentKind::PinCode => format!("Enter the PIN code for {name}."),
        AgentKind::Passkey => format!("Enter the passkey shown on {name}."),
        AgentKind::DisplayPinCode(pin) => format!("Type {pin} on {name}, then press Enter."),
        AgentKind::DisplayPasskey(key, _) => format!("Type {} on {name}, then press Enter.", passkey_text(*key)),
        AgentKind::Confirm(key) => format!("Does {name} show the code {}?", passkey_text(*key)),
        AgentKind::Authorize => format!("Allow {name} to pair with this computer?"),
        AgentKind::AuthorizeService(uuid) => format!("Allow {name} to use {}?", service_name(uuid)),
    }
}

/// The one open agent dialog and the device it is about.
struct Open {
    device: String,
    window: gtk::Window,
}

/// Shows the agent requests one at a time: a new request replaces the open
/// dialog (BlueZ asks one thing at a time), a Cancel closes it.
pub struct AgentHost {
    /// The window the dialogs belong to; `None` in the panel.
    parent: Option<gtk::Window>,
    open: RefCell<Option<Open>>,
}

impl AgentHost {
    /// A host for dialogs over `parent`, or top-level ones.
    pub fn new(parent: Option<gtk::Window>) -> Rc<Self> {
        Rc::new(Self { parent, open: RefCell::new(None) })
    }

    /// Show `event`; `state` gives the device names.
    pub fn on_event(self: &Rc<Self>, state: &BtState, event: AgentEvent) {
        match event {
            AgentEvent::Cancel => self.close(),
            AgentEvent::Request { device, kind, reply } => {
                self.close();
                let window = self.dialog(&state.device_name(&device), &kind, reply);
                *self.open.borrow_mut() = Some(Open { device, window });
            }
        }
    }

    /// Close the dialog of a device that is now paired (a code shown to type
    /// on a keyboard has no answer: it ends with the pairing).
    pub fn on_state(&self, state: &BtState) {
        let done = self.open.borrow().as_ref().is_some_and(|o| state.device(&o.device).is_some_and(|d| d.paired));
        if done { self.close(); }
    }

    /// Close the open dialog; an unanswered request is rejected.
    pub fn close(&self) {
        let open = self.open.borrow_mut().take();
        if let Some(o) = open { o.window.close(); }
    }

    /// Whether a dialog is open (for the tests).
    pub fn is_open(&self) -> bool {
        self.open.borrow().is_some()
    }

    /// Build and show the dialog of one request.
    fn dialog(self: &Rc<Self>, name: &str, kind: &AgentKind, reply: async_channel::Sender<AgentReply>) -> gtk::Window {
        let window = gtk::Window::builder().title("Bluetooth pairing").modal(self.parent.is_some()).resizable(false).default_width(DIALOG_WIDTH).build();
        if let Some(p) = &self.parent { window.set_transient_for(Some(p)); }
        let body = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        body.set_margin_top(MARGIN);
        body.set_margin_bottom(MARGIN);
        body.set_margin_start(MARGIN);
        body.set_margin_end(MARGIN);
        let text = gtk::Label::new(Some(&prompt(kind, name)));
        text.set_wrap(true);
        text.set_xalign(0.0);
        body.append(&text);
        // Answered once: the first button takes the sender; closing the
        // window with the sender still here rejects.
        let slot: Rc<RefCell<Option<async_channel::Sender<AgentReply>>>> = Rc::new(RefCell::new(kind.needs_reply().then_some(reply)));
        let entry = matches!(kind, AgentKind::PinCode | AgentKind::Passkey).then(gtk::Entry::new);
        if let Some(e) = &entry {
            e.set_activates_default(true);
            if *kind == AgentKind::Passkey { e.set_max_length(6); e.set_input_purpose(gtk::InputPurpose::Digits); }
            body.append(e);
        }
        if let AgentKind::DisplayPasskey(_, entered) = kind {
            let typed = gtk::Label::new(Some(&format!("Digits typed: {entered}")));
            typed.add_css_class("dim-label");
            typed.set_xalign(0.0);
            body.append(&typed);
        }
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        buttons.set_halign(gtk::Align::End);
        let labels: (&str, Option<&str>) = match kind {
            AgentKind::DisplayPinCode(_) | AgentKind::DisplayPasskey(..) => ("Close", None),
            AgentKind::Confirm(_) => ("Pair", Some("Cancel")),
            AgentKind::Authorize | AgentKind::AuthorizeService(_) => ("Allow", Some("Deny")),
            AgentKind::PinCode | AgentKind::Passkey => ("Pair", Some("Cancel")),
        };
        if let Some(no) = labels.1 {
            let b = gtk::Button::with_label(no);
            let window = window.clone();
            b.connect_clicked(move |_| window.close());
            buttons.append(&b);
        }
        let yes = gtk::Button::with_label(labels.0);
        yes.add_css_class("suggested-action");
        {
            let (window, slot, kind, entry) = (window.clone(), slot.clone(), kind.clone(), entry.clone());
            yes.connect_clicked(move |_| {
                let text = entry.as_ref().map(|e| e.text().to_string()).unwrap_or_default();
                let answer = match kind {
                    AgentKind::PinCode if text.trim().is_empty() => return,
                    AgentKind::PinCode => AgentReply::Pin(text.trim().to_string()),
                    AgentKind::Passkey => match parse_passkey(&text) {
                        Some(k) => AgentReply::Passkey(k),
                        None => return,
                    },
                    _ => AgentReply::Accept,
                };
                if let Some(tx) = slot.borrow_mut().take() {
                    if tx.send_blocking(answer).is_err() { opusdm_ipc::err_log!(LOG_TAG, "agent dialog: the request was already gone"); }
                }
                window.close();
            });
        }
        buttons.append(&yes);
        body.append(&buttons);
        window.set_child(Some(&body));
        window.set_default_widget(Some(&yes));
        {
            let weak = Rc::downgrade(self);
            let mine = window.clone();
            window.connect_close_request(move |_| {
                if let Some(tx) = slot.borrow_mut().take() {
                    if tx.send_blocking(AgentReply::Reject).is_err() { opusdm_ipc::err_log!(LOG_TAG, "agent dialog: the request was already gone"); }
                }
                // Closed by the user: forget it, unless a newer one replaced it.
                match weak.upgrade() {
                    Some(host) => {
                        let ours = host.open.borrow().as_ref().is_some_and(|o| o.window == mine);
                        if ours { host.open.borrow_mut().take(); }
                    }
                    None => opusdm_ipc::err_log!(LOG_TAG, "agent dialog closed after its host was gone"),
                }
                glib::Propagation::Proceed
            });
        }
        window.present();
        if let Some(e) = &entry { e.grab_focus(); }
        window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passkeys_have_six_digits() {
        assert_eq!(passkey_text(42), "000042");
        assert_eq!(parse_passkey(" 012345 "), Some(12345));
        assert_eq!(parse_passkey("1234567"), None);
        assert_eq!(parse_passkey("12a"), None);
        assert_eq!(parse_passkey(""), None);
    }

    #[test]
    fn prompts_name_the_device_and_code() {
        assert_eq!(prompt(&AgentKind::Confirm(7), "Pixel"), "Does Pixel show the code 000007?");
        assert_eq!(prompt(&AgentKind::DisplayPasskey(123456, 2), "Keys"), "Type 123456 on Keys, then press Enter.");
        assert_eq!(prompt(&AgentKind::AuthorizeService("0000110b-0000-1000-8000-00805f9b34fb".into()), "Buds"), "Allow Buds to use audio (A2DP)?");
    }
}
