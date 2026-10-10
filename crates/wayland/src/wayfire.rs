//! Wayfire's IPC (plugins `ipc` and `ipc-rules`): a Unix socket that speaks
//! JSON. Unlike the Wayland protocols it tells the geometry, the pid and the
//! workspace of every window, so the panel's pager can draw the real window
//! boxes under Wayfire.
//!
//! Wire format: every message is a 4 byte little-endian length followed by
//! that many bytes of JSON. A request is `{"method": ..., "data": {...}}`;
//! the reply is an object, with `"error"` when the request failed. After
//! `window-rules/events/watch` the same connection receives one object per
//! event.
//!
//! Wayfire's workspaces are a grid per output; a view's geometry is relative
//! to the workspace its output shows, so a window on another workspace has
//! coordinates outside the output. OpusDM numbers the workspaces row by row,
//! like a pager: `index = y * columns + x`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde_json::{json, Value};

/// Environment variable Wayfire sets for its children: the IPC socket path.
pub const ENV_SOCKET: &str = "WAYFIRE_SOCKET";

/// Largest message accepted from the socket, a guard against garbage.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A top-level window, as `window-rules/list-views` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub id: u32,
    pub title: String,
    pub app_id: String,
    pub pid: Option<u32>,
    pub minimized: bool,
    pub activated: bool,
    /// Shown on every workspace.
    pub sticky: bool,
    /// Window geometry without decorations, relative to the workspace its
    /// output shows.
    pub geometry: Rect,
    pub output_id: Option<u32>,
}

/// An output and its workspace grid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub id: u32,
    /// Position and size in the global layout.
    pub geometry: Rect,
    /// Workspace shown now, as (column, row).
    pub workspace: (i32, i32),
    /// Grid size, as (columns, rows).
    pub grid: (i32, i32),
}

/// Windows and outputs at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Every mapped top-level window, in Wayfire's order.
    pub views: Vec<View>,
    /// Every output; the first one sets the desktops of the pager.
    pub outputs: Vec<Output>,
}

impl State {
    /// Read windows and outputs from the running Wayfire.
    pub fn read() -> Result<State, String> {
        let views = call("window-rules/list-views", json!({}))?;
        let outputs = call("window-rules/list-outputs", json!({}))?;
        Ok(State::from_json(&views, &outputs))
    }

    /// Build a state from the replies of `list-views` and `list-outputs`.
    /// Entries that are not mapped top-level windows are left out.
    pub fn from_json(views: &Value, outputs: &Value) -> State {
        let as_list = |v: &Value| v.as_array().cloned().unwrap_or_default();
        State {
            views: as_list(views).iter().filter_map(parse_view).collect(),
            outputs: as_list(outputs).iter().filter_map(parse_output).collect(),
        }
    }

    /// The output whose grid is the pager's desktops.
    fn main_output(&self) -> Option<&Output> {
        self.outputs.first()
    }

    /// The output `view` is on, else the main one.
    fn output_of(&self, view: &View) -> Option<&Output> {
        self.outputs.iter().find(|o| Some(o.id) == view.output_id).or_else(|| self.main_output())
    }

    /// Number of desktops (the main output's grid), at least 1.
    pub fn desktop_count(&self) -> u32 {
        self.main_output().map_or(1, |o| (o.grid.0 * o.grid.1).max(1) as u32)
    }

    /// Index of the desktop the main output shows.
    pub fn current_desktop(&self) -> u32 {
        self.main_output().map_or(0, |o| index_of(o.workspace, o.grid))
    }

    /// The focused window.
    pub fn active(&self) -> Option<u32> {
        self.views.iter().find(|v| v.activated).map(|v| v.id)
    }

    /// Workspace (column, row) the centre of `view` lies on.
    fn workspace_of(&self, view: &View) -> Option<(i32, i32)> {
        let o = self.output_of(view)?;
        if o.geometry.w <= 0 || o.geometry.h <= 0 { return None; }
        let g = view.geometry;
        let dx = (g.x + g.w / 2).div_euclid(o.geometry.w);
        let dy = (g.y + g.h / 2).div_euclid(o.geometry.h);
        let col = (o.workspace.0 + dx).clamp(0, (o.grid.0 - 1).max(0));
        let row = (o.workspace.1 + dy).clamp(0, (o.grid.1 - 1).max(0));
        Some((col, row))
    }

    /// Desktop index of `view`; `None` for a sticky window (all desktops).
    pub fn desktop_of(&self, view: &View) -> Option<u32> {
        if view.sticky { return None; }
        let o = self.output_of(view)?;
        Some(index_of(self.workspace_of(view)?, o.grid))
    }

    /// Rectangle of `view` in global coordinates, as if its own workspace
    /// were shown: what a pager draws in that workspace's miniature.
    pub fn rect_of(&self, view: &View) -> Rect {
        let g = view.geometry;
        let (Some(o), Some((col, row))) = (self.output_of(view), self.workspace_of(view)) else { return g };
        let x = g.x - (col - o.workspace.0) * o.geometry.w + o.geometry.x;
        let y = g.y - (row - o.workspace.1) * o.geometry.h + o.geometry.y;
        Rect { x, y, w: g.w, h: g.h }
    }
}

/// Pager index of workspace `(col, row)` in a grid of `grid` columns/rows.
fn index_of(ws: (i32, i32), grid: (i32, i32)) -> u32 {
    (ws.1 * grid.0.max(1) + ws.0).max(0) as u32
}

/// Workspace `(col, row)` of pager index `index` in a grid of `grid`.
fn workspace_at(index: u32, grid: (i32, i32)) -> (i32, i32) {
    let cols = grid.0.max(1);
    (index as i32 % cols, index as i32 / cols)
}

/// A `{x, y, width, height}` object.
fn parse_rect(v: &Value) -> Rect {
    let n = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0) as i32;
    Rect { x: n("x"), y: n("y"), w: n("width"), h: n("height") }
}

/// One entry of `list-views`; `None` unless it is a mapped top-level window
/// (layer surfaces, popups and the like are left out).
fn parse_view(v: &Value) -> Option<View> {
    if v.get("role").and_then(Value::as_str) != Some("toplevel") { return None; }
    if v.get("mapped").and_then(Value::as_bool) != Some(true) { return None; }
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let number = |k: &str| v.get(k).and_then(Value::as_i64).filter(|n| *n > 0).map(|n| n as u32);
    Some(View {
        id: v.get("id").and_then(Value::as_u64)? as u32,
        title: text("title"),
        app_id: text("app-id"),
        pid: number("pid"),
        minimized: flag("minimized"),
        activated: flag("activated"),
        sticky: flag("sticky"),
        geometry: v.get("geometry").map(parse_rect).unwrap_or_default(),
        output_id: number("output-id"),
    })
}

/// One entry of `list-outputs`.
fn parse_output(v: &Value) -> Option<Output> {
    let ws = v.get("workspace")?;
    let n = |k: &str| ws.get(k).and_then(Value::as_i64).unwrap_or(0) as i32;
    Some(Output {
        id: v.get("id").and_then(Value::as_u64)? as u32,
        geometry: v.get("geometry").map(parse_rect).unwrap_or_default(),
        workspace: (n("x"), n("y")),
        grid: (n("grid_width").max(1), n("grid_height").max(1)),
    })
}

/// Wayfire's IPC socket, when this process runs under Wayfire.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os(ENV_SOCKET).filter(|p| !p.is_empty()).map(PathBuf::from).filter(|p| p.exists())
}

/// Whether this process runs under Wayfire with its IPC socket.
pub fn under_wayfire() -> bool {
    socket_path().is_some()
}

/// Open a new connection to the IPC socket.
fn connect() -> Result<UnixStream, String> {
    let path = socket_path().ok_or_else(|| format!("{ENV_SOCKET} is not set or the socket is missing"))?;
    UnixStream::connect(&path).map_err(|e| format!("cannot connect to {}: {e}", path.display()))
}

/// `body` framed for the socket: its length (4 bytes, little endian), then it.
fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

/// Send request `method` with `data` on `stream`.
fn send(stream: &mut UnixStream, method: &str, data: Value) -> Result<(), String> {
    let body = json!({ "method": method, "data": data }).to_string();
    stream.write_all(&frame(body.as_bytes())).map_err(|e| format!("{method}: write failed: {e}"))
}

/// Read one message from `stream`.
fn receive(stream: &mut UnixStream) -> Result<Value, String> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).map_err(|e| format!("read failed: {e}"))?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE { return Err(format!("message of {len} bytes refused")); }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).map_err(|e| format!("read failed: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("bad JSON from Wayfire: {e}"))
}

/// The reply as a result: `Err` with Wayfire's message when it has `error`.
fn checked(method: &str, reply: Value) -> Result<Value, String> {
    match reply.get("error").and_then(Value::as_str) {
        Some(e) => Err(format!("{method}: {e}")),
        None => Ok(reply),
    }
}

/// Send one request on a new connection and return the reply.
pub fn call(method: &str, data: Value) -> Result<Value, String> {
    let mut stream = connect()?;
    send(&mut stream, method, data)?;
    checked(method, receive(&mut stream)?)
}

/// Call `on_change` with the current state, then again after every event
/// Wayfire sends, from a thread of its own. Returns once the thread runs;
/// the thread stops (with a log line) when Wayfire goes away.
pub fn watch(mut on_change: impl FnMut(&State) + Send + 'static) -> Result<(), String> {
    let mut stream = connect()?;
    send(&mut stream, "window-rules/events/watch", json!({}))?;
    checked("window-rules/events/watch", receive(&mut stream)?)?;
    std::thread::Builder::new()
        .name("opusdm-wayfire".into())
        .spawn(move || {
            let mut last: Option<State> = None;
            loop {
                match State::read() {
                    Ok(state) if last.as_ref() != Some(&state) => {
                        on_change(&state);
                        last = Some(state);
                    }
                    Ok(_) => {}
                    Err(e) => opusdm_ipc::err_log!("opusdm-wayland", "Wayfire state not read: {e}"),
                }
                if let Err(e) = receive(&mut stream) {
                    opusdm_ipc::err_log!("opusdm-wayland", "Wayfire event watch stopped: {e}");
                    return;
                }
            }
        })
        .map_err(|e| format!("cannot start the Wayfire thread: {e}"))?;
    Ok(())
}

/// Raise and focus window `id`.
pub fn focus(id: u32) -> Result<(), String> {
    call("window-rules/focus-view", json!({ "id": id })).map(drop)
}

/// Minimize (or restore) window `id`.
pub fn set_minimized(id: u32, minimized: bool) -> Result<(), String> {
    call("wm-actions/set-minimized", json!({ "view_id": id, "state": minimized })).map(drop)
}

/// Show desktop `index` on the main output.
pub fn switch_desktop(index: u32) -> Result<(), String> {
    let state = State::read()?;
    let o = state.main_output().ok_or("Wayfire reports no output")?;
    let (x, y) = workspace_at(index, o.grid);
    call("vswitch/set-workspace", json!({ "x": x, "y": y, "output-id": o.id })).map(drop)
}

/// Where window `id` goes to be on desktop `index` of its output, without
/// changing the desktop shown: its geometry shifted by whole outputs.
pub fn geometry_on_desktop(state: &State, id: u32, index: u32) -> Result<Rect, String> {
    let view = state.views.iter().find(|v| v.id == id).ok_or_else(|| format!("no window {id}"))?;
    let o = state.output_of(view).ok_or("Wayfire reports no output")?;
    let from = state.workspace_of(view).ok_or("the output has no size")?;
    let to = workspace_at(index, o.grid);
    if to.0 >= o.grid.0 || to.1 >= o.grid.1 { return Err(format!("no desktop {index}")); }
    let g = view.geometry;
    Ok(Rect { x: g.x + (to.0 - from.0) * o.geometry.w, y: g.y + (to.1 - from.1) * o.geometry.h, w: g.w, h: g.h })
}

/// Move window `id` to desktop `index`; the desktop shown does not change.
pub fn move_to_desktop(id: u32, index: u32) -> Result<(), String> {
    let g = geometry_on_desktop(&State::read()?, id, index)?;
    let geometry = json!({ "x": g.x, "y": g.y, "width": g.w, "height": g.h });
    call("window-rules/configure-view", json!({ "id": id, "geometry": geometry })).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 1280x720 output on a 2x2 grid showing workspace (1, 0).
    fn outputs() -> Value {
        json!([{ "id": 1, "name": "WL-1", "geometry": { "x": 0, "y": 0, "width": 1280, "height": 720 },
                 "workspace": { "grid_width": 2, "grid_height": 2, "x": 1, "y": 0 } }])
    }

    fn view(id: u32, x: i32, y: i32) -> Value {
        json!({ "id": id, "role": "toplevel", "mapped": true, "title": "t", "app-id": "a", "pid": 42,
                "activated": id == 2, "minimized": false, "sticky": false, "output-id": 1,
                "geometry": { "x": x, "y": y, "width": 600, "height": 400 } })
    }

    #[test]
    fn only_mapped_toplevels_are_kept() {
        let views = json!([view(1, 50, 60), { "id": 5, "role": "desktop-environment", "mapped": true },
                           { "id": 6, "role": "toplevel", "mapped": false }]);
        let st = State::from_json(&views, &outputs());
        assert_eq!(st.views.len(), 1);
        assert_eq!(st.views[0].pid, Some(42));
        assert_eq!(st.views[0].geometry, Rect { x: 50, y: 60, w: 600, h: 400 });
    }

    #[test]
    fn desktops_follow_the_grid_of_the_first_output() {
        let st = State::from_json(&json!([]), &outputs());
        assert_eq!((st.desktop_count(), st.current_desktop()), (4, 1));
        assert_eq!(State::default().desktop_count(), 1);
    }

    #[test]
    fn a_window_off_the_output_is_on_another_desktop() {
        // Shown workspace (1, 0); one window on it, one left of it, one below.
        let st = State::from_json(&json!([view(1, 50, 60), view(2, -1230, 60), view(3, 50, 780)]), &outputs());
        let desktops: Vec<_> = st.views.iter().map(|v| st.desktop_of(v)).collect();
        assert_eq!(desktops, vec![Some(1), Some(0), Some(3)]);
        let rects: Vec<_> = st.views.iter().map(|v| st.rect_of(v)).collect();
        assert!(rects.iter().all(|r| (r.x, r.y) == (50, 60)));
        assert_eq!(st.active(), Some(2));
    }

    #[test]
    fn a_sticky_window_is_on_every_desktop() {
        let mut v = view(1, 50, 60);
        v["sticky"] = json!(true);
        let st = State::from_json(&json!([v]), &outputs());
        assert_eq!(st.desktop_of(&st.views[0]), None);
    }

    #[test]
    fn moving_to_a_desktop_shifts_by_whole_outputs() {
        let st = State::from_json(&json!([view(1, 50, 60)]), &outputs());
        assert_eq!(geometry_on_desktop(&st, 1, 2).unwrap(), Rect { x: -1230, y: 780, w: 600, h: 400 });
        assert_eq!(geometry_on_desktop(&st, 1, 1).unwrap(), Rect { x: 50, y: 60, w: 600, h: 400 });
        assert!(geometry_on_desktop(&st, 1, 4).is_err());
        assert!(geometry_on_desktop(&st, 9, 0).is_err());
    }

    #[test]
    fn frames_carry_a_little_endian_length() {
        assert_eq!(frame(b"{}"), vec![2, 0, 0, 0, b'{', b'}']);
    }

    #[test]
    fn an_error_reply_is_an_error() {
        assert!(checked("m", json!({ "error": "Missing \"id\"" })).is_err());
        assert!(checked("m", json!({ "result": "ok" })).is_ok());
    }
}
