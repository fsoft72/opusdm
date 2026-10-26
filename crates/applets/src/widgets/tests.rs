use super::*;
use crate::manifest;
use crate::runtime::{Event, Notifier, Runtime, Sink};
use crate::canvas::{Command, Style};
use crate::ui_tree::{self, Kind, Orientation};
use gtk::{gdk, glib};
use serde_json::{json, Map};
use std::cell::RefCell;
use std::sync::Arc;

/// A [`Dispatch`] that records every call.
#[derive(Default)]
struct Recorder(RefCell<Vec<(u32, Value)>>);

impl Dispatch for Recorder {
    /// Remember the call.
    fn dispatch(&self, handler: u32, arg: Value) {
        self.0.borrow_mut().push((handler, arg));
    }
}

impl Recorder {
    /// The calls so far.
    fn calls(&self) -> Vec<(u32, Value)> {
        self.0.borrow().clone()
    }
}

/// Parse and build `tree` with `recorder` as the dispatcher.
fn build_json(tree: Value, recorder: &Rc<Recorder>) -> Result<BuiltTree, String> {
    let node = ui_tree::parse(&tree)?;
    build(&node, recorder.clone())
}

/// The widget `id` of `tree` as a `T`.
fn get<T: IsA<gtk::Widget>>(tree: &BuiltTree, id: &str) -> T {
    tree.widget(id).unwrap_or_else(|| panic!("no widget {id}")).clone().downcast::<T>().unwrap_or_else(|_| panic!("{id} has another type"))
}

/// The texts of the rows of `list`.
fn row_texts(list: &gtk::ListBox) -> Vec<String> {
    let mut texts = Vec::new();
    let mut index = 0;
    while let Some(row) = list.row_at_index(index) {
        let label = row.child().and_downcast::<gtk::Label>().expect("a row holds a label");
        texts.push(label.text().to_string());
        index += 1;
    }
    texts
}

/// Emit `row-activated` for row `index` of `list`, as GTK does on a click
/// (`ListBoxRow::emit_activate` needs a window root to read the focus).
fn activate_row_at(list: &gtk::ListBox, index: i32) {
    let row = list.row_at_index(index).unwrap_or_else(|| panic!("no row {index}"));
    list.emit_by_name::<()>("row-activated", &[&row]);
}

/// GTK may be used from one test thread only, so every widget check is in
/// this one test, one helper per check.
#[test]
fn gtk_widget_checks() {
    if gtk::init().is_err() { return; }
    a_label_shows_its_text();
    update_changes_a_label_text();
    a_button_click_dispatches_its_handler_id();
    update_of_an_unknown_id_is_an_error();
    entry_switch_and_list_dispatch_their_args();
    programmatic_updates_do_not_echo();
    bad_updates_are_errors_naming_the_id_and_the_prop();
    common_props_and_handler_updates();
    list_items_update_rebuilds_the_rows();
    image_and_canvas();
    duplicate_ids_are_refused_at_build();
    a_click_reaches_the_applet_script_and_is_logged_after_the_runtime_is_gone();
    set_canvas_commands_stores_them_and_refuses_unknown_ids();
    a_script_canvas_reaches_its_drawing_area();
    the_error_listener_reaches_every_canvas_of_the_tree();
    long_text_does_not_widen_the_window();
}

/// Long text never forces the applet window wider than the applet asked: a
/// list row label ellipsizes at the end (left-aligned, hexpand, the full
/// text as its tooltip, also after an update) and a label node wraps on
/// word / char boundaries, so their minimum width stays small.
fn long_text_does_not_widen_the_window() {
    let recorder = Rc::new(Recorder::default());
    let long = "x".repeat(500);
    let tree = build_json(json!({ "type": "box", "children": [
        { "type": "list", "id": "l", "items": [{ "text": long }] },
        { "type": "label", "id": "t", "text": long }
    ] }), &recorder).unwrap();
    let list = get::<gtk::ListBox>(&tree, "l");
    let row_label = |list: &gtk::ListBox| list.row_at_index(0).and_then(|r| r.child()).and_downcast::<gtk::Label>().expect("a row holds a label");
    let label = row_label(&list);
    assert_eq!(label.ellipsize(), gtk::pango::EllipsizeMode::End, "a list row label ellipsizes at the end");
    assert_eq!(label.xalign(), 0.0, "a list row label is left-aligned");
    assert!(label.hexpands(), "a list row label expands horizontally");
    assert_eq!(label.tooltip_text().as_deref(), Some(long.as_str()), "the full text is the tooltip");
    let (min, _, _, _) = list.measure(gtk::Orientation::Horizontal, -1);
    assert!(min < 200, "a list with a 500 character item must have a small minimum width, not {min}");
    apply_update(&tree, "l", &json!({ "items": [{ "text": "short" }] })).unwrap();
    let label = row_label(&list);
    assert_eq!(label.ellipsize(), gtk::pango::EllipsizeMode::End, "an updated row ellipsizes too");
    assert_eq!(label.tooltip_text().as_deref(), Some("short"));
    let text = get::<gtk::Label>(&tree, "t");
    assert!(text.wraps(), "a label node wraps");
    assert_eq!(text.wrap_mode(), gtk::pango::WrapMode::WordChar, "on word or char boundaries");
    assert_eq!(text.xalign(), 0.5, "the label alignment is unchanged (centered)");
    let (min, _, _, _) = text.measure(gtk::Orientation::Horizontal, -1);
    assert!(min < 200, "a label with 500 characters must have a small minimum width, not {min}");
}

/// a_label_shows_its_text
fn a_label_shows_its_text() {
    let recorder = Rc::new(Recorder::default());
    let tree = build_json(json!({ "type": "label", "id": "t", "text": "Hello" }), &recorder).unwrap();
    let label = tree.root.clone().downcast::<gtk::Label>().unwrap();
    assert_eq!(label.text(), "Hello");
    assert_eq!(get::<gtk::Label>(&tree, "t"), label);
}

/// update_changes_a_label_text
fn update_changes_a_label_text() {
    let recorder = Rc::new(Recorder::default());
    let tree = build_json(json!({ "type": "box", "children": [{ "type": "label", "id": "t", "text": "old" }] }), &recorder).unwrap();
    apply_update(&tree, "t", &json!({ "text": "new" })).unwrap();
    assert_eq!(get::<gtk::Label>(&tree, "t").text(), "new");
}

/// a_button_click_dispatches_its_handler_id
fn a_button_click_dispatches_its_handler_id() {
    let recorder = Rc::new(Recorder::default());
    let tree = build_json(json!({ "type": "button", "id": "b", "label": "Go", "onClick": { "handler": 9 } }), &recorder).unwrap();
    let button = get::<gtk::Button>(&tree, "b");
    assert_eq!(button.label().as_deref(), Some("Go"));
    button.emit_clicked();
    button.emit_clicked();
    assert_eq!(recorder.calls(), [(9, Value::Null), (9, Value::Null)]);
    let plain = build_json(json!({ "type": "button", "id": "p", "label": "Nothing" }), &recorder).unwrap();
    get::<gtk::Button>(&plain, "p").emit_clicked();
    assert_eq!(recorder.calls().len(), 2, "a button without onClick dispatches nothing");
}

/// update_of_an_unknown_id_is_an_error
fn update_of_an_unknown_id_is_an_error() {
    let recorder = Rc::new(Recorder::default());
    let tree = build_json(json!({ "type": "label", "id": "t", "text": "x" }), &recorder).unwrap();
    let e = apply_update(&tree, "nope", &json!({ "text": "y" })).unwrap_err();
    assert!(e.contains("\"nope\""), "{e}");
}

/// Entry change -> the text, switch toggle -> the state, list item ->
/// null with the item's own handler.
fn entry_switch_and_list_dispatch_their_args() {
    let recorder = Rc::new(Recorder::default());
    let tree = json!({ "type": "box", "children": [
        { "type": "entry", "id": "e", "text": "start", "onChange": { "handler": 1 } },
        { "type": "switch", "id": "s", "active": false, "onToggle": { "handler": 2 } },
        { "type": "list", "id": "l", "items": [{ "text": "one", "onClick": { "handler": 3 } }, { "text": "two", "onClick": { "handler": 4 } }, { "text": "three" }] }
    ] });
    let tree = build_json(tree, &recorder).unwrap();
    assert!(recorder.calls().is_empty(), "building dispatches nothing: {:?}", recorder.calls());
    let entry = get::<gtk::Entry>(&tree, "e");
    assert_eq!(entry.text(), "start");
    entry.set_text("typed");
    assert_eq!(recorder.calls().last(), Some(&(1, json!("typed"))));
    let switch = get::<gtk::Switch>(&tree, "s");
    switch.set_active(true);
    assert_eq!(recorder.calls().last(), Some(&(2, json!(true))));
    switch.set_active(false);
    assert_eq!(recorder.calls().last(), Some(&(2, json!(false))));
    let list = get::<gtk::ListBox>(&tree, "l");
    assert_eq!(row_texts(&list), ["one", "two", "three"]);
    let before = recorder.calls().len();
    activate_row_at(&list, 1);
    activate_row_at(&list, 0);
    activate_row_at(&list, 2);
    assert_eq!(recorder.calls()[before..], [(4, Value::Null), (3, Value::Null)]);
}

/// Decision 6: `apply_update` of an entry's text or a switch's state does
/// not call `onChange` / `onToggle`, and the user's changes still do.
fn programmatic_updates_do_not_echo() {
    let recorder = Rc::new(Recorder::default());
    let tree = json!({ "type": "box", "children": [
        { "type": "entry", "id": "e", "onChange": { "handler": 1 } },
        { "type": "switch", "id": "s", "onToggle": { "handler": 2 } }
    ] });
    let tree = build_json(tree, &recorder).unwrap();
    apply_update(&tree, "e", &json!({ "text": "set by the applet" })).unwrap();
    apply_update(&tree, "s", &json!({ "active": true })).unwrap();
    assert_eq!(get::<gtk::Entry>(&tree, "e").text(), "set by the applet");
    assert!(get::<gtk::Switch>(&tree, "s").is_active());
    assert!(recorder.calls().is_empty(), "an update echoed: {:?}", recorder.calls());
    get::<gtk::Entry>(&tree, "e").set_text("by the user");
    get::<gtk::Switch>(&tree, "s").set_active(false);
    // GTK replaces a non-empty text with a delete then an insert, so the
    // entry may report "" before the new text: check the final values.
    let calls = recorder.calls();
    assert_eq!(calls.iter().rfind(|(h, _)| *h == 1), Some(&(1, json!("by the user"))), "the entry handler was left blocked: {calls:?}");
    assert_eq!(calls.last(), Some(&(2, json!(false))), "the switch handler was left blocked: {calls:?}");
}

/// Unknown props, wrong prop types and a non-object are errors naming the
/// id and the prop; nothing of a refused update is applied.
fn bad_updates_are_errors_naming_the_id_and_the_prop() {
    let recorder = Rc::new(Recorder::default());
    let tree = json!({ "type": "box", "children": [
        { "type": "label", "id": "t", "text": "x" },
        { "type": "button", "id": "b", "label": "B" }
    ] });
    let tree = build_json(tree, &recorder).unwrap();
    let e = apply_update(&tree, "t", &json!({ "label": "y" })).unwrap_err();
    assert!(e.contains("\"t\"") && e.contains("label"), "{e}");
    let e = apply_update(&tree, "b", &json!({ "label": 5 })).unwrap_err();
    assert!(e.contains("\"b\"") && e.contains("label"), "{e}");
    let e = apply_update(&tree, "t", &json!({ "text": "applied?", "visible": "no" })).unwrap_err();
    assert!(e.contains("visible"), "{e}");
    assert_eq!(get::<gtk::Label>(&tree, "t").text(), "x", "a refused update was partly applied");
    let e = apply_update(&tree, "t", &json!("text")).unwrap_err();
    assert!(e.contains("\"t\""), "{e}");
}

/// `visible` and `class` on any type; handler props replaced by an update.
fn common_props_and_handler_updates() {
    let recorder = Rc::new(Recorder::default());
    let tree = json!({ "type": "box", "id": "root", "class": "card", "children": [
        { "type": "button", "id": "b", "label": "B", "class": ["one", "two"], "visible": false, "onClick": { "handler": 1 } }
    ] });
    let tree = build_json(tree, &recorder).unwrap();
    assert!(tree.root.has_css_class("card"));
    let button = get::<gtk::Button>(&tree, "b");
    assert!(button.has_css_class("one") && button.has_css_class("two"));
    assert!(!button.is_visible());
    apply_update(&tree, "b", &json!({ "visible": true, "class": "three", "label": "New" })).unwrap();
    assert!(button.is_visible());
    assert!(!button.has_css_class("one") && !button.has_css_class("two") && button.has_css_class("three"));
    assert_eq!(button.label().as_deref(), Some("New"));
    assert_eq!(tree.handler_of("b"), Some(1));
    apply_update(&tree, "b", &json!({ "onClick": { "handler": 7 } })).unwrap();
    assert_eq!(tree.handler_of("b"), Some(7));
    button.emit_clicked();
    apply_update(&tree, "b", &json!({ "onClick": null })).unwrap();
    button.emit_clicked();
    assert_eq!(recorder.calls(), [(7, Value::Null)]);
}

/// An `items` update replaces the rows and their handlers.
fn list_items_update_rebuilds_the_rows() {
    let recorder = Rc::new(Recorder::default());
    let tree = build_json(json!({ "type": "list", "id": "l", "items": [{ "text": "a", "onClick": { "handler": 1 } }] }), &recorder).unwrap();
    apply_update(&tree, "l", &json!({ "items": [{ "text": "x", "onClick": { "handler": 5 } }, { "text": "y", "onClick": { "handler": 6 } }] })).unwrap();
    let list = get::<gtk::ListBox>(&tree, "l");
    assert_eq!(row_texts(&list), ["x", "y"]);
    activate_row_at(&list, 1);
    assert_eq!(recorder.calls(), [(6, Value::Null)]);
    let e = apply_update(&tree, "l", &json!({ "items": [{ "label": "x" }] })).unwrap_err();
    assert!(e.contains("\"l\"") && e.contains("items"), "{e}");
}

/// An image is loaded at build time (a missing file is an error); a canvas
/// gets its size and keeps its `draw` handler.
fn image_and_canvas() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("dot.png");
    let pixels = glib::Bytes::from(&[255u8; 16]);
    gdk::MemoryTexture::new(2, 2, gdk::MemoryFormat::R8g8b8a8, &pixels, 8).save_to_png(&png).unwrap();
    let recorder = Rc::new(Recorder::default());
    let path = png.display().to_string();
    let tree = json!({ "type": "box", "children": [
        { "type": "image", "id": "i", "path": path, "width": 32, "height": 24 },
        { "type": "canvas", "id": "c", "width": 120, "height": 80, "draw": { "handler": 4 } }
    ] });
    let tree = build_json(tree, &recorder).unwrap();
    let picture = get::<gtk::Picture>(&tree, "i");
    assert!(picture.paintable().is_some());
    assert_eq!(picture.size_request(), (32, 24));
    let canvas = get::<gtk::DrawingArea>(&tree, "c");
    assert_eq!((canvas.content_width(), canvas.content_height()), (120, 80));
    assert_eq!(tree.handler_of("c"), Some(4));
    apply_update(&tree, "i", &json!({ "path": path })).unwrap();
    let missing = dir.path().join("missing.png").display().to_string();
    let e = apply_update(&tree, "i", &json!({ "path": missing })).unwrap_err();
    assert!(e.contains("\"i\"") && e.contains("missing.png"), "{e}");
    let e = build_json(json!({ "type": "image", "id": "gone", "path": missing }), &recorder).err().expect("a missing image must fail the build");
    assert!(e.contains("image") && e.contains("missing.png"), "{e}");
}

/// Two nodes with the same id are refused by `build` too (a tree made by
/// hand, not by `parse`).
fn duplicate_ids_are_refused_at_build() {
    let label = |id: &str| Node { id: Some(id.to_string()), classes: vec![], visible: true, kind: Kind::Label { text: id.to_string() } };
    let node = Node { id: None, classes: vec![], visible: true, kind: Kind::Box { orientation: Orientation::Vertical, spacing: 0, children: vec![label("x"), label("x")] } };
    let recorder = Rc::new(Recorder::default());
    let e = build(&node, recorder).err().expect("duplicate ids must fail the build");
    assert!(e.contains("\"x\"") && e.contains("duplicate"), "{e}");
}

/// End to end: `opus.ui.show` in the script, the `Event::Ui` tree built
/// with `Runtime::dispatcher`, a click runs the script's handler. Once the
/// runtime is dropped a click is logged by the dispatcher and does nothing.
fn a_click_reaches_the_applet_script_and_is_logged_after_the_runtime_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let code = r#"opus.ui.show({ type: "box", children: [{ type: "button", id: "go", label: "Go", onClick: () => opus.log("clicked") }] })"#;
    std::fs::write(dir.path().join("main.js"), code).unwrap();
    let manifest = manifest::parse(r#"{ "name": "Widget Test" }"#).unwrap();
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink: Sink = {
        let events = events.clone();
        Rc::new(move |e| events.borrow_mut().push(e))
    };
    let notifier: Notifier = Arc::new(|_: &Value| Err("no notifications in tests".to_string()));
    let rt = Runtime::start_with_notifier("widget-test", dir.path(), &manifest, Map::new(), sink, notifier).unwrap();
    let shown = events.borrow().iter().find_map(|e| if let Event::Ui(v) = e { Some(v.clone()) } else { None }).expect("show sent an Event::Ui");
    assert_eq!(shown["op"], "show");
    let node = ui_tree::parse(&shown["tree"]).unwrap();
    let tree = build(&node, rt.dispatcher()).unwrap();
    let button = get::<gtk::Button>(&tree, "go");
    button.emit_clicked();
    assert!(events.borrow().contains(&Event::Log("clicked".to_string())), "{:?}", events.borrow());
    drop(rt);
    let count = events.borrow().len();
    button.emit_clicked();
    assert_eq!(events.borrow().len(), count, "a click after the runtime is gone did something: {:?}", events.borrow());
}

/// `set_canvas_commands` stores the commands the draw function replays; an
/// unknown id, a widget that is not a canvas and too many commands are
/// errors that keep the previous picture.
fn set_canvas_commands_stores_them_and_refuses_unknown_ids() {
    let recorder = Rc::new(Recorder::default());
    let tree = json!({ "type": "box", "children": [
        { "type": "canvas", "id": "c", "width": 20, "height": 20, "draw": { "handler": 1 } },
        { "type": "label", "id": "t", "text": "x" }
    ] });
    let tree = build_json(tree, &recorder).unwrap();
    assert_eq!(canvas_commands(&tree, "c"), Some(vec![]), "a new canvas has no picture yet");
    let commands = vec![Command::FillStyle { style: Style::Color("#f00".to_string()) }, Command::FillRect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }];
    set_canvas_commands(&tree, "c", commands.clone()).unwrap();
    assert_eq!(canvas_commands(&tree, "c"), Some(commands.clone()));
    let e = set_canvas_commands(&tree, "nope", vec![]).unwrap_err();
    assert!(e.contains("\"nope\""), "{e}");
    let e = set_canvas_commands(&tree, "t", vec![]).unwrap_err();
    assert!(e.contains("\"t\"") && e.contains("label"), "{e}");
    let e = set_canvas_commands(&tree, "c", vec![Command::BeginPath; crate::canvas::MAX_COMMANDS + 1]).unwrap_err();
    assert!(e.contains("100000"), "{e}");
    assert_eq!(canvas_commands(&tree, "c"), Some(commands), "a refused picture keeps the previous one");
    assert!(recorder.calls().is_empty(), "the canvas never calls the applet: {:?}", recorder.calls());
}

/// End to end: the script shows a canvas, its `Event::Redraw` is parsed
/// and stored in the built tree's drawing area.
fn a_script_canvas_reaches_its_drawing_area() {
    let dir = tempfile::tempdir().unwrap();
    let code = r##"opus.ui.show({ type: "canvas", id: "c", width: 20, height: 20, draw: (ctx) => { ctx.fillStyle = "#f00"; ctx.fillRect(0, 0, 10, 10); } })"##;
    std::fs::write(dir.path().join("main.js"), code).unwrap();
    let manifest = manifest::parse(r#"{ "name": "Canvas Test" }"#).unwrap();
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink: Sink = {
        let events = events.clone();
        Rc::new(move |e| events.borrow_mut().push(e))
    };
    let notifier: Notifier = Arc::new(|_: &Value| Err("no notifications in tests".to_string()));
    let rt = Runtime::start_with_notifier("canvas-test", dir.path(), &manifest, Map::new(), sink, notifier).unwrap();
    let shown = events.borrow().iter().find_map(|e| if let Event::Ui(v) = e { Some(v.clone()) } else { None }).expect("show sent an Event::Ui");
    let tree = build(&ui_tree::parse(&shown["tree"]).unwrap(), rt.dispatcher()).unwrap();
    let (id, commands) = events.borrow().iter().find_map(|e| if let Event::Redraw { id, commands } = e { Some((id.clone(), commands.clone())) } else { None }).expect("show sent an Event::Redraw");
    set_canvas_commands(&tree, &id, crate::canvas::parse(&commands).unwrap()).unwrap();
    assert_eq!(canvas_commands(&tree, "c").map(|c| c.len()), Some(2));
}

/// A painter of canvas `id` holding `commands`.
fn painter(commands: Vec<Command>) -> Painter {
    Painter::new("c", Rc::new(RefCell::new(commands)))
}

/// Paint with `painter` on a fresh 20x20 surface; the error it logged.
fn paint_once(painter: &Painter) -> Option<String> {
    let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 20, 20).unwrap();
    let cr = gtk::cairo::Context::new(&surface).unwrap();
    painter.paint(&cr)
}

/// Decision 3: a replay error is logged once, again only when it changes,
/// and the last one stays visible in the painter. No GTK needed.
#[test]
fn a_replay_error_is_logged_once_until_it_changes() {
    let bad = |colour: &str| vec![Command::FillStyle { style: Style::Color(colour.to_string()) }];
    let p = painter(bad("nonsense"));
    let first = paint_once(&p).expect("the first failure is logged");
    assert!(first.contains("nonsense"), "{first}");
    assert_eq!(paint_once(&p), None, "the same error again is not logged again");
    assert_eq!(p.last_error().as_deref(), Some(first.as_str()));
    *p.commands.borrow_mut() = bad("other");
    assert!(paint_once(&p).expect("a different error is logged").contains("other"));
    *p.commands.borrow_mut() = vec![];
    assert_eq!(paint_once(&p), None);
    assert_eq!(p.last_error(), None, "a good frame clears the last error");
    *p.commands.borrow_mut() = bad("other");
    assert!(paint_once(&p).is_some(), "after a good frame the error is logged again");
}

/// Task 8c decision 6: a NEW replay error (after the dedupe) is passed to
/// the listener; the same error again is not. No GTK needed.
#[test]
fn a_new_replay_error_reaches_the_listener_once() {
    let bad = |colour: &str| vec![Command::FillStyle { style: Style::Color(colour.to_string()) }];
    let p = painter(bad("nonsense"));
    let got: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = got.clone();
    p.set_listener(Rc::new(move |e: String| sink.borrow_mut().push(e)));
    paint_once(&p);
    paint_once(&p);
    assert_eq!(got.borrow().len(), 1, "{:?}", got.borrow());
    assert!(got.borrow()[0].contains("nonsense"));
    *p.commands.borrow_mut() = bad("other");
    paint_once(&p);
    assert_eq!(got.borrow().len(), 2);
    *p.commands.borrow_mut() = vec![];
    paint_once(&p);
    assert_eq!(got.borrow().len(), 2, "a good frame is not an error");
}

/// `set_error_listener` reaches every canvas of the tree, with or without an id.
fn the_error_listener_reaches_every_canvas_of_the_tree() {
    let recorder = Rc::new(Recorder::default());
    let tree = json!({ "type": "box", "children": [
        { "type": "canvas", "id": "c", "width": 20, "height": 20 },
        { "type": "canvas", "width": 20, "height": 20 }
    ] });
    let tree = build_json(tree, &recorder).unwrap();
    let got: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = got.clone();
    set_error_listener(&tree, Rc::new(move |e: String| sink.borrow_mut().push(e)));
    assert_eq!(tree.painters.len(), 2);
    for p in &tree.painters {
        *p.commands.borrow_mut() = vec![Command::FillStyle { style: Style::Color("zzz".to_string()) }];
        paint_once(p);
    }
    assert_eq!(got.borrow().len(), 2, "{:?}", got.borrow());
}
