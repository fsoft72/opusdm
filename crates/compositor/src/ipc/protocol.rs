//! Requests and events of the private IPC: what each method does to the
//! compositor state, the JSON of a window, and the events a `watch`
//! connection gets when the windows or the desktops change.

use std::sync::atomic::Ordering;

use serde::{Serialize, Serializer};
use serde_json::{json, Map, Value};
use smithay::input::keyboard::{xkb, Keycode, Keysym};

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};
use crate::wm::WindowId;

/// A window as the IPC shows it (`windows/list`, `window-added`,
/// `window-changed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowView {
    /// Stable id of the window (`WindowId`).
    pub id: u32,
    /// `xdg_toplevel.set_app_id`, empty when unset.
    pub app_id: String,
    /// `xdg_toplevel.set_title`, empty when unset.
    pub title: String,
    /// Pid of the client process, when the socket credentials have one.
    pub pid: Option<u32>,
    /// Position (in the space, or where a hidden window will show again) and
    /// size of the window geometry, as `{"x","y","width","height"}`.
    #[serde(serialize_with = "_serialize_rect")]
    pub geometry: Rect,
    /// 0-based desktop of the window.
    pub desktop: u32,
    /// True while minimized.
    pub minimized: bool,
    /// True while maximized.
    pub maximized: bool,
    /// True while fullscreen.
    pub fullscreen: bool,
    /// True when the window is the active one (the xdg `Activated` state):
    /// the last window that took the keyboard, which stays active while a
    /// layer surface (the panel, the Run window) has the keyboard.
    pub activated: bool,
}

/// Writes `rect` as `{"x","y","width","height"}`.
fn _serialize_rect<S: Serializer>(rect: &Rect, serializer: S) -> Result<S::Ok, S::Error> {
    json!({"x": rect.x, "y": rect.y, "width": rect.w, "height": rect.h}).serialize(serializer)
}

/// The JSON object of the window `v`.
pub fn window_json(v: &WindowView) -> Value {
    match serde_json::to_value(v) {
        Ok(value) => value,
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "ipc: window {} not serialized: {e}", v.id);
            json!({"id": v.id})
        }
    }
}

/// A parsed request: `{"method": ..., "data": {...}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The method name.
    pub method: String,
    /// The data object (`{}` when the request has none).
    pub data: Value,
}

/// Parses the body of one message. An error is a protocol error: the body
/// is not a JSON object with a string `method`.
pub fn parse_request(body: &[u8]) -> Result<Request, String> {
    let value: Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let Value::Object(mut object) = value else { return Err("the request is not a JSON object".to_string()) };
    let Some(Value::String(method)) = object.remove("method") else {
        return Err("the request has no string 'method'".to_string());
    };
    let data = match object.remove("data") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(data) => data,
    };
    Ok(Request { method, data })
}

/// What the connection does after the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    /// Nothing: the connection goes on with requests.
    Nothing,
    /// The connection becomes a `watch` event stream.
    Watch,
    /// The connection sent `hotkeys/set`: it is the hub from now on.
    Hub,
}

/// Runs the request `req` on `state` and returns its reply (`{"ok": true,
/// ...}` or `{"error": "..."}`, logged with `err_log!`) and what the
/// connection does next.
pub fn handle(state: &mut State, req: &Request) -> (Value, After) {
    opusdm_ipc::debug_log!(LOG_TAG, "ipc request {} {}", req.method, req.data);
    let after = match req.method.as_str() {
        "watch" => After::Watch,
        "hotkeys/set" => After::Hub,
        _ => After::Nothing,
    };
    match _run(state, req) {
        Ok(Value::Object(mut fields)) => {
            fields.insert("ok".to_string(), Value::Bool(true));
            (Value::Object(fields), after)
        }
        Ok(other) => (json!({"ok": true, "result": other}), after),
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "ipc {} refused: {e}", req.method);
            (json!({"error": e}), After::Nothing)
        }
    }
}

/// The work of `handle`: the extra fields of the reply, or the refusal.
fn _run(state: &mut State, req: &Request) -> Result<Value, String> {
    let data = &req.data;
    match req.method.as_str() {
        "windows/list" => {
            let windows: Vec<Value> = state.window_views().iter().map(window_json).collect();
            Ok(json!({"windows": windows}))
        }
        "desktops/list" => Ok(json!({"count": state.desktops.count(), "current": state.desktops.current()})),
        "window/activate" => state.activate_window(_window_id(data)?).map(|_| json!({})),
        "window/minimize" => {
            let id = _window_id(data)?;
            state.set_minimized(id, _bool(data, "state")?).map(|_| json!({}))
        }
        "window/close" => state.close_window(_window_id(data)?).map(|_| json!({})),
        "window/kill" => state.kill_window_pid(_window_id(data)?, "window/kill").map(|pid| json!({"pid": pid})),
        "window/move-to-desktop" => {
            let id = _window_id(data)?;
            state.move_to_desktop(id, _u32(data, "desktop")?).map(|_| json!({}))
        }
        "desktops/switch" => state.switch_desktop(_u32(data, "index")?).map(|_| json!({})),
        "desktops/set-count" => Ok(json!({"count": state.set_desktop_count(_u32(data, "count")?)})),
        "session/quit" => {
            opusdm_ipc::debug_log!(LOG_TAG, "session/quit: stopping the compositor");
            state.quit.store(true, Ordering::Relaxed);
            Ok(json!({}))
        }
        "watch" => Ok(json!({})),
        "hotkeys/set" => Ok(json!({"errors": state.hotkeys.set_hub(&_hotkey_entries(data)?)})),
        "reload" => Ok(json!({"errors": state.reload()})),
        "bindings/list" => {
            let bindings: Vec<Value> = state.hotkeys.list().into_iter().map(|(name, key)| json!({"name": name, "key": key})).collect();
            Ok(json!({"bindings": bindings}))
        }
        "debug/key" => _debug_key(state, _str(data, "key")?, _key_state(data)?).map(|_| json!({})),
        "debug/click" => _debug_click(state, _u32(data, "x")?, _u32(data, "y")?).map(|_| json!({})),
        "debug/pointer" => _debug_pointer(state, _f64(data, "x")?, _f64(data, "y")?).map(|_| json!({})),
        "debug/button" => _debug_button(state, _button(data)?, _pressed(data)?).map(|_| json!({})),
        "debug/foreign-toplevels" => _debug_foreign_toplevels(state),
        "debug/workspaces" => _debug_workspaces(state),
        other => Err(format!("unknown method '{other}'")),
    }
}

/// The `id` of `data` as a window id.
fn _window_id(data: &Value) -> Result<WindowId, String> {
    _u32(data, "id").map(WindowId)
}

/// The unsigned 32-bit integer `key` of `data`.
fn _u32(data: &Value, key: &str) -> Result<u32, String> {
    let value = data.get(key).ok_or_else(|| format!("missing '{key}'"))?;
    value.as_u64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| format!("'{key}' must be an unsigned integer, got {value}"))
}

/// The finite number `key` of `data`.
fn _f64(data: &Value, key: &str) -> Result<f64, String> {
    let value = data.get(key).ok_or_else(|| format!("missing '{key}'"))?;
    value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("'{key}' must be a number, got {value}"))
}

/// The Linux input code of the `"button"` of a `debug/button` request:
/// `"left"`, `"right"` or `"middle"`.
fn _button(data: &Value) -> Result<u32, String> {
    match _str(data, "button")? {
        "left" => Ok(crate::input::BTN_LEFT),
        "right" => Ok(crate::input::BTN_RIGHT),
        "middle" => Ok(crate::input::BTN_MIDDLE),
        other => Err(format!("'button' must be left, right or middle, got '{other}'")),
    }
}

/// The `"state"` of a `debug/button` request: true for `"press"`, false
/// for `"release"`.
fn _pressed(data: &Value) -> Result<bool, String> {
    match _str(data, "state")? {
        "press" => Ok(true),
        "release" => Ok(false),
        other => Err(format!("'state' must be press or release, got '{other}'")),
    }
}

/// The boolean `key` of `data`.
fn _bool(data: &Value, key: &str) -> Result<bool, String> {
    let value = data.get(key).ok_or_else(|| format!("missing '{key}'"))?;
    value.as_bool().ok_or_else(|| format!("'{key}' must be a boolean, got {value}"))
}

/// The string `key` of `data`.
fn _str<'a>(data: &'a Value, key: &str) -> Result<&'a str, String> {
    let value = data.get(key).ok_or_else(|| format!("missing '{key}'"))?;
    value.as_str().ok_or_else(|| format!("'{key}' must be a string, got {value}"))
}

/// The `hotkeys` of a `hotkeys/set` request as (key, function) pairs: an
/// array of `{"key": string, "function": string}` objects.
fn _hotkey_entries(data: &Value) -> Result<Vec<(String, String)>, String> {
    let list = data.get("hotkeys").ok_or("missing 'hotkeys'")?;
    let items = list.as_array().ok_or_else(|| format!("'hotkeys' must be an array, got {list}"))?;
    items.iter().map(|item| Ok((_str(item, "key")?.to_string(), _str(item, "function")?.to_string()))).collect()
}

/// What `debug/key` does with its key (`"state"` of the request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyStep {
    /// Presses the modifiers and the key, and releases nothing.
    Press,
    /// Releases the key, then the modifiers.
    Release,
    /// Presses and releases them (the default).
    Tap,
}

/// The `"state"` of a `debug/key` request: `"press"`, `"release"` or
/// `"tap"`; `"tap"` when it is missing.
fn _key_state(data: &Value) -> Result<KeyStep, String> {
    if data.get("state").is_none() { return Ok(KeyStep::Tap); }
    match _str(data, "state")? {
        "press" => Ok(KeyStep::Press),
        "release" => Ok(KeyStep::Release),
        "tap" => Ok(KeyStep::Tap),
        other => Err(format!("'state' must be press, release or tap, got '{other}'")),
    }
}

/// `debug/key`: with `Tap` presses the modifiers of `combo` (`Ctrl+Alt+u`),
/// presses and releases its key, then releases the modifiers; `Press` does
/// only the presses (modifiers, then the key) and `Release` only the
/// releases (the key, then the modifiers), so a key (`Alt_L`) can be held
/// across requests. All through `State::keyboard_key`, so the xkb state
/// stays right. Refused unless the compositor runs with test input
/// (`--headless`).
fn _debug_key(state: &mut State, combo: &str, step: KeyStep) -> Result<(), String> {
    if !state.test_input { return Err("debug/key is only available with --headless".to_string()); }
    let parsed = opusdm_ipc::bindings::parse_key(combo)?;
    let m = parsed.mods;
    // Key names of the held modifiers, in press order.
    let held = [("Control_L", m.ctrl), ("Alt_L", m.alt), ("Shift_L", m.shift), ("Super_L", m.sup)];
    let mut modifiers = Vec::new();
    for (name, _) in held.iter().filter(|(_, on)| *on) {
        modifiers.push(_keycode_for(state, name)?);
    }
    let key = _keycode_for(state, &parsed.normalized_name())?;
    let time = u32::try_from(state.start_time.elapsed().as_millis()).unwrap_or(u32::MAX);
    if step != KeyStep::Release {
        for code in &modifiers {
            state.keyboard_key(*code, true, time);
        }
        state.keyboard_key(key, true, time);
    }
    if step != KeyStep::Press {
        state.keyboard_key(key, false, time);
        for code in modifiers.iter().rev() {
            state.keyboard_key(*code, false, time);
        }
    }
    opusdm_ipc::debug_log!(LOG_TAG, "debug/key {combo} ({step:?}) sent");
    Ok(())
}

/// `debug/click`: moves the pointer to `(x, y)` (space coordinates), then
/// presses and releases the left button, through `State::pointer_motion`
/// and `State::pointer_button` (click focus, grabs and the client events as
/// for real input). Refused unless the compositor runs with test input
/// (`--headless`).
fn _debug_click(state: &mut State, x: u32, y: u32) -> Result<(), String> {
    if !state.test_input { return Err("debug/click is only available with --headless".to_string()); }
    let time = u32::try_from(state.start_time.elapsed().as_millis()).unwrap_or(u32::MAX);
    state.pointer_motion((f64::from(x), f64::from(y)).into(), time);
    state.pointer_button(crate::input::BTN_LEFT, true, time);
    state.pointer_button(crate::input::BTN_LEFT, false, time);
    opusdm_ipc::debug_log!(LOG_TAG, "debug/click at {x},{y} sent");
    Ok(())
}

/// `debug/pointer`: moves the pointer to `(x, y)` (space coordinates)
/// through `State::pointer_motion`, as the nested backend does with a host
/// motion: the focus, the move / resize grabs and the client events as for
/// real input. Refused unless the compositor runs with test input
/// (`--headless`).
fn _debug_pointer(state: &mut State, x: f64, y: f64) -> Result<(), String> {
    if !state.test_input { return Err("debug/pointer is only available with --headless".to_string()); }
    let time = u32::try_from(state.start_time.elapsed().as_millis()).unwrap_or(u32::MAX);
    state.pointer_motion((x, y).into(), time);
    opusdm_ipc::debug_log!(LOG_TAG, "debug/pointer to {x},{y} sent");
    Ok(())
}

/// `debug/button`: presses or releases the pointer button `button` (a
/// Linux input code) through `State::pointer_button`: click focus, Alt +
/// drag grabs, the window menu and the client events as for real input.
/// Refused unless the compositor runs with test input (`--headless`).
fn _debug_button(state: &mut State, button: u32, pressed: bool) -> Result<(), String> {
    if !state.test_input { return Err("debug/button is only available with --headless".to_string()); }
    let time = u32::try_from(state.start_time.elapsed().as_millis()).unwrap_or(u32::MAX);
    state.pointer_button(button, pressed, time);
    opusdm_ipc::debug_log!(LOG_TAG, "debug/button {button:#x} pressed {pressed} sent");
    Ok(())
}

/// `debug/foreign-toplevels`: how many foreign-toplevel managers and handles
/// the compositor keeps (the tests check that a dead client leaves none).
/// Refused unless the compositor runs with test input (`--headless`).
fn _debug_foreign_toplevels(state: &State) -> Result<Value, String> {
    if !state.test_input { return Err("debug/foreign-toplevels is only available with --headless".to_string()); }
    let (managers, handles) = state.foreign_toplevels.counts();
    Ok(json!({"managers": managers, "handles": handles}))
}

/// `debug/workspaces`: how many workspace managers and workspaces the
/// compositor keeps (the tests check that a dead client leaves none).
/// Refused unless the compositor runs with test input (`--headless`).
fn _debug_workspaces(state: &State) -> Result<Value, String> {
    if !state.test_input { return Err("debug/workspaces is only available with --headless".to_string()); }
    let (managers, workspaces) = state.workspaces.counts();
    Ok(json!({"managers": managers, "workspaces": workspaces}))
}

/// Lowest and highest xkb keycode searched for a keysym.
const KEYCODE_RANGE: (u32, u32) = (8, 255);

/// The keycode that gives the keysym named `name` (exact name first, then
/// case-insensitive) at the first level of the active layout of the
/// current keymap.
fn _keycode_for(state: &mut State, name: &str) -> Result<Keycode, String> {
    let mut sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
    if sym == Keysym::NoSymbol {
        sym = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
    }
    if sym == Keysym::NoSymbol { return Err(format!("unknown key name '{name}'")); }
    let keyboard = state.seat.get_keyboard().ok_or("the seat has no keyboard")?;
    keyboard.with_xkb_state(state, |ctx| {
        let xkb = ctx.xkb().lock().map_err(|_| "the xkb state lock is poisoned".to_string())?;
        let layout = xkb.active_layout();
        (KEYCODE_RANGE.0..=KEYCODE_RANGE.1)
            .map(Keycode::new)
            .find(|code| xkb.raw_syms_for_key_in_layout(*code, layout).contains(&sym))
            .ok_or_else(|| format!("no key of the keymap gives '{name}'"))
    })
}

/// What the `watch` connections were last told: the windows in stack order
/// and the desktops (count, current).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// The windows, in stack order.
    pub windows: Vec<WindowView>,
    /// Desktop count and current desktop.
    pub desktops: (u32, u32),
}

/// The events that turn `last` into the new state (`windows` in stack
/// order, `desktops` as (count, current)) and stores it in `last`:
/// `window-removed` first, then `window-added` / `window-changed` in stack
/// order, then `desktops-changed`.
pub fn snapshot_events(last: &mut Snapshot, windows: Vec<WindowView>, desktops: (u32, u32)) -> Vec<Value> {
    let mut events = Vec::new();
    for old in &last.windows {
        if !windows.iter().any(|w| w.id == old.id) {
            events.push(json!({"event": "window-removed", "id": old.id}));
        }
    }
    for view in &windows {
        match last.windows.iter().find(|w| w.id == view.id) {
            None => events.push(json!({"event": "window-added", "window": window_json(view)})),
            Some(old) if old != view => events.push(json!({"event": "window-changed", "window": window_json(view)})),
            Some(_) => {}
        }
    }
    if last.desktops != desktops {
        events.push(json!({"event": "desktops-changed", "count": desktops.0, "current": desktops.1}));
    }
    *last = Snapshot { windows, desktops };
    events
}

#[cfg(test)]
mod tests {
    use smithay::{
        input::keyboard::XkbConfig,
        output::{Mode, Output, PhysicalProperties, Subpixel},
        reexports::wayland_server::Display,
    };

    use super::*;

    /// A state with `desktops` desktops, an output and a keyboard, no client.
    fn state(desktops: u32) -> (Display<State>, State) {
        let display: Display<State> = Display::new().expect("display");
        let props = PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "t".into(), model: "t".into() };
        let output = Output::new("TEST-1".into(), props);
        let mode = Mode { size: (1920, 1080).into(), refresh: 60_000 };
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        let mut state = State::new(&display.handle(), output, desktops);
        state.seat.add_keyboard(XkbConfig::default(), 600, 25).expect("keyboard");
        (display, state)
    }

    /// Runs `method` with `data` on `s` and returns the reply.
    fn call(s: &mut State, method: &str, data: Value) -> Value {
        handle(s, &Request { method: method.to_string(), data }).0
    }

    /// A window view with the id `id` and the title `title`.
    fn view(id: u32, title: &str) -> WindowView {
        WindowView {
            id,
            app_id: "app".into(),
            title: title.into(),
            pid: Some(7),
            geometry: Rect { x: 1, y: 2, w: 3, h: 4 },
            desktop: 0,
            minimized: false,
            maximized: false,
            fullscreen: false,
            activated: false,
        }
    }

    #[test]
    fn requests_are_parsed_and_bad_ones_are_protocol_errors() {
        let req = parse_request(br#"{"method":"desktops/switch","data":{"index":1}}"#).unwrap();
        assert_eq!(req, Request { method: "desktops/switch".into(), data: json!({"index": 1}) });
        assert_eq!(parse_request(br#"{"method":"windows/list"}"#).unwrap().data, json!({}));
        assert!(parse_request(b"hello").is_err());
        assert!(parse_request(b"[1]").is_err());
        assert!(parse_request(br#"{"method":3}"#).is_err());
    }

    #[test]
    fn the_window_json_has_the_documented_keys() {
        let v = window_json(&view(5, "t"));
        assert_eq!(v["geometry"], json!({"x": 1, "y": 2, "width": 3, "height": 4}));
        for key in ["id", "app_id", "title", "pid", "desktop", "minimized", "maximized", "fullscreen", "activated"] {
            assert!(v.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn desktops_are_listed_switched_and_counted() {
        let (_d, mut s) = state(4);
        assert_eq!(call(&mut s, "desktops/list", json!({})), json!({"ok": true, "count": 4, "current": 0}));
        assert_eq!(call(&mut s, "desktops/switch", json!({"index": 2})), json!({"ok": true}));
        assert_eq!(call(&mut s, "desktops/set-count", json!({"count": 99})), json!({"ok": true, "count": 16}));
        assert_eq!(call(&mut s, "desktops/list", json!({}))["current"], 2);
    }

    #[test]
    fn refusals_are_error_replies() {
        let (_d, mut s) = state(4);
        for (method, data) in [
            ("bogus", json!({})),
            ("window/activate", json!({"id": 3})),
            ("window/minimize", json!({"id": 3})),
            ("window/kill", json!({"id": 3})),
            ("desktops/switch", json!({"index": -1})),
            ("desktops/switch", json!({"index": 4})),
        ] {
            let reply = call(&mut s, method, data);
            assert!(reply["error"].is_string(), "{method}: {reply}");
        }
    }

    #[test]
    fn debug_key_needs_test_input() {
        let (_d, mut s) = state(4);
        assert!(call(&mut s, "debug/key", json!({"key": "a"}))["error"].is_string());
        s.test_input = true;
        assert_eq!(call(&mut s, "debug/key", json!({"key": "Ctrl+Alt+u"})), json!({"ok": true}));
        assert!(call(&mut s, "debug/key", json!({"key": "Ctrl+NoSuchKey"}))["error"].is_string());
        let keyboard = s.seat.get_keyboard().unwrap();
        assert!(keyboard.pressed_keys().is_empty(), "every key is released");
        assert!(!keyboard.modifier_state().ctrl && !keyboard.modifier_state().alt);
    }

    #[test]
    fn debug_click_needs_test_input() {
        let (_d, mut s) = state(4);
        assert!(call(&mut s, "debug/click", json!({"x": 10, "y": 10}))["error"].is_string());
        s.test_input = true;
        assert_eq!(call(&mut s, "debug/click", json!({"x": 10, "y": 10})), json!({"ok": true}));
        assert!(call(&mut s, "debug/click", json!({"x": -1, "y": 10}))["error"].is_string());
    }

    #[test]
    fn debug_pointer_and_debug_button_need_test_input() {
        let (_d, mut s) = state(4);
        s.seat.add_pointer();
        assert!(call(&mut s, "debug/pointer", json!({"x": 10, "y": 10}))["error"].is_string());
        assert!(call(&mut s, "debug/button", json!({"button": "left", "state": "press"}))["error"].is_string());
        s.test_input = true;
        assert_eq!(call(&mut s, "debug/pointer", json!({"x": 10.5, "y": -3})), json!({"ok": true}));
        assert_eq!(call(&mut s, "debug/button", json!({"button": "middle", "state": "press"})), json!({"ok": true}));
        assert_eq!(call(&mut s, "debug/button", json!({"button": "middle", "state": "release"})), json!({"ok": true}));
        assert!(call(&mut s, "debug/button", json!({"button": "left", "state": "tap"}))["error"].is_string());
    }

    #[test]
    fn hotkeys_set_replaces_the_table_and_reports_conflicts() {
        let (_d, mut s) = state(4);
        let data = json!({"hotkeys": [{"key": "Alt+space", "function": "run"}, {"key": "Ctrl+backslash", "function": "terminal"}]});
        let (reply, after) = handle(&mut s, &Request { method: "hotkeys/set".into(), data });
        assert_eq!(after, After::Hub);
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["errors"], json!(["hotkey Alt+space: run conflicts with the built-in window menu, the built-in wins"]));
        let ctrl = opusdm_ipc::bindings::Mods { ctrl: true, ..Default::default() };
        assert_eq!(s.hotkeys.lookup(ctrl, "backslash"), Some(crate::hotkeys::Binding::Hub("terminal".into())));
        for bad in [json!({}), json!({"hotkeys": 3}), json!({"hotkeys": [{"key": "a"}]})] {
            let (reply, after) = handle(&mut s, &Request { method: "hotkeys/set".into(), data: bad });
            assert!(reply["error"].is_string() && after == After::Nothing, "{reply}");
        }
    }

    #[test]
    fn a_matched_key_release_is_swallowed_even_after_its_modifier() {
        let (_d, mut s) = state(4);
        s.test_input = true;
        s.hotkeys.set_hub(&[("Ctrl+backslash".into(), "terminal".into())]);
        let ctrl = _keycode_for(&mut s, "Control_L").unwrap();
        let key = _keycode_for(&mut s, "backslash").unwrap();
        s.keyboard_key(ctrl, true, 0);
        s.keyboard_key(key, true, 0);
        assert!(s.swallowed_keys.contains(&key), "the matched press is recorded");
        s.keyboard_key(ctrl, false, 0);
        s.keyboard_key(key, false, 0);
        assert!(s.swallowed_keys.is_empty(), "its release is swallowed and forgotten");
        let keyboard = s.seat.get_keyboard().unwrap();
        assert!(keyboard.pressed_keys().is_empty());
        s.keyboard_key(key, true, 0);
        assert!(s.swallowed_keys.is_empty(), "backslash without Ctrl is not a hotkey");
        s.keyboard_key(key, false, 0);
    }

    #[test]
    fn a_hotkey_written_with_the_shifted_symbol_matches() {
        let (_d, mut s) = state(4);
        s.test_input = true;
        assert!(s.hotkeys.set_hub(&[("Ctrl+Shift+exclam".into(), "shifted".into())]).is_empty());
        let one = _keycode_for(&mut s, "1").unwrap();
        assert_eq!(call(&mut s, "debug/key", json!({"key": "Ctrl+Shift+1"})), json!({"ok": true}));
        assert!(s.swallowed_keys.is_empty(), "press and release swallowed and forgotten");
        s.hotkeys.set_hub(&[("Ctrl+Shift+1".into(), "base".into())]);
        let ctrl = _keycode_for(&mut s, "Control_L").unwrap();
        let shift = _keycode_for(&mut s, "Shift_L").unwrap();
        for code in [ctrl, shift, one] {
            s.keyboard_key(code, true, 0);
        }
        assert!(s.swallowed_keys.contains(&one), "the base keysym matches first");
        for code in [one, shift, ctrl] {
            s.keyboard_key(code, false, 0);
        }
        assert!(s.swallowed_keys.is_empty());
        s.hotkeys.set_hub(&[("Ctrl+Shift+exclam".into(), "shifted".into())]);
        for code in [ctrl, shift, one] {
            s.keyboard_key(code, true, 0);
        }
        assert!(s.swallowed_keys.contains(&one), "the modified keysym matches when the base one does not");
        for code in [one, shift, ctrl] {
            s.keyboard_key(code, false, 0);
        }
    }

    #[test]
    fn the_quit_binding_sets_the_quit_flag() {
        let (_d, mut s) = state(4);
        s.test_input = true;
        assert_eq!(call(&mut s, "debug/key", json!({"key": "Ctrl+Alt+BackSpace"})), json!({"ok": true}));
        assert!(s.quit.load(Ordering::Relaxed));
        assert!(s.swallowed_keys.is_empty());
    }

    #[test]
    fn session_quit_sets_the_quit_flag_and_watch_turns_the_connection() {
        let (_d, mut s) = state(4);
        let (reply, after) = handle(&mut s, &Request { method: "session/quit".into(), data: json!({}) });
        assert_eq!((reply, after), (json!({"ok": true}), After::Nothing));
        assert!(s.quit.load(Ordering::Relaxed));
        let (reply, after) = handle(&mut s, &Request { method: "watch".into(), data: json!({}) });
        assert_eq!((reply, after), (json!({"ok": true}), After::Watch));
    }

    #[test]
    fn snapshot_events_report_each_difference_once() {
        let mut last = Snapshot { windows: vec![view(1, "a"), view(2, "b")], desktops: (4, 0) };
        let events = snapshot_events(&mut last, vec![view(2, "B"), view(3, "c")], (4, 1));
        let names: Vec<&str> = events.iter().filter_map(|e| e["event"].as_str()).collect();
        assert_eq!(names, ["window-removed", "window-changed", "window-added", "desktops-changed"]);
        assert_eq!(events[0]["id"], 1);
        assert_eq!(events[1]["window"]["title"], "B");
        assert_eq!(events[2]["window"]["id"], 3);
        assert_eq!((events[3]["count"].clone(), events[3]["current"].clone()), (json!(4), json!(1)));
        assert!(snapshot_events(&mut last, vec![view(2, "B"), view(3, "c")], (4, 1)).is_empty());
    }
}
