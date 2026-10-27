//! The secondary windows of one applet (`opus.ui.openWindow`).
//!
//! Ownership for the whole run: the [`SecondaryWindows`] of an applet is a
//! field of that applet, so it lives exactly as long as the applet: each
//! window and its built tree are owned by the map here. A window's
//! `close-request` handler holds only what the host gives it (a `Weak` of the
//! applet) and the widgets hold the runtime's dispatcher (a `Weak` of the
//! runtime), so there is no cycle. When the applet is dropped (disable,
//! reload) `Drop` disconnects the handlers and destroys the windows, and no
//! JavaScript is called. A window closed by the user is taken out of the map
//! by [`SecondaryWindows::take_closed_by_user`], which hands back the
//! `onClose` handler id for the host to dispatch; a window closed by the
//! script ([`SecondaryWindows::close`]) never dispatches it.

use crate::ui_tree::{self, Node};
use crate::widgets::{self, BuiltTree, Dispatch};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{debug_log, err_log};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Most secondary windows one applet may have open at once.
pub const MAX_SECONDARY_WINDOWS: usize = 8;

/// Width of a secondary window when the script gives none.
const DEFAULT_WIDTH: i32 = 400;

/// Height of a secondary window when the script gives none.
const DEFAULT_HEIGHT: i32 = 300;

/// A validated `opus.ui.openWindow` request.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenSpec {
    /// The id the script's handle uses (`w1`, `w2`, ...).
    pub window: String,
    /// The window title.
    pub title: String,
    /// The default size in pixels.
    pub size: (i32, i32),
    /// The widget tree.
    pub node: Node,
    /// The handler to call when the user closes the window.
    pub on_close: Option<u32>,
}

impl OpenSpec {
    /// Read the payload of the `ui.window.open` op: `window` (non-empty
    /// string), `title` (string, default `default_title`), `width` and
    /// `height` (1 to [`MAX_SIZE`], default 400 x 300), `tree` and `onClose`
    /// (`{ "handler": <id> }` or missing). A tree with a canvas is refused:
    /// a canvas needs the main window's redraw path.
    pub fn from_json(payload: &Value, default_title: &str) -> Result<OpenSpec, String> {
        let window = match payload.get("window").and_then(Value::as_str) {
            Some(window) if !window.is_empty() => window.to_string(),
            _ => return Err("opus.ui.openWindow: internal: missing window id".to_string()),
        };
        let title = match payload.get("title") {
            None | Some(Value::Null) => default_title.to_string(),
            Some(value) => ui_tree::text_value("openWindow", "title", value)?,
        };
        let width = payload.get("width").filter(|v| !v.is_null()).map(|v| ui_tree::size_value("openWindow", "width", v, 1)).transpose()?.unwrap_or(DEFAULT_WIDTH);
        let height = payload.get("height").filter(|v| !v.is_null()).map(|v| ui_tree::size_value("openWindow", "height", v, 1)).transpose()?.unwrap_or(DEFAULT_HEIGHT);
        let node = ui_tree::parse(payload.get("tree").unwrap_or(&Value::Null)).map_err(|e| format!("opus.ui.openWindow: {e}"))?;
        if has_canvas(&node) { return Err("opus.ui.openWindow: a canvas is not supported in a secondary window".to_string()) }
        let on_close = ui_tree::handler_value("openWindow", "onClose", payload.get("onClose").unwrap_or(&Value::Null))?;
        Ok(OpenSpec { window, title, size: (width, height), node, on_close })
    }
}

/// Whether `node` or a descendant is a canvas.
fn has_canvas(node: &Node) -> bool {
    match &node.kind {
        ui_tree::Kind::Canvas { .. } => true,
        ui_tree::Kind::Box { children, .. } => children.iter().any(has_canvas),
        _ => false,
    }
}

/// One open secondary window.
struct Secondary {
    window: gtk::Window,
    /// Kept for `opus.ui` updates of this window; its widgets are the child of `window`.
    tree: BuiltTree,
    /// The script's `onClose` handler id.
    on_close: Option<u32>,
    /// The `close-request` handler, disconnected when the script closes the window.
    close_signal: glib::SignalHandlerId,
}

/// The secondary windows of one applet (see the module doc).
pub struct SecondaryWindows {
    /// The applet id, for log lines.
    applet: String,
    app: gtk::Application,
    /// Present the windows; tests build them without showing them.
    present: bool,
    windows: RefCell<HashMap<String, Secondary>>,
}

impl SecondaryWindows {
    /// No window yet; windows are added to `app` and shown when `present`.
    pub fn new(applet: &str, app: &gtk::Application, present: bool) -> SecondaryWindows {
        SecondaryWindows { applet: applet.to_string(), app: app.clone(), present, windows: RefCell::default() }
    }

    /// Build and show the window of `spec`. `on_user_close` is called with
    /// the window id when the user closes it (it must hold only weak
    /// references and take the window out with [`Self::take_closed_by_user`]).
    /// The window is not kept on an error.
    pub fn open(&self, spec: OpenSpec, dispatch: Rc<dyn Dispatch>, on_user_close: impl Fn(&str) -> glib::Propagation + 'static) -> Result<(), String> {
        let OpenSpec { window: id, title, size, node, on_close } = spec;
        if self.windows.borrow().contains_key(&id) { return Err(format!("applet {}: secondary window {id} is already open", self.applet)) }
        if self.windows.borrow().len() >= MAX_SECONDARY_WINDOWS {
            return Err(format!("applet {}: at most {MAX_SECONDARY_WINDOWS} secondary windows can be open", self.applet));
        }
        let tree = widgets::build(&node, dispatch).map_err(|e| format!("opus.ui.openWindow: {e}"))?;
        let window = gtk::Window::builder().application(&self.app).title(&title).default_width(size.0).default_height(size.1).build();
        window.set_child(Some(&tree.root));
        let window_id = id.clone();
        let close_signal = window.connect_close_request(move |_| on_user_close(&window_id));
        if self.present { window.present(); }
        debug_log!(APP_TAG, "applet {}: secondary window {id} opened", self.applet);
        self.windows.borrow_mut().insert(id, Secondary { window, tree, on_close, close_signal });
        Ok(())
    }

    /// Apply `props` to the widget `widget_id` of window `window`. An
    /// unknown window (already closed) is an error and is logged.
    pub fn update(&self, window: &str, widget_id: &str, props: &Value) -> Result<(), String> {
        let windows = self.windows.borrow();
        let Some(secondary) = windows.get(window) else {
            let message = format!("applet {}: update of secondary window {window}, which is not open", self.applet);
            err_log!(APP_TAG, "{message}");
            return Err(message);
        };
        widgets::apply_update(&secondary.tree, widget_id, props)
    }

    /// Close window `window` for the script: no `onClose` is dispatched.
    /// `false` (logged) when no such window is open.
    pub fn close(&self, window: &str) -> bool {
        let taken = self.windows.borrow_mut().remove(window);
        let Some(secondary) = taken else {
            err_log!(APP_TAG, "applet {}: close of secondary window {window}, which is not open", self.applet);
            return false;
        };
        secondary.window.disconnect(secondary.close_signal);
        secondary.window.destroy();
        debug_log!(APP_TAG, "applet {}: secondary window {window} closed by the script", self.applet);
        true
    }

    /// The user closed window `window`: forget it and give back its
    /// `onClose` handler id (the outer `None`, logged, when no such window is
    /// open). GTK destroys the window itself after the close request.
    pub fn take_closed_by_user(&self, window: &str) -> Option<Option<u32>> {
        let taken = self.windows.borrow_mut().remove(window);
        let Some(secondary) = taken else {
            err_log!(APP_TAG, "applet {}: secondary window {window} closed by the user, but it is not open", self.applet);
            return None;
        };
        debug_log!(APP_TAG, "applet {}: secondary window {window} closed by the user", self.applet);
        Some(secondary.on_close)
    }

    /// The ids of the open windows, sorted.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.windows.borrow().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// The window `window`, if open.
    pub fn window(&self, window: &str) -> Option<gtk::Window> {
        self.windows.borrow().get(window).map(|s| s.window.clone())
    }
}

impl Drop for SecondaryWindows {
    /// The applet is going: disconnect the close handlers (the host may be
    /// half gone) and destroy every window.
    fn drop(&mut self) {
        for (id, secondary) in self.windows.get_mut().drain() {
            secondary.window.disconnect(secondary.close_signal);
            secondary.window.destroy();
            debug_log!(APP_TAG, "applet {}: secondary window {id} closed with the applet", self.applet);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A minimal valid payload for window `w1`.
    fn payload() -> Value {
        json!({ "window": "w1", "tree": { "type": "label", "text": "x" } })
    }

    #[test]
    fn defaults_come_from_the_applet_and_the_constants() {
        let spec = OpenSpec::from_json(&payload(), "My Applet").unwrap();
        assert_eq!((spec.window.as_str(), spec.title.as_str(), spec.size, spec.on_close), ("w1", "My Applet", (DEFAULT_WIDTH, DEFAULT_HEIGHT), None));
    }

    #[test]
    fn title_size_and_on_close_are_read() {
        let mut p = payload();
        p["title"] = json!("Detail");
        p["width"] = json!(250.0);
        p["height"] = json!(120);
        p["onClose"] = json!({ "handler": 7 });
        let spec = OpenSpec::from_json(&p, "x").unwrap();
        assert_eq!((spec.title.as_str(), spec.size, spec.on_close), ("Detail", (250, 120), Some(7)));
    }

    #[test]
    fn bad_requests_are_errors_naming_the_problem() {
        let mut p = payload();
        p["width"] = json!(0);
        assert!(OpenSpec::from_json(&p, "x").unwrap_err().contains("width"));
        let mut p = payload();
        p["tree"] = json!({ "type": "gauge" });
        assert!(OpenSpec::from_json(&p, "x").unwrap_err().contains("gauge"));
        let mut p = payload();
        p["onClose"] = json!("fn");
        assert!(OpenSpec::from_json(&p, "x").unwrap_err().contains("onClose"));
        assert!(OpenSpec::from_json(&json!({ "tree": { "type": "label", "text": "x" } }), "x").is_err());
    }

    #[test]
    fn a_canvas_anywhere_in_the_tree_is_refused() {
        let mut p = payload();
        p["tree"] = json!({ "type": "box", "children": [{ "type": "box", "children": [{ "type": "canvas", "width": 5, "height": 5 }] }] });
        assert!(OpenSpec::from_json(&p, "x").unwrap_err().contains("canvas"));
    }
}
