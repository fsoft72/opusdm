//! Checking the Secret Service: which collection apps use, and whether it is
//! usable. The check talks to a [`Vault`], so it is tested with a fake and
//! run for real against the session bus by [`SecretServiceVault`].

use crate::health::{Health, Reason};
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, Proxy};
use zbus::names::BusName;
use zbus::zvariant::OwnedObjectPath;

/// Tag used in log lines.
const TAG: &str = "opusdm-keyring-watch";

/// The error text a [`Vault`] returns when a call took too long.
pub const TIMEOUT_ERROR: &str = "timeout";

/// How long one whole check may take.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The well known name of the Secret Service.
const SERVICE_NAME: &str = "org.freedesktop.secrets";
/// The object path of the Secret Service.
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
/// The Secret Service interface.
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
/// The collection interface.
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
/// The alias apps use to find the keyring.
const DEFAULT_ALIAS: &str = "default";
/// The label of the keyring that the login unlocks.
const LOGIN_LABEL: &str = "login";
/// The path `ReadAlias` returns when the alias is not set.
const NO_OBJECT: &str = "/";

/// What the check needs to know about the Secret Service.
pub trait Vault {
    /// Whether the session bus can be reached.
    fn bus_ok(&self) -> bool;
    /// Whether somebody owns `org.freedesktop.secrets` (the service may be
    /// started by bus activation first).
    fn service_present(&self) -> Result<bool, String>;
    /// Whether the collection behind the `default` alias is locked; `None`
    /// when there is no such collection.
    fn default_locked(&self) -> Result<Option<bool>, String>;
    /// Whether the collection labelled `login` is locked, when it is a
    /// different one from the default; `None` when there is no such one.
    fn login_locked_if_distinct(&self) -> Result<Option<bool>, String>;
}

/// The health of `v`: bus, service, default collection, locked, then the
/// `login` collection when it is a different one.
pub fn evaluate(v: &dyn Vault) -> Health {
    if !v.bus_ok() { return Health::Unhealthy(Reason::NoBus); }
    match _evaluate_calls(v) {
        Ok(health) => health,
        Err(e) if e == TIMEOUT_ERROR => Health::Unhealthy(Reason::Timeout),
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "the Secret Service check failed: {e}");
            Health::Unhealthy(Reason::NoService)
        }
    }
}

/// The part of [`evaluate`] that can fail with a vault error.
fn _evaluate_calls(v: &dyn Vault) -> Result<Health, String> {
    if !v.service_present()? { return Ok(Health::Unhealthy(Reason::NoService)); }
    match v.default_locked()? {
        None => return Ok(Health::Unhealthy(Reason::NoCollection)),
        Some(true) => return Ok(Health::Unhealthy(Reason::Locked)),
        Some(false) => {}
    }
    if v.login_locked_if_distinct()? == Some(true) { return Ok(Health::Unhealthy(Reason::Locked)); }
    Ok(Health::Healthy)
}

/// Run `check` on a worker thread and wait `timeout` for its answer. A check
/// that does not answer in time is [`Reason::Timeout`]; its thread is left to
/// finish by itself, it never blocks the caller.
pub fn run_with_timeout(check: impl FnOnce() -> Health + Send + 'static, timeout: Duration) -> Health {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("opusdm-keyring-probe".into()).spawn(move || {
        // The receiver is gone when the caller timed out: nothing to report.
        let _ = tx.send(check());
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!(TAG, "cannot start the check thread: {e}");
        return Health::Unhealthy(Reason::Timeout);
    }
    match rx.recv_timeout(timeout) {
        Ok(health) => health,
        Err(_) => {
            opusdm_ipc::err_log!(TAG, "the Secret Service check did not answer in {timeout:?}");
            Health::Unhealthy(Reason::Timeout)
        }
    }
}

/// One check against the real session bus, with [`PROBE_TIMEOUT`].
pub fn probe_with_timeout(timeout: Duration) -> Health {
    run_with_timeout(|| evaluate(&SecretServiceVault::new()), timeout)
}

/// The Secret Service on the session bus. It only reads properties, so no
/// encrypted session is needed and a locked keyring never shows a prompt.
pub struct SecretServiceVault {
    /// The session bus, or why it could not be reached.
    conn: Result<Connection, String>,
}

impl SecretServiceVault {
    /// Connect to the session bus.
    pub fn new() -> Self {
        let conn = Connection::session().map_err(|e| format!("session bus: {e}"));
        if let Err(e) = &conn { opusdm_ipc::err_log!(TAG, "{e}"); }
        SecretServiceVault { conn }
    }

    /// The connection, or the error to report.
    fn _conn(&self) -> Result<&Connection, String> {
        self.conn.as_ref().map_err(|e| e.clone())
    }

    /// A proxy for `path` and `iface` of the Secret Service.
    fn _proxy(&self, path: &str, iface: &str) -> Result<Proxy<'static>, String> {
        Proxy::new(self._conn()?, SERVICE_NAME, path.to_owned(), iface.to_owned()).map_err(_call_error)
    }

    /// The collections the service lists.
    fn _collections(&self) -> Result<Vec<OwnedObjectPath>, String> {
        self._proxy(SERVICE_PATH, SERVICE_IFACE)?.get_property("Collections").map_err(_call_error)
    }

    /// The collection behind the `default` alias, falling back to the one
    /// labelled `login`.
    fn _default_path(&self) -> Result<Option<OwnedObjectPath>, String> {
        let alias: OwnedObjectPath = self._proxy(SERVICE_PATH, SERVICE_IFACE)?.call("ReadAlias", &(DEFAULT_ALIAS,)).map_err(_call_error)?;
        if alias.as_str() != NO_OBJECT { return Ok(Some(alias)); }
        self._login_path()
    }

    /// The collection labelled `login`, if any.
    fn _login_path(&self) -> Result<Option<OwnedObjectPath>, String> {
        for path in self._collections()? {
            let label: String = self._proxy(path.as_str(), COLLECTION_IFACE)?.get_property("Label").map_err(_call_error)?;
            if label == LOGIN_LABEL { return Ok(Some(path)); }
        }
        Ok(None)
    }

    /// Whether the collection at `path` is locked.
    fn _locked(&self, path: &OwnedObjectPath) -> Result<bool, String> {
        self._proxy(path.as_str(), COLLECTION_IFACE)?.get_property("Locked").map_err(_call_error)
    }
}

impl Default for SecretServiceVault {
    /// Same as [`SecretServiceVault::new`].
    fn default() -> Self { Self::new() }
}

impl Vault for SecretServiceVault {
    /// See [`Vault::bus_ok`].
    fn bus_ok(&self) -> bool { self.conn.is_ok() }

    /// See [`Vault::service_present`]; asks the bus to start the service
    /// when nobody owns the name.
    fn service_present(&self) -> Result<bool, String> {
        let bus = DBusProxy::new(self._conn()?).map_err(_call_error)?;
        let name = BusName::try_from(SERVICE_NAME).map_err(|e| e.to_string())?;
        if bus.name_has_owner(name.clone()).map_err(_call_error)? { return Ok(true); }
        let wk = zbus::names::WellKnownName::try_from(SERVICE_NAME).map_err(|e| e.to_string())?;
        match bus.start_service_by_name(wk, 0) {
            Ok(_) => Ok(true),
            Err(e) => {
                opusdm_ipc::err_log!(TAG, "nobody owns {SERVICE_NAME} and it cannot be started: {e}");
                Ok(false)
            }
        }
    }

    /// See [`Vault::default_locked`].
    fn default_locked(&self) -> Result<Option<bool>, String> {
        match self._default_path()? {
            Some(path) => self._locked(&path).map(Some),
            None => Ok(None),
        }
    }

    /// See [`Vault::login_locked_if_distinct`].
    fn login_locked_if_distinct(&self) -> Result<Option<bool>, String> {
        let Some(login) = self._login_path()? else { return Ok(None) };
        if self._default_path()?.as_ref() == Some(&login) { return Ok(None); }
        self._locked(&login).map(Some)
    }
}

/// A bus error as one line.
fn _call_error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault whose answers are set by the test.
    struct FakeVault {
        bus: bool,
        service: Result<bool, String>,
        default: Result<Option<bool>, String>,
        login: Result<Option<bool>, String>,
    }

    impl FakeVault {
        /// A fine vault.
        fn fine() -> Self {
            FakeVault { bus: true, service: Ok(true), default: Ok(Some(false)), login: Ok(None) }
        }
    }

    impl Vault for FakeVault {
        fn bus_ok(&self) -> bool { self.bus }
        fn service_present(&self) -> Result<bool, String> { self.service.clone() }
        fn default_locked(&self) -> Result<Option<bool>, String> { self.default.clone() }
        fn login_locked_if_distinct(&self) -> Result<Option<bool>, String> { self.login.clone() }
    }

    #[test]
    fn healthy_when_all_is_unlocked() {
        assert_eq!(evaluate(&FakeVault::fine()), Health::Healthy);
    }

    #[test]
    fn no_bus_comes_first() {
        let v = FakeVault { bus: false, service: Ok(false), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Unhealthy(Reason::NoBus));
    }

    #[test]
    fn no_service_is_reported_before_collections() {
        let v = FakeVault { service: Ok(false), default: Ok(None), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Unhealthy(Reason::NoService));
    }

    #[test]
    fn no_default_collection() {
        let v = FakeVault { default: Ok(None), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Unhealthy(Reason::NoCollection));
    }

    #[test]
    fn locked_default() {
        let v = FakeVault { default: Ok(Some(true)), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Unhealthy(Reason::Locked));
    }

    #[test]
    fn distinct_locked_login_is_unhealthy() {
        let v = FakeVault { login: Ok(Some(true)), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Unhealthy(Reason::Locked));
    }

    #[test]
    fn distinct_unlocked_login_is_healthy() {
        let v = FakeVault { login: Ok(Some(false)), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Healthy);
    }

    #[test]
    fn vault_error_is_logged_not_swallowed() {
        let v = FakeVault { default: Err("boom".into()), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Unhealthy(Reason::NoService));
    }

    #[test]
    fn timeout_error_is_a_timeout() {
        let v = FakeVault { service: Err(TIMEOUT_ERROR.into()), ..FakeVault::fine() };
        assert_eq!(evaluate(&v), Health::Unhealthy(Reason::Timeout));
    }

    #[test]
    fn slow_probe_times_out() {
        let slow = || {
            std::thread::sleep(Duration::from_millis(500));
            Health::Healthy
        };
        assert_eq!(run_with_timeout(slow, Duration::from_millis(20)), Health::Unhealthy(Reason::Timeout));
    }

    #[test]
    fn fast_probe_answers() {
        assert_eq!(run_with_timeout(|| Health::Healthy, Duration::from_secs(5)), Health::Healthy);
    }

    /// Manual check against the real session bus:
    /// `cargo test -p opusdm-keyring-watch live_probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_probe() {
        println!("{:?}", probe_with_timeout(PROBE_TIMEOUT));
    }
}
