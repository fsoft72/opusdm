//! The D-Bus side of the agent: polkitd calls `BeginAuthentication` when a
//! program of this session needs the password of an administrator, and
//! `CancelAuthentication` when it no longer does. Each request goes to the
//! GTK loop, which shows a dialog and answers on the request's reply channel.

use crate::identity::{self, Identities};
use std::collections::HashMap;
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, Value};

/// The bus name of polkitd.
const SERVICE: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
/// Where this agent's object lives on the system bus.
const AGENT_PATH: &str = "/org/opusdm/PolicyKit1/AuthenticationAgent";
const LOGIN_SERVICE: &str = "org.freedesktop.login1";
const LOGIN_PATH: &str = "/org/freedesktop/login1";
const LOGIN_MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const LOGIN_SESSION_IFACE: &str = "org.freedesktop.login1.Session";
/// The locale when the environment names none.
const DEFAULT_LOCALE: &str = "C";

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Authenticated,
    Cancelled,
}

/// One `BeginAuthentication`, for the dialog.
#[derive(Debug)]
pub struct Request {
    pub action_id: String,
    pub message: String,
    pub icon_name: String,
    pub details: HashMap<String, String>,
    pub cookie: String,
    /// The users who may authenticate, the current one first.
    pub users: Vec<String>,
    /// Where the dialog sends the outcome.
    pub reply: async_channel::Sender<Outcome>,
}

/// What the agent asks of the GTK loop.
#[derive(Debug)]
pub enum AgentMsg {
    Begin(Request),
    /// Close the dialog of this cookie.
    Cancel(String),
}

/// The errors an authentication agent answers with.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// Something went wrong.
    Failed(String),
    /// The user (or polkitd) cancelled the request.
    Cancelled(String),
    /// The caller is not polkitd.
    NotAuthorized(String),
}

/// The object polkitd calls.
struct Agent {
    tx: async_channel::Sender<AgentMsg>,
    /// The unique bus name of polkitd: the only caller served, or any
    /// process on the bus could open password dialogs.
    polkit_owner: String,
}

impl Agent {
    /// Refuse a call that does not come from polkitd.
    fn check_sender(&self, header: &Header<'_>) -> Result<(), AgentError> {
        match header.sender() {
            Some(sender) if sender.as_str() == self.polkit_owner => Ok(()),
            _ => Err(AgentError::NotAuthorized("Only polkitd may call the agent".to_string())),
        }
    }
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    /// Ask the user to authenticate; returns once the dialog is done.
    #[allow(clippy::too_many_arguments)]
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Identities,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.check_sender(&header)?;
        let current = identity::current_user();
        let users = identity::user_names(&identity::parse(&identities), &identity::SystemAccounts, current.as_deref());
        if users.is_empty() { return Err(AgentError::Failed("No user can authenticate this request".to_string())); }
        let (reply, outcome) = async_channel::bounded(1);
        let request = Request { action_id, message, icon_name, details, cookie, users, reply };
        self.tx.send(AgentMsg::Begin(request)).await.map_err(|_| AgentError::Failed("The agent is shutting down".to_string()))?;
        match outcome.recv().await {
            Ok(Outcome::Authenticated) => Ok(()),
            Ok(Outcome::Cancelled) | Err(_) => Err(AgentError::Cancelled("The authentication was cancelled".to_string())),
        }
    }

    /// polkitd no longer needs the request of `cookie`.
    async fn cancel_authentication(&self, cookie: String, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.check_sender(&header)?;
        self.tx.send(AgentMsg::Cancel(cookie)).await.map_err(|_| AgentError::Failed("The agent is shutting down".to_string()))
    }
}

/// The locale for polkitd's messages, from the environment.
fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_LOCALE.to_string())
}

/// The logind session this program runs in: `XDG_SESSION_ID`, or logind's
/// answer for this process.
fn session_id(conn: &Connection) -> Result<String, String> {
    if let Some(id) = std::env::var("XDG_SESSION_ID").ok().filter(|id| !id.is_empty()) {
        return Ok(id);
    }
    let manager = zbus::blocking::Proxy::new(conn, LOGIN_SERVICE, LOGIN_PATH, LOGIN_MANAGER_IFACE).map_err(|e| e.to_string())?;
    let path: OwnedObjectPath = manager
        .call("GetSessionByPID", &(std::process::id(),))
        .map_err(|e| format!("logind does not know the session of this process: {e}"))?;
    let session = zbus::blocking::Proxy::new(conn, LOGIN_SERVICE, path, LOGIN_SESSION_IFACE).map_err(|e| e.to_string())?;
    session.get_property::<String>("Id").map_err(|e| e.to_string())
}

/// Serve the agent on the system bus `conn` and register it with polkitd
/// for this login session. Requests go to `tx`. The registration ends with
/// the connection.
pub fn register(conn: &Connection, tx: async_channel::Sender<AgentMsg>) -> Result<(), String> {
    let bus = zbus::blocking::fdo::DBusProxy::new(conn).map_err(|e| e.to_string())?;
    let name = zbus::names::WellKnownName::try_from(SERVICE).map_err(|e| e.to_string())?;
    // polkitd starts on demand: start it to learn its unique name.
    bus.start_service_by_name(name.clone(), 0).map_err(|e| format!("cannot start polkitd: {e}"))?;
    let polkit_owner = bus.get_name_owner(name.into()).map_err(|e| format!("polkitd is not on the bus: {e}"))?.to_string();
    let session = session_id(conn)?;
    conn.object_server().at(AGENT_PATH, Agent { tx, polkit_owner }).map_err(|e| e.to_string())?;
    let subject = ("unix-session", HashMap::from([("session-id", Value::from(session.as_str()))]));
    let authority = zbus::blocking::Proxy::new(conn, SERVICE, AUTHORITY_PATH, AUTHORITY_IFACE).map_err(|e| e.to_string())?;
    authority
        .call::<_, _, ()>("RegisterAuthenticationAgent", &(subject, locale(), AGENT_PATH))
        .map_err(|e| format!("polkitd refused the agent for session {session}: {e}"))
}
