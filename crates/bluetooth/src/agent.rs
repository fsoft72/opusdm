//! The pairing agent (`org.bluez.Agent1`): BlueZ asks it for a PIN, a
//! passkey or a yes / no while a device pairs. Every request goes to the GTK
//! side as an [`AgentEvent`] and the D-Bus call waits for the answer. When
//! nobody can answer (the GTK side is gone, the dialog was closed) the
//! request is rejected, never left to time out.

use crate::bluez::SERVICE;
use crate::error::BtError;
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::ObjectPath;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-bluetooth";
/// Path of the agent of the panel, the default agent of the session.
pub const PANEL_AGENT_PATH: &str = "/org/opusdm/bluetooth/panel";
/// Path of the agent of the Bluetooth tool, for the pairings it starts.
pub const MANAGER_AGENT_PATH: &str = "/org/opusdm/bluetooth/manager";
/// The agent can show a code and the user can type one.
const CAPABILITY: &str = "KeyboardDisplay";
const MANAGER_PATH: &str = "/org/bluez";
const MANAGER_IFACE: &str = "org.bluez.AgentManager1";

/// What BlueZ asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentKind {
    /// Type the PIN code of the device (old devices).
    PinCode,
    /// Type the 6 digit passkey shown by the device.
    Passkey,
    /// Type this PIN on the device (a keyboard).
    DisplayPinCode(String),
    /// Type this passkey on the device; the second number is how many digits
    /// were typed so far.
    DisplayPasskey(u32, u16),
    /// Does this passkey match the one on the device (a phone)?
    Confirm(u32),
    /// May the device pair (a pairing with no code)?
    Authorize,
    /// May the device use this service (UUID)?
    AuthorizeService(String),
}

impl AgentKind {
    /// Whether BlueZ waits for an answer; the display kinds only show a code.
    pub fn needs_reply(&self) -> bool {
        !matches!(self, Self::DisplayPinCode(_) | Self::DisplayPasskey(..))
    }
}

/// The answer of the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentReply {
    Pin(String),
    Passkey(u32),
    Accept,
    Reject,
}

/// What the agent sends to the GTK side.
#[derive(Debug)]
pub enum AgentEvent {
    /// A request for the device at `device`. Answer once on `reply`; for a
    /// display kind nobody reads it.
    Request { device: String, kind: AgentKind, reply: async_channel::Sender<AgentReply> },
    /// BlueZ gave up the request in progress: close its dialog.
    Cancel,
}

/// Where to serve an agent and where its events go.
#[derive(Clone)]
pub struct AgentSetup {
    pub path: &'static str,
    /// Ask to be the default agent: it gets the pairings started by other
    /// devices. Only the panel asks.
    pub default: bool,
    pub events: async_channel::Sender<AgentEvent>,
}

/// The errors the agent answers BlueZ with.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

/// What the agent returns to BlueZ.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Done,
    Pin(String),
    Passkey(u32),
}

/// Turn the user's `reply` to a request of `kind` into the answer to BlueZ.
/// No reply (the dialog was closed, nobody listens) and a reply of the wrong
/// type reject.
pub fn answer(kind: &AgentKind, reply: Option<AgentReply>) -> Result<Answer, AgentError> {
    if !kind.needs_reply() { return Ok(Answer::Done); }
    let rejected = || AgentError::Rejected("Rejected by the user".to_string());
    match (kind, reply) {
        (AgentKind::PinCode, Some(AgentReply::Pin(pin))) => Ok(Answer::Pin(pin)),
        (AgentKind::Passkey, Some(AgentReply::Passkey(key))) => Ok(Answer::Passkey(key)),
        (AgentKind::Confirm(_) | AgentKind::Authorize | AgentKind::AuthorizeService(_), Some(AgentReply::Accept)) => Ok(Answer::Done),
        _ => Err(rejected()),
    }
}

/// Send a request to the GTK side and wait for the answer. `None` when the
/// GTK side is gone or dropped the reply; logged.
pub async fn ask(events: &async_channel::Sender<AgentEvent>, device: &str, kind: AgentKind) -> Option<AgentReply> {
    let (reply_tx, reply_rx) = async_channel::bounded(1);
    let wants_reply = kind.needs_reply();
    if events.send(AgentEvent::Request { device: device.to_string(), kind: kind.clone(), reply: reply_tx }).await.is_err() {
        opusdm_ipc::err_log!(LOG_TAG, "agent: nobody shows the {kind:?} request for {device}: rejected");
        return None;
    }
    if !wants_reply { return None; }
    let reply = reply_rx.recv().await.ok();
    if reply.is_none() { opusdm_ipc::err_log!(LOG_TAG, "agent: the {kind:?} request for {device} got no answer: rejected"); }
    reply
}

/// The object BlueZ calls.
struct Agent {
    events: async_channel::Sender<AgentEvent>,
    /// The unique bus name of BlueZ: the only caller served, or any process
    /// could pop pairing dialogs.
    bluez_owner: String,
}

impl Agent {
    /// Refuse a call that does not come from BlueZ.
    fn check_sender(&self, header: &Header<'_>) -> Result<(), AgentError> {
        match header.sender() {
            Some(sender) if sender.as_str() == self.bluez_owner => Ok(()),
            _ => {
                opusdm_ipc::err_log!(LOG_TAG, "agent: a call not from BlueZ was refused");
                Err(AgentError::Rejected("Only BlueZ may call the agent".to_string()))
            }
        }
    }

    /// Check the caller, ask the GTK side and map the reply.
    async fn handle(&self, header: &Header<'_>, device: &ObjectPath<'_>, kind: AgentKind) -> Result<Answer, AgentError> {
        self.check_sender(header)?;
        // A display kind returns at once: with nobody to show the code to,
        // the pairing fails on the device side.
        let reply = ask(&self.events, device.as_str(), kind.clone()).await;
        answer(&kind, reply)
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    /// BlueZ unregistered the agent.
    fn release(&self) {
        opusdm_ipc::err_log!(LOG_TAG, "agent: released by BlueZ");
    }

    async fn request_pin_code(&self, device: ObjectPath<'_>, #[zbus(header)] header: Header<'_>) -> Result<String, AgentError> {
        match self.handle(&header, &device, AgentKind::PinCode).await? {
            Answer::Pin(pin) => Ok(pin),
            _ => Err(AgentError::Rejected("No PIN".to_string())),
        }
    }

    async fn display_pin_code(&self, device: ObjectPath<'_>, pincode: String, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.handle(&header, &device, AgentKind::DisplayPinCode(pincode)).await.map(|_| ())
    }

    async fn request_passkey(&self, device: ObjectPath<'_>, #[zbus(header)] header: Header<'_>) -> Result<u32, AgentError> {
        match self.handle(&header, &device, AgentKind::Passkey).await? {
            Answer::Passkey(key) => Ok(key),
            _ => Err(AgentError::Rejected("No passkey".to_string())),
        }
    }

    async fn display_passkey(&self, device: ObjectPath<'_>, passkey: u32, entered: u16, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.handle(&header, &device, AgentKind::DisplayPasskey(passkey, entered)).await.map(|_| ())
    }

    async fn request_confirmation(&self, device: ObjectPath<'_>, passkey: u32, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.handle(&header, &device, AgentKind::Confirm(passkey)).await.map(|_| ())
    }

    async fn request_authorization(&self, device: ObjectPath<'_>, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.handle(&header, &device, AgentKind::Authorize).await.map(|_| ())
    }

    async fn authorize_service(&self, device: ObjectPath<'_>, uuid: String, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.handle(&header, &device, AgentKind::AuthorizeService(uuid)).await.map(|_| ())
    }

    /// BlueZ gave up the request in progress.
    async fn cancel(&self, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.check_sender(&header)?;
        if self.events.send(AgentEvent::Cancel).await.is_err() {
            opusdm_ipc::err_log!(LOG_TAG, "agent: nobody to tell about a Cancel");
        }
        Ok(())
    }
}

/// Serve the agent on `conn` and register it with BlueZ, as the default
/// agent when `setup.default`. BlueZ must be running; the registration ends
/// with the connection.
pub fn register(conn: &Connection, setup: &AgentSetup) -> Result<(), BtError> {
    let bus = zbus::blocking::fdo::DBusProxy::new(conn)?;
    let name = zbus::names::BusName::try_from(SERVICE).map_err(|e| BtError::other(e.to_string()))?;
    let bluez_owner = bus.get_name_owner(name).map_err(|e| BtError::other(e.to_string()))?.to_string();
    conn.object_server().at(setup.path, Agent { events: setup.events.clone(), bluez_owner })?;
    let path = ObjectPath::try_from(setup.path).map_err(|e| BtError::other(e.to_string()))?;
    let manager = zbus::blocking::Proxy::new(conn, SERVICE, MANAGER_PATH, MANAGER_IFACE)?;
    manager.call::<_, _, ()>("RegisterAgent", &(&path, CAPABILITY))?;
    if setup.default { manager.call::<_, _, ()>("RequestDefaultAgent", &(&path,))?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;

    #[test]
    fn display_kinds_answer_at_once() {
        assert_eq!(answer(&AgentKind::DisplayPasskey(123456, 0), None).unwrap(), Answer::Done);
        assert_eq!(answer(&AgentKind::DisplayPinCode("0000".into()), None).unwrap(), Answer::Done);
    }

    #[test]
    fn replies_answer_their_request() {
        assert_eq!(answer(&AgentKind::PinCode, Some(AgentReply::Pin("1234".into()))).unwrap(), Answer::Pin("1234".into()));
        assert_eq!(answer(&AgentKind::Passkey, Some(AgentReply::Passkey(42))).unwrap(), Answer::Passkey(42));
        assert_eq!(answer(&AgentKind::Confirm(1), Some(AgentReply::Accept)).unwrap(), Answer::Done);
        assert_eq!(answer(&AgentKind::AuthorizeService("x".into()), Some(AgentReply::Accept)).unwrap(), Answer::Done);
    }

    #[test]
    fn missing_or_wrong_reply_rejects() {
        assert!(matches!(answer(&AgentKind::Confirm(1), None), Err(AgentError::Rejected(_))));
        assert!(matches!(answer(&AgentKind::Confirm(1), Some(AgentReply::Reject)), Err(AgentError::Rejected(_))));
        assert!(matches!(answer(&AgentKind::PinCode, Some(AgentReply::Accept)), Err(AgentError::Rejected(_))));
    }

    #[test]
    fn a_dropped_receiver_rejects() {
        let (tx, rx) = async_channel::unbounded();
        drop(rx);
        assert_eq!(block_on(ask(&tx, "/d", AgentKind::Authorize)), None);
    }

    #[test]
    fn a_dropped_reply_rejects_and_a_reply_arrives() {
        let (tx, rx) = async_channel::unbounded();
        let gtk_side = std::thread::spawn(move || {
            let AgentEvent::Request { reply, .. } = rx.recv_blocking().unwrap() else { panic!("a request") };
            drop(reply);
            let AgentEvent::Request { device, kind, reply } = rx.recv_blocking().unwrap() else { panic!("a request") };
            assert_eq!((device.as_str(), kind), ("/d", AgentKind::Confirm(7)));
            reply.send_blocking(AgentReply::Accept).unwrap();
        });
        assert_eq!(block_on(ask(&tx, "/d", AgentKind::Authorize)), None);
        assert_eq!(block_on(ask(&tx, "/d", AgentKind::Confirm(7))), Some(AgentReply::Accept));
        gtk_side.join().unwrap();
    }
}
