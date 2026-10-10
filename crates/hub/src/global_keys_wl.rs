//! System-global hotkeys (F-03) on Wayland, where no client may grab keys.
//! Two ways, picked by the session:
//!
//! - **labwc** (it sets `LABWC_PID` for its children): the global hotkeys
//!   become labwc keybinds that start `opusdm-hub --run FUNCTION` (see
//!   `remote`). They live in a marked block inside `<keyboard>` of labwc's
//!   `rc.xml` (see `opusdm_wayland::labwc`), rewritten when the hotkeys
//!   change, after which labwc reloads it.
//! - **other compositors**: the `org.freedesktop.portal.GlobalShortcuts`
//!   portal (KDE Plasma, GNOME 48+, Hyprland), which asks the user once to
//!   confirm the shortcuts and then reports each press.
//!
//! When neither is there the hub logs it once and the global hotkeys stay
//! inert, as on a session without X.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use opusdm_ipc::bindings::{parse_key, Hotkey};
use opusdm_ipc::function::FunctionId;

/// Name of the OpusDM block of labwc's `rc.xml` that holds the hotkeys.
pub const BLOCK_NAME: &str = "global hotkeys";

/// Called with the function of each global hotkey pressed.
pub type OnHit = Box<dyn Fn(FunctionId) + Send + Sync>;

/// The global hotkey backend of this Wayland session.
pub enum GlobalKeysWl {
    /// Keybinds in labwc's `rc.xml`.
    Labwc { rc: PathBuf, hub: PathBuf, sock: PathBuf },
    /// The GlobalShortcuts portal; the thread rebinds what it is sent.
    Portal(mpsc::Sender<Vec<Hotkey>>),
}

impl GlobalKeysWl {
    /// Pick the backend: labwc when the hub runs under it, else the portal.
    /// `None` (logged) when there is none.
    pub fn start(on_hit: OnHit) -> Option<GlobalKeysWl> {
        if opusdm_wayland::labwc::under_labwc() {
            let hub = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("opusdm-hub"));
            return Some(GlobalKeysWl::Labwc { rc: opusdm_wayland::labwc::rc_path(), hub, sock: opusdm_ipc::sockpath::hub_sock_path() });
        }
        match portal::start(on_hit) {
            Ok(tx) => Some(GlobalKeysWl::Portal(tx)),
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-hub", "global hotkeys are not available on this Wayland session: {e}");
                None
            }
        }
    }

    /// Make `globals` the global hotkeys.
    pub fn set(&self, globals: &[Hotkey]) {
        match self {
            GlobalKeysWl::Labwc { rc, hub, sock } => {
                if let Err(e) = sync_labwc(rc, globals, hub, sock) {
                    opusdm_ipc::err_log!("opusdm-hub", "cannot write the global hotkeys into {}: {e}", rc.display());
                }
            }
            GlobalKeysWl::Portal(tx) => {
                let _ = tx.send(globals.to_vec());
            }
        }
    }
}

// --------------------------------------------------------------------------
// labwc
// --------------------------------------------------------------------------

/// A hotkey text (`Ctrl+Alt+u`) as a labwc key (`C-A-u`): `S`, `C`, `A`, `W`
/// for Shift, Ctrl, Alt, Super, then the XKB key name (the same names GDK
/// uses). `None` when the text does not parse.
pub fn labwc_key(key: &str) -> Option<String> {
    let combo = parse_key(key).ok()?;
    let mut out = String::new();
    for (on, letter) in [(combo.mods.shift, "S-"), (combo.mods.ctrl, "C-"), (combo.mods.alt, "A-"), (combo.mods.sup, "W-")] {
        if on { out.push_str(letter); }
    }
    out.push_str(&combo.name);
    Some(out)
}

/// `text` quoted for a shell-like command line (labwc splits `Execute`
/// commands that way).
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The content of the OpusDM block of `rc.xml` for `globals`: one keybind
/// per hotkey running `hub --run FUNCTION --sock SOCK`. Empty when there are
/// no global hotkeys. The second value lists the key texts that
/// do not parse, for the log.
pub fn labwc_block(globals: &[Hotkey], hub: &Path, sock: &Path) -> (String, Vec<String>) {
    let mut lines = Vec::new();
    let mut bad = Vec::new();
    for hotkey in globals {
        let Some(key) = labwc_key(&hotkey.key) else {
            bad.push(hotkey.key.clone());
            continue;
        };
        let command = format!(
            "{} --run {} --sock {}",
            shell_quote(&hub.to_string_lossy()),
            shell_quote(&hotkey.function),
            shell_quote(&sock.to_string_lossy())
        );
        lines.push(format!(
            "    <keybind key=\"{}\">\n      <action name=\"Execute\" command=\"{}\" />\n    </keybind>\n",
            opusdm_wayland::labwc::xml_escape(&key),
            opusdm_wayland::labwc::xml_escape(&command)
        ));
    }
    (lines.concat(), bad)
}

/// Bring the OpusDM block of `rc` in line with `globals` and have labwc
/// reload it when the file changed.
fn sync_labwc(rc: &Path, globals: &[Hotkey], hub: &Path, sock: &Path) -> Result<(), String> {
    use opusdm_wayland::labwc;
    let (block, bad) = labwc_block(globals, hub, sock);
    for key in bad {
        opusdm_ipc::err_log!("opusdm-hub", "global hotkey '{key}' cannot be written as a labwc keybind");
    }
    let current = labwc::read(rc)?;
    let updated = labwc::with_block(current.as_deref(), labwc::Section::Keyboard, BLOCK_NAME, &block)?;
    if labwc::write_if_changed(rc, current.as_deref(), &updated)? {
        labwc::reload()?;
    }
    Ok(())
}

// --------------------------------------------------------------------------
// GlobalShortcuts portal
// --------------------------------------------------------------------------

mod portal {
    use super::*;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

    /// The live session and what its shortcut ids run.
    #[derive(Default)]
    struct Bound {
        session: Option<OwnedObjectPath>,
        functions: HashMap<String, FunctionId>,
    }

    /// Check the portal is there and start the two threads: one binds the
    /// hotkey sets it is sent, one listens for presses.
    pub fn start(on_hit: OnHit) -> Result<mpsc::Sender<Vec<Hotkey>>, String> {
        let conn = Connection::session().map_err(|e| format!("no session bus: {e}"))?;
        register_app(&conn);
        let portal = Proxy::new(&conn, DESTINATION, PATH, INTERFACE).map_err(|e| e.to_string())?;
        portal.get_property::<u32>("version").map_err(|_| "the desktop portal offers no GlobalShortcuts".to_string())?;
        let bound = Arc::new(Mutex::new(Bound::default()));
        let activated = portal.receive_signal("Activated").map_err(|e| e.to_string())?;
        {
            let bound = bound.clone();
            std::thread::Builder::new()
                .name("global-keys-portal-listen".into())
                .spawn(move || {
                    for message in activated {
                        let Ok((session, id, _, _)) = message.body().deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>() else { continue };
                        let bound = bound.lock().unwrap_or_else(|e| e.into_inner());
                        if bound.session.as_ref() != Some(&session) { continue; }
                        if let Some(function) = bound.functions.get(&id) { on_hit(function.clone()); }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        let (tx, rx) = mpsc::channel::<Vec<Hotkey>>();
        std::thread::Builder::new()
            .name("global-keys-portal-bind".into())
            .spawn(move || {
                for globals in rx {
                    if let Err(e) = rebind(&conn, &portal, &bound, &globals) {
                        opusdm_ipc::err_log!("opusdm-hub", "cannot bind the global hotkeys through the portal: {e}");
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(tx)
    }

    /// Tell the portal which app this is (needed by some backends for a
    /// process outside a sandbox). Older portals do not have the call.
    fn register_app(conn: &Connection) {
        let options: HashMap<&str, Value> = HashMap::new();
        let _ = conn.call_method(Some(DESTINATION), PATH, Some("org.freedesktop.host.portal.Registry"), "Register", &("opusdm-hub", options));
    }

    /// Replace the session with a new one holding `globals`.
    fn rebind(conn: &Connection, portal: &Proxy, bound: &Mutex<Bound>, globals: &[Hotkey]) -> Result<(), String> {
        let old = bound.lock().unwrap_or_else(|e| e.into_inner()).session.take();
        if let Some(old) = old {
            let _ = conn.call_method(Some(DESTINATION), old.as_str(), Some("org.freedesktop.portal.Session"), "Close", &());
        }
        if globals.is_empty() { return Ok(()); }
        let created = request(conn, "CreateSession", |token| {
            let options: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(token.to_string())), ("session_handle_token", Value::from(token.to_string()))]);
            portal.call_method("CreateSession", &(options,)).map(|_| ())
        })?;
        let session = created.get("session_handle").and_then(session_path).ok_or("the portal gave no session")?;
        let (shortcuts, functions) = shortcuts_for(globals);
        let session_ref = ObjectPath::try_from(session.as_str()).map_err(|e| e.to_string())?;
        request(conn, "BindShortcuts", |token| {
            let options: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(token.to_string()))]);
            portal.call_method("BindShortcuts", &(&session_ref, &shortcuts, "", options)).map(|_| ())
        })?;
        let mut bound = bound.lock().unwrap_or_else(|e| e.into_inner());
        bound.session = Some(session);
        bound.functions = functions;
        Ok(())
    }

    /// The session handle of a `CreateSession` answer (a string or an
    /// object path, depending on the portal version).
    fn session_path(value: &OwnedValue) -> Option<OwnedObjectPath> {
        if let Ok(path) = OwnedObjectPath::try_from(value.clone()) { return Some(path); }
        let text = String::try_from(value.clone()).ok()?;
        OwnedObjectPath::try_from(text).ok()
    }

    /// The portal's shortcut list for `globals` and the function of each id.
    fn shortcuts_for(globals: &[Hotkey]) -> (Vec<(String, HashMap<&'static str, Value<'static>>)>, HashMap<String, FunctionId>) {
        let mut shortcuts = Vec::new();
        let mut functions = HashMap::new();
        for (index, hotkey) in globals.iter().enumerate() {
            let id = format!("opusdm-{index}");
            let mut props: HashMap<&'static str, Value<'static>> = HashMap::from([("description", Value::from(format!("OpusDM: {}", hotkey.function)))]);
            if let Some(trigger) = portal_trigger(&hotkey.key) { props.insert("preferred_trigger", Value::from(trigger)); }
            functions.insert(id.clone(), hotkey.function.clone());
            shortcuts.push((id, props));
        }
        (shortcuts, functions)
    }

    /// Call a portal method that answers through a `Request` object and wait
    /// for its `Response`. `call` gets the handle token to pass.
    fn request(
        conn: &Connection,
        method: &str,
        call: impl FnOnce(&str) -> zbus::Result<()>,
    ) -> Result<HashMap<String, OwnedValue>, String> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let token = format!("opusdm{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).ok_or("no unique bus name")?;
        let path = format!("{PATH}/request/{sender}/{token}");
        let request = Proxy::new(conn, DESTINATION, path, "org.freedesktop.portal.Request").map_err(|e| e.to_string())?;
        let mut responses = request.receive_signal("Response").map_err(|e| e.to_string())?;
        call(&token).map_err(|e| format!("{method}: {e}"))?;
        let message = responses.next().ok_or_else(|| format!("{method}: no answer"))?;
        let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().map_err(|e| e.to_string())?;
        if code != 0 { return Err(format!("{method} was refused or cancelled (code {code})")); }
        Ok(results)
    }
}

/// A hotkey text as a portal trigger (`CTRL+ALT+u`, the XDG shortcuts
/// format: `SHIFT`, `CTRL`, `ALT`, `LOGO`, then the XKB key name).
pub fn portal_trigger(key: &str) -> Option<String> {
    let combo = parse_key(key).ok()?;
    let mut parts: Vec<&str> = Vec::new();
    for (on, name) in [(combo.mods.shift, "SHIFT"), (combo.mods.ctrl, "CTRL"), (combo.mods.alt, "ALT"), (combo.mods.sup, "LOGO")] {
        if on { parts.push(name); }
    }
    parts.push(&combo.name);
    Some(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey(key: &str, function: &str) -> Hotkey {
        Hotkey { key: key.into(), function: function.into(), global: true }
    }

    #[test]
    fn keys_become_labwc_and_portal_names() {
        assert_eq!(labwc_key("Ctrl+Alt+u").as_deref(), Some("C-A-u"));
        assert_eq!(labwc_key("Super+Shift+Return").as_deref(), Some("S-W-Return"));
        assert_eq!(labwc_key("Ctrl+backslash").as_deref(), Some("C-backslash"));
        assert_eq!(labwc_key("Hyper+x"), None);
        assert_eq!(portal_trigger("Ctrl+Shift+space").as_deref(), Some("SHIFT+CTRL+space"));
        assert_eq!(portal_trigger("Super+e").as_deref(), Some("LOGO+e"));
    }

    #[test]
    fn the_block_runs_the_hub_with_quoted_arguments() {
        let (block, bad) = labwc_block(&[hotkey("Ctrl+Print", "exec:opusdm-shot"), hotkey("bad+key", "x")], Path::new("/opt/o dm/opusdm-hub"), Path::new("/run/h.sock"));
        assert_eq!(bad, ["bad+key"]);
        assert!(block.starts_with("    <keybind key=\"C-Print\">\n"));
        assert!(block.contains("command=\"&apos;/opt/o dm/opusdm-hub&apos; --run &apos;exec:opusdm-shot&apos; --sock &apos;/run/h.sock&apos;\""));
        assert!(block.ends_with("    </keybind>\n"));
        assert_eq!(labwc_block(&[], Path::new("h"), Path::new("s")).0, "");
    }
}
