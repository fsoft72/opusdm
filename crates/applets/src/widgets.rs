//! GTK widgets for a parsed [`Node`] tree, and the changes of
//! `opus.ui.update`. Signals call the applet's handlers through a
//! [`Dispatch`]: button click -> `null`, entry change -> the text, switch
//! toggle -> the state, list row -> `null` (each row has its own handler).
//! No window here: the host puts [`BuiltTree::root`] in one.
//!
//! A canvas is a `gtk::DrawingArea` whose draw function replays the last
//! command list given to [`set_canvas_commands`] (see `canvas`); it never
//! calls JavaScript.

use crate::canvas::{self, Command};
use crate::ui_tree::{self, Kind, ListItem, Node, Orientation};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, glib};
use opusdm_ipc::{debug_log, err_log};
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Calls a JavaScript handler of the applet. The runtime's dispatcher
/// (`Runtime::dispatcher`) holds only a weak reference to the runtime.
pub trait Dispatch {
    /// Call handler `handler` with `arg`.
    fn dispatch(&self, handler: u32, arg: Value);
}

/// A built tree: its root widget and the widgets that have an id.
///
/// Ownership: the widgets own their signal closures; a closure holds the
/// `Rc<dyn Dispatch>` given to [`build`] and the cell with its handler id,
/// never a widget of the tree, so there is no reference cycle. The caller
/// owns the `BuiltTree` with the window it shows.
pub struct BuiltTree {
    /// The widget to put in the applet window.
    pub root: gtk::Widget,
    ids: HashMap<String, Item>,
    /// The painter of every canvas of the tree, with or without an id.
    painters: Vec<Rc<Painter>>,
}

/// One widget with an id: what `apply_update` needs to change it.
struct Item {
    widget: gtk::Widget,
    view: View,
    /// The handler the widget's signal dispatches (`onClick`, `onChange`,
    /// `onToggle`, `draw`); `None` for a node type without one.
    handler: Rc<Cell<Option<u32>>>,
    /// The CSS classes the applet added (GTK's own classes are kept).
    classes: RefCell<Vec<String>>,
}

/// The typed widget of an [`Item`].
enum View {
    Box,
    Label(gtk::Label),
    Button(gtk::Button),
    /// The entry and its `changed` handler, blocked while an update sets the text.
    Entry(gtk::Entry, glib::SignalHandlerId),
    /// The switch and its `notify::active` handler, blocked while an update sets it.
    Switch(gtk::Switch, glib::SignalHandlerId),
    /// The list and the handler of each row, by row index.
    List(gtk::ListBox, Rc<RefCell<Vec<Option<u32>>>>),
    Image(gtk::Picture),
    /// A canvas: its drawing area and the painter its draw function uses.
    Canvas(gtk::DrawingArea, Rc<Painter>),
}

/// One validated prop of an update, ready to apply.
enum Change {
    Text(String),
    Active(bool),
    Handler(Option<u32>),
    Texture(gdk::Texture),
    Items(Vec<ListItem>),
    Visible(bool),
    Classes(Vec<String>),
}

impl View {
    /// The node type name, for errors.
    fn type_name(&self) -> &'static str {
        match self {
            View::Box => "box",
            View::Label(_) => "label",
            View::Button(_) => "button",
            View::Entry(..) => "entry",
            View::Switch(..) => "switch",
            View::List(..) => "list",
            View::Image(_) => "image",
            View::Canvas(..) => "canvas",
        }
    }

    /// The name of this type's handler prop, if it has one.
    fn handler_prop(&self) -> Option<&'static str> {
        match self {
            View::Button(_) => Some("onClick"),
            View::Entry(..) => Some("onChange"),
            View::Switch(..) => Some("onToggle"),
            View::Canvas(..) => Some("draw"),
            _ => None,
        }
    }
}

impl BuiltTree {
    /// The widget with id `id`.
    pub fn widget(&self, id: &str) -> Option<&gtk::Widget> {
        self.ids.get(id).map(|item| &item.widget)
    }

    /// The handler id now connected to the widget `id` (`onClick`,
    /// `onChange`, `onToggle` or `draw`).
    pub fn handler_of(&self, id: &str) -> Option<u32> {
        self.ids.get(id).and_then(|item| item.handler.get())
    }
}

/// Build the widgets of `node`; signals call `dispatch`. An image that
/// cannot be loaded or a duplicate id fails the whole tree.
pub fn build(node: &Node, dispatch: Rc<dyn Dispatch>) -> Result<BuiltTree, String> {
    let mut ids = HashMap::new();
    let mut painters = Vec::new();
    let root = build_node(node, &dispatch, &mut ids, &mut painters)?;
    Ok(BuiltTree { root, ids, painters })
}

/// Build `node` and its children; widgets with an id go into `ids`, the
/// painter of every canvas into `painters`.
fn build_node(node: &Node, dispatch: &Rc<dyn Dispatch>, ids: &mut HashMap<String, Item>, painters: &mut Vec<Rc<Painter>>) -> Result<gtk::Widget, String> {
    let what = ui_tree::describe(node.kind.type_name(), node.id.as_deref());
    if let Some(id) = &node.id {
        if ids.contains_key(id) { return Err(format!("{what}: duplicate id \"{id}\" in the tree")) }
    }
    let handler: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    let (widget, view): (gtk::Widget, View) = match &node.kind {
        Kind::Box { orientation, spacing, children } => {
            let orientation = match orientation {
                Orientation::Vertical => gtk::Orientation::Vertical,
                Orientation::Horizontal => gtk::Orientation::Horizontal,
            };
            let container = gtk::Box::new(orientation, *spacing);
            for child in children {
                container.append(&build_node(child, dispatch, ids, painters)?);
            }
            (container.upcast(), View::Box)
        }
        Kind::Label { text } => {
            let label = gtk::Label::new(Some(text));
            // Wrapping keeps the minimum width small: a long text must not
            // make the window wider than the applet asked (alignment kept).
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            (label.clone().upcast(), View::Label(label))
        }
        Kind::Button { label, on_click } => {
            let button = gtk::Button::with_label(label);
            handler.set(*on_click);
            let (cell, dispatch) = (handler.clone(), dispatch.clone());
            button.connect_clicked(move |_| fire(dispatch.as_ref(), &cell, "click", Value::Null));
            (button.clone().upcast(), View::Button(button))
        }
        Kind::Entry { text, on_change } => {
            let entry = gtk::Entry::new();
            entry.set_text(text);
            handler.set(*on_change);
            let (cell, dispatch) = (handler.clone(), dispatch.clone());
            let signal = entry.connect_changed(move |entry| fire(dispatch.as_ref(), &cell, "change", Value::String(entry.text().to_string())));
            (entry.clone().upcast(), View::Entry(entry, signal))
        }
        Kind::Switch { active, on_toggle } => {
            let switch = gtk::Switch::new();
            switch.set_active(*active);
            handler.set(*on_toggle);
            let (cell, dispatch) = (handler.clone(), dispatch.clone());
            let signal = switch.connect_active_notify(move |switch| fire(dispatch.as_ref(), &cell, "toggle", Value::Bool(switch.is_active())));
            (switch.clone().upcast(), View::Switch(switch, signal))
        }
        Kind::List { items } => {
            let list = gtk::ListBox::new();
            let handlers: Rc<RefCell<Vec<Option<u32>>>> = Rc::default();
            fill_list(&list, &handlers, items);
            let (rows, dispatch) = (handlers.clone(), dispatch.clone());
            list.connect_row_activated(move |_, row| activate_row(dispatch.as_ref(), &rows, row));
            (list.clone().upcast(), View::List(list, handlers))
        }
        Kind::Image { path, width, height } => {
            let picture = gtk::Picture::for_paintable(&load_texture(&what, path)?);
            picture.set_size_request(width.unwrap_or(-1), height.unwrap_or(-1));
            (picture.clone().upcast(), View::Image(picture))
        }
        Kind::Canvas { width, height, draw } => {
            // `draw` runs in JavaScript (opus.ui.redraw); the handler is kept
            // for `handler_of`, the draw function only replays commands.
            let area = gtk::DrawingArea::new();
            area.set_content_width(*width);
            area.set_content_height(*height);
            handler.set(*draw);
            let painter = Rc::new(Painter::new(node.id.as_deref().unwrap_or("(no id)"), Rc::default()));
            painters.push(painter.clone());
            let (clip_width, clip_height) = (f64::from(*width), f64::from(*height));
            let draw_painter = painter.clone();
            area.set_draw_func(move |_, cr, _, _| {
                // Clip to the canvas size, as a browser canvas does.
                cr.rectangle(0.0, 0.0, clip_width, clip_height);
                cr.clip();
                draw_painter.paint(cr);
            });
            (area.clone().upcast(), View::Canvas(area, painter))
        }
    };
    for class in &node.classes {
        widget.add_css_class(class);
    }
    widget.set_visible(node.visible);
    if let Some(id) = &node.id {
        ids.insert(id.clone(), Item { widget: widget.clone(), view, handler, classes: RefCell::new(node.classes.clone()) });
    }
    Ok(widget)
}

/// Dispatch the handler in `cell` with `arg`; a widget without a handler
/// (no `onClick` given) does nothing, by design.
fn fire(dispatch: &dyn Dispatch, cell: &Cell<Option<u32>>, what: &str, arg: Value) {
    let Some(handler) = cell.get() else {
        debug_log!(APP_TAG, "widget {what} with no handler, nothing to call");
        return;
    };
    dispatch.dispatch(handler, arg);
}

/// Dispatch the handler of the activated `row`. The handler list is
/// released before the call: the handler may replace the rows.
fn activate_row(dispatch: &dyn Dispatch, rows: &RefCell<Vec<Option<u32>>>, row: &gtk::ListBoxRow) {
    let index = row.index();
    let handler = usize::try_from(index).ok().and_then(|index| rows.borrow().get(index).copied());
    let Some(handler) = handler else {
        err_log!(APP_TAG, "list row {index} activated but it is not one of the list's items");
        return;
    };
    let Some(handler) = handler else {
        debug_log!(APP_TAG, "list row {index} has no onClick, nothing to call");
        return;
    };
    dispatch.dispatch(handler, Value::Null);
}

/// Replace the rows of `list` by one activatable label per item, and the
/// handlers in `handlers` by theirs. Each label is left-aligned, expands,
/// ellipsizes at the end (so a long title does not make the window wider
/// than the applet asked) and has the full text as its tooltip.
fn fill_list(list: &gtk::ListBox, handlers: &RefCell<Vec<Option<u32>>>, items: &[ListItem]) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for item in items {
        let label = gtk::Label::new(Some(&item.text));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_tooltip_text(Some(&item.text));
        list.append(&label);
    }
    *handlers.borrow_mut() = items.iter().map(|item| item.on_click).collect();
}

/// Load the image file `path` (an error names `what` and the file).
fn load_texture(what: &str, path: &str) -> Result<gdk::Texture, String> {
    gdk::Texture::from_filename(path).map_err(|e| format!("{what}: cannot load the image {path}: {e}"))
}

/// Apply the `opus.ui.update` props `props` to the widget `id`. An unknown
/// id, an unknown prop for the widget's type or a wrong prop type is an
/// error, and then nothing is changed. Setting an entry's text or a
/// switch's state does not call its `onChange` / `onToggle`.
pub fn apply_update(tree: &BuiltTree, id: &str, props: &Value) -> Result<(), String> {
    let Some(item) = tree.ids.get(id) else { return Err(format!("update: no widget with id \"{id}\"")) };
    let what = format!("update of {}", ui_tree::describe(item.view.type_name(), Some(id)));
    let Some(props) = props.as_object() else { return Err(format!("{what}: the props must be an object, not {props}")) };
    let changes = parse_changes(&item.view, &what, props)?;
    for change in changes {
        apply_change(item, change);
    }
    Ok(())
}

/// Validate every prop of an update before anything is applied.
fn parse_changes(view: &View, what: &str, props: &Map<String, Value>) -> Result<Vec<Change>, String> {
    let mut changes = Vec::with_capacity(props.len());
    for (key, value) in props {
        let change = match (view, key.as_str()) {
            (_, "visible") => Change::Visible(ui_tree::bool_value(what, key, value)?),
            (_, "class") => Change::Classes(if value.is_null() { Vec::new() } else { ui_tree::classes_value(what, value)? }),
            (View::Label(_), "text") | (View::Button(_), "label") | (View::Entry(..), "text") => Change::Text(ui_tree::text_value(what, key, value)?),
            (View::Switch(..), "active") => Change::Active(ui_tree::bool_value(what, key, value)?),
            (View::Image(_), "path") => Change::Texture(load_texture(what, &ui_tree::path_value(what, value)?)?),
            (View::List(..), "items") => Change::Items(ui_tree::items_value(what, value)?),
            (view, key) if view.handler_prop() == Some(key) => Change::Handler(ui_tree::handler_value(what, key, value)?),
            (_, key) => return Err(format!("{what}: unknown prop \"{key}\"")),
        };
        changes.push(change);
    }
    Ok(changes)
}

/// Apply one validated change to `item`.
fn apply_change(item: &Item, change: Change) {
    match (&item.view, change) {
        (_, Change::Visible(visible)) => item.widget.set_visible(visible),
        (_, Change::Classes(classes)) => {
            for old in item.classes.borrow().iter() {
                item.widget.remove_css_class(old);
            }
            for class in &classes {
                item.widget.add_css_class(class);
            }
            *item.classes.borrow_mut() = classes;
        }
        (_, Change::Handler(handler)) => item.handler.set(handler),
        (View::Label(label), Change::Text(text)) => label.set_text(&text),
        (View::Button(button), Change::Text(text)) => button.set_label(&text),
        (View::Entry(entry, signal), Change::Text(text)) => {
            entry.block_signal(signal);
            entry.set_text(&text);
            entry.unblock_signal(signal);
        }
        (View::Switch(switch, signal), Change::Active(active)) => {
            switch.block_signal(signal);
            switch.set_active(active);
            switch.unblock_signal(signal);
        }
        (View::Image(picture), Change::Texture(texture)) => picture.set_paintable(Some(&texture)),
        (View::List(list, handlers), Change::Items(items)) => fill_list(list, handlers, &items),
        (view, _) => err_log!(APP_TAG, "internal: an update change does not fit a {}, ignored", view.type_name()),
    }
}

/// What the draw function of one canvas keeps between frames: the commands
/// to replay, the images they load, and the last replay error it logged.
/// It holds no widget, so the drawing area that owns it has no cycle.
struct Painter {
    /// The canvas id, for log lines.
    id: String,
    /// The last command list given to [`set_canvas_commands`].
    commands: Rc<RefCell<Vec<Command>>>,
    images: RefCell<canvas::Images>,
    /// The last replay error logged, so a redraw every second with the same
    /// fault logs it once; a good frame clears it.
    last_error: RefCell<Option<String>>,
    /// Told every new replay error (see [`set_error_listener`]).
    listener: RefCell<Option<ErrorListener>>,
}

/// Receives the new replay errors of a canvas. It must hold only weak
/// references: the drawing area owns it through its painter.
pub type ErrorListener = Rc<dyn Fn(String)>;

impl Painter {
    /// A painter of canvas `id` replaying `commands`.
    fn new(id: &str, commands: Rc<RefCell<Vec<Command>>>) -> Painter {
        Painter { id: id.to_string(), commands, images: RefCell::default(), last_error: RefCell::new(None), listener: RefCell::new(None) }
    }

    /// Replay the commands on `cr`. A failure is logged with `err_log!`
    /// unless it is the same as the last one logged; returns what was logged.
    fn paint(&self, cr: &gtk::cairo::Context) -> Option<String> {
        let result = canvas::replay_with(cr, &self.commands.borrow(), &mut self.images.borrow_mut());
        let Err(error) = result else {
            *self.last_error.borrow_mut() = None;
            return None;
        };
        if self.last_error.borrow().as_deref() == Some(error.as_str()) { return None; }
        err_log!(APP_TAG, "canvas \"{}\": {error}", self.id);
        *self.last_error.borrow_mut() = Some(error.clone());
        // Cloned out first: the listener may set another listener.
        let listener = self.listener.borrow().clone();
        if let Some(listener) = listener {
            listener(format!("canvas \"{}\": {error}", self.id));
        }
        Some(error)
    }

    /// Tell `listener` every new replay error from now on.
    fn set_listener(&self, listener: ErrorListener) {
        *self.listener.borrow_mut() = Some(listener);
    }

    /// The last replay error logged, if the last frame failed.
    #[cfg(test)]
    fn last_error(&self) -> Option<String> {
        self.last_error.borrow().clone()
    }
}

/// Tell `listener` every new replay error of every canvas of `tree` (an
/// error is new when it differs from the last one of that canvas, as for
/// the log). The listener runs inside the draw function: it should defer
/// any widget change to the main loop.
pub fn set_error_listener(tree: &BuiltTree, listener: ErrorListener) {
    for painter in &tree.painters {
        painter.set_listener(listener.clone());
    }
}

/// Give canvas `id` the commands of its latest `draw` (from
/// `canvas::parse` of an `Event::Redraw`) and queue a repaint. An unknown
/// id, a widget that is not a canvas, or more than [`canvas::MAX_COMMANDS`]
/// commands is an error, and then the previous picture stays.
pub fn set_canvas_commands(tree: &BuiltTree, id: &str, commands: Vec<Command>) -> Result<(), String> {
    let Some(item) = tree.ids.get(id) else { return Err(format!("redraw: no widget with id \"{id}\"")) };
    let View::Canvas(area, painter) = &item.view else { return Err(format!("redraw: {} is not a canvas", ui_tree::describe(item.view.type_name(), Some(id)))) };
    if commands.len() > canvas::MAX_COMMANDS {
        return Err(format!("redraw of canvas \"{id}\": {} commands, at most {} are allowed", commands.len(), canvas::MAX_COMMANDS));
    }
    *painter.commands.borrow_mut() = commands;
    area.queue_draw();
    Ok(())
}

/// The commands canvas `id` replays now (`None` when it is not a canvas).
#[cfg(test)]
fn canvas_commands(tree: &BuiltTree, id: &str) -> Option<Vec<Command>> {
    match &tree.ids.get(id)?.view {
        View::Canvas(_, painter) => Some(painter.commands.borrow().clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
