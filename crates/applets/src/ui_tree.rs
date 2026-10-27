//! The declarative widget tree of `opus.ui.show`: the JSON the prelude
//! sends (function props already replaced by `{ "handler": <id> }`) parsed
//! into a [`Node`]. Pure: no GTK here, see `widgets.rs` for the widgets.
//!
//! Every error names the node type, its id when it has one, and the prop.

use serde_json::{Map, Value};
use std::collections::HashSet;

/// Deepest nesting of nodes a tree may have (the root is level 1).
pub const MAX_DEPTH: usize = 32;

/// Largest width, height or spacing a node may ask for, in pixels.
pub const MAX_SIZE: i64 = 16384;

/// Props every node type accepts.
const COMMON_PROPS: [&str; 4] = ["type", "id", "class", "visible"];

/// One node of the tree: the common props and the type-specific ones.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The id `opus.ui.update` uses to find the node.
    pub id: Option<String>,
    /// CSS class names added to the widget.
    pub classes: Vec<String>,
    /// Whether the widget is shown (default `true`).
    pub visible: bool,
    /// The node type and its props.
    pub kind: Kind,
}

/// How a `box` lays out its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

/// A node type with its props. Handler props hold the id the prelude
/// registered (`None` when the applet gave no function).
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Box { orientation: Orientation, spacing: i32, children: Vec<Node> },
    Label { text: String },
    Button { label: String, on_click: Option<u32> },
    Entry { text: String, on_change: Option<u32> },
    Switch { active: bool, on_toggle: Option<u32> },
    List { items: Vec<ListItem> },
    /// A drop-down of strings; `selected` is an index into `items` (`None`: nothing selected).
    Dropdown { items: Vec<String>, selected: Option<usize>, on_change: Option<u32> },
    Image { path: String, width: Option<i32>, height: Option<i32> },
    Canvas { width: i32, height: i32, draw: Option<u32> },
}

/// One row of a `list`.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub text: String,
    pub on_click: Option<u32>,
}

impl Kind {
    /// The `type` name of the node, as the applet writes it.
    pub fn type_name(&self) -> &'static str {
        match self {
            Kind::Box { .. } => "box",
            Kind::Label { .. } => "label",
            Kind::Button { .. } => "button",
            Kind::Entry { .. } => "entry",
            Kind::Switch { .. } => "switch",
            Kind::List { .. } => "list",
            Kind::Dropdown { .. } => "dropdown",
            Kind::Image { .. } => "image",
            Kind::Canvas { .. } => "canvas",
        }
    }
}

/// Parse the tree `value`. Unknown types or props, wrong prop types,
/// duplicate ids and nesting deeper than [`MAX_DEPTH`] are errors.
pub fn parse(value: &Value) -> Result<Node, String> {
    let mut ids = HashSet::new();
    parse_node(value, 1, &mut ids)
}

/// Parse the node `value` at nesting level `depth`; `ids` collects the ids
/// seen so far in the tree.
fn parse_node(value: &Value, depth: usize, ids: &mut HashSet<String>) -> Result<Node, String> {
    let Some(object) = value.as_object() else { return Err(format!("a node must be an object, not {}", json_kind(value))) };
    let ty = match object.get("type") {
        Some(Value::String(ty)) => ty.as_str(),
        Some(other) => return Err(format!("a node \"type\" must be a string, not {}", json_kind(other))),
        None => return Err("a node has no \"type\"".to_string()),
    };
    let id = match object.get("id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if !id.is_empty() => Some(id.clone()),
        Some(other) => return Err(format!("{ty}: \"id\" must be a non-empty string, not {}", json_kind(other))),
    };
    let what = describe(ty, id.as_deref());
    if depth > MAX_DEPTH { return Err(format!("{what}: the tree is nested deeper than {MAX_DEPTH} levels")) }
    if let Some(id) = &id {
        if !ids.insert(id.clone()) { return Err(format!("{what}: duplicate id \"{id}\" in the tree")) }
    }
    let props = Props { object, what: &what };
    let kind = match ty {
        "box" => {
            props.allow(&["orientation", "spacing", "children"])?;
            let orientation = match props.get("orientation") {
                None => Orientation::Vertical,
                Some(value) => orientation_value(&what, value)?,
            };
            let spacing = props.get("spacing").map(|v| size_value(&what, "spacing", v, 0)).transpose()?.unwrap_or(0);
            let children = match props.get("children") {
                None => Vec::new(),
                Some(Value::Array(children)) => children.iter().map(|child| parse_node(child, depth + 1, ids)).collect::<Result<Vec<_>, _>>()?,
                Some(other) => return Err(format!("{what}: \"children\" must be an array, not {}", json_kind(other))),
            };
            Kind::Box { orientation, spacing, children }
        }
        "label" => {
            props.allow(&["text"])?;
            Kind::Label { text: text_value(&what, "text", props.required("text")?)? }
        }
        "button" => {
            props.allow(&["label", "onClick"])?;
            Kind::Button { label: text_value(&what, "label", props.required("label")?)?, on_click: props.handler("onClick")? }
        }
        "entry" => {
            props.allow(&["text", "onChange"])?;
            let text = props.get("text").map(|v| text_value(&what, "text", v)).transpose()?.unwrap_or_default();
            Kind::Entry { text, on_change: props.handler("onChange")? }
        }
        "switch" => {
            props.allow(&["active", "onToggle"])?;
            let active = props.get("active").map(|v| bool_value(&what, "active", v)).transpose()?.unwrap_or(false);
            Kind::Switch { active, on_toggle: props.handler("onToggle")? }
        }
        "list" => {
            props.allow(&["items"])?;
            let items = props.get("items").map(|v| items_value(&what, v)).transpose()?.unwrap_or_default();
            Kind::List { items }
        }
        "dropdown" => {
            props.allow(&["items", "selected", "onChange"])?;
            let items = strings_value(&what, props.required("items")?)?;
            let selected = props.get("selected").map(|v| selected_value(&what, v, items.len())).transpose()?;
            Kind::Dropdown { items, selected, on_change: props.handler("onChange")? }
        }
        "image" => {
            props.allow(&["path", "width", "height"])?;
            let width = props.get("width").map(|v| size_value(&what, "width", v, 1)).transpose()?;
            let height = props.get("height").map(|v| size_value(&what, "height", v, 1)).transpose()?;
            Kind::Image { path: path_value(&what, props.required("path")?)?, width, height }
        }
        "canvas" => {
            props.allow(&["width", "height", "draw"])?;
            let width = size_value(&what, "width", props.required("width")?, 1)?;
            let height = size_value(&what, "height", props.required("height")?, 1)?;
            Kind::Canvas { width, height, draw: props.handler("draw")? }
        }
        other => return Err(format!("unknown node type \"{other}\"{}", id.as_deref().map(|id| format!(" (id \"{id}\")")).unwrap_or_default())),
    };
    let classes = props.get("class").map(|v| classes_value(&what, v)).transpose()?.unwrap_or_default();
    let visible = props.get("visible").map(|v| bool_value(&what, "visible", v)).transpose()?.unwrap_or(true);
    Ok(Node { id, classes, visible, kind })
}

/// The props of one node, with the node's description for errors.
struct Props<'a> {
    object: &'a Map<String, Value>,
    what: &'a str,
}

impl Props<'_> {
    /// Refuse any prop that is not common nor in `known`.
    fn allow(&self, known: &[&str]) -> Result<(), String> {
        let unknown = self.object.keys().find(|key| !COMMON_PROPS.contains(&key.as_str()) && !known.contains(&key.as_str()));
        match unknown {
            Some(key) => Err(format!("{}: unknown prop \"{key}\"", self.what)),
            None => Ok(()),
        }
    }

    /// The prop `key`; `null` counts as missing.
    fn get(&self, key: &str) -> Option<&Value> {
        self.object.get(key).filter(|v| !v.is_null())
    }

    /// The prop `key`, an error when it is missing.
    fn required(&self, key: &str) -> Result<&Value, String> {
        self.get(key).ok_or_else(|| format!("{}: missing prop \"{key}\"", self.what))
    }

    /// The handler prop `key` (`None` when missing or `null`).
    fn handler(&self, key: &str) -> Result<Option<u32>, String> {
        handler_value(self.what, key, self.object.get(key).unwrap_or(&Value::Null))
    }
}

/// How a node is named in errors: `button "ok"`, or `button` with no id.
pub(crate) fn describe(ty: &str, id: Option<&str>) -> String {
    match id {
        Some(id) => format!("{ty} \"{id}\""),
        None => ty.to_string(),
    }
}

/// The JSON type of `value`, for errors.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// A string prop.
pub(crate) fn text_value(what: &str, key: &str, value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        other => Err(format!("{what}: \"{key}\" must be a string, not {}", json_kind(other))),
    }
}

/// A boolean prop.
pub(crate) fn bool_value(what: &str, key: &str, value: &Value) -> Result<bool, String> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        other => Err(format!("{what}: \"{key}\" must be true or false, not {}", json_kind(other))),
    }
}

/// A whole number of pixels from `min` to [`MAX_SIZE`] (a JS number may
/// arrive as an integral float).
pub(crate) fn size_value(what: &str, key: &str, value: &Value, min: i64) -> Result<i32, String> {
    let number = value.as_i64().or_else(|| value.as_f64().filter(|f| f.fract() == 0.0 && f.abs() <= MAX_SIZE as f64).map(|f| f as i64));
    match number {
        Some(n) if (min..=MAX_SIZE).contains(&n) => Ok(n as i32),
        _ => Err(format!("{what}: \"{key}\" must be a whole number from {min} to {MAX_SIZE}, not {value}")),
    }
}

/// `"vertical"` or `"horizontal"`.
fn orientation_value(what: &str, value: &Value) -> Result<Orientation, String> {
    match value.as_str() {
        Some("vertical") => Ok(Orientation::Vertical),
        Some("horizontal") => Ok(Orientation::Horizontal),
        _ => Err(format!("{what}: \"orientation\" must be \"vertical\" or \"horizontal\", not {value}")),
    }
}

/// A handler prop: `null` (no handler) or `{ "handler": <id> }`, which
/// the prelude puts in place of a function.
pub(crate) fn handler_value(what: &str, key: &str, value: &Value) -> Result<Option<u32>, String> {
    if value.is_null() { return Ok(None) }
    let bad = || format!("{what}: \"{key}\" must be a function, not {value}");
    let Some(object) = value.as_object() else { return Err(bad()) };
    if object.len() != 1 { return Err(bad()) }
    let id = object.get("handler").and_then(|v| v.as_u64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64)));
    id.and_then(|id| u32::try_from(id).ok()).map(Some).ok_or_else(bad)
}

/// The `items` of a list: `[{ text, onClick? }]`.
pub(crate) fn items_value(what: &str, value: &Value) -> Result<Vec<ListItem>, String> {
    let Some(items) = value.as_array() else { return Err(format!("{what}: \"items\" must be an array, not {}", json_kind(value))) };
    let mut list = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_what = format!("{what} items[{index}]");
        let Some(object) = item.as_object() else { return Err(format!("{item_what}: an item must be an object, not {}", json_kind(item))) };
        if let Some(key) = object.keys().find(|key| *key != "text" && *key != "onClick") {
            return Err(format!("{item_what}: unknown prop \"{key}\""));
        }
        let text = match object.get("text") {
            Some(text) => text_value(&item_what, "text", text)?,
            None => return Err(format!("{item_what}: missing prop \"text\"")),
        };
        let on_click = handler_value(&item_what, "onClick", object.get("onClick").unwrap_or(&Value::Null))?;
        list.push(ListItem { text, on_click });
    }
    Ok(list)
}

/// The `items` of a dropdown: an array of strings.
pub(crate) fn strings_value(what: &str, value: &Value) -> Result<Vec<String>, String> {
    let Some(items) = value.as_array() else { return Err(format!("{what}: \"items\" must be an array, not {}", json_kind(value))) };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::String(text) => Ok(text.clone()),
            other => Err(format!("{what} items[{index}]: an item must be a string, not {}", json_kind(other))),
        })
        .collect()
}

/// The `selected` index of a dropdown with `len` items: a whole number from
/// 0 to `len - 1` (a JS number may arrive as an integral float).
pub(crate) fn selected_value(what: &str, value: &Value, len: usize) -> Result<usize, String> {
    let number = value.as_u64().or_else(|| value.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0 && *f <= MAX_SIZE as f64).map(|f| f as u64));
    match number {
        Some(n) if (n as usize) < len => Ok(n as usize),
        _ => Err(format!("{what}: \"selected\" must be a whole number from 0 to {} (the last item), not {value}", len.saturating_sub(1))),
    }
}

/// An image path: absolute (applets build it with `opus.applet.assetPath`).
pub(crate) fn path_value(what: &str, value: &Value) -> Result<String, String> {
    let path = text_value(what, "path", value)?;
    if !std::path::Path::new(&path).is_absolute() {
        return Err(format!("{what}: \"path\" must be absolute (use opus.applet.assetPath), not \"{path}\""));
    }
    Ok(path)
}

/// CSS classes: one string of names separated by spaces, or an array of names.
pub(crate) fn classes_value(what: &str, value: &Value) -> Result<Vec<String>, String> {
    let bad = || format!("{what}: \"class\" must be a string or an array of class names, not {value}");
    match value {
        Value::String(names) => Ok(names.split_whitespace().map(str::to_string).collect()),
        Value::Array(names) => names
            .iter()
            .map(|name| match name.as_str() {
                Some(name) if !name.is_empty() && !name.contains(char::is_whitespace) => Ok(name.to_string()),
                _ => Err(bad()),
            })
            .collect(),
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests;
