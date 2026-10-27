use super::*;
use serde_json::json;

/// `levels` boxes nested in each other, a label in the deepest one.
fn nested(levels: usize) -> Value {
    let mut node = json!({ "type": "label", "text": "leaf" });
    for _ in 1..levels {
        node = json!({ "type": "box", "children": [node] });
    }
    node
}

/// The error of parsing `value`.
fn error_of(value: Value) -> String {
    parse(&value).expect_err("the tree should be refused")
}

#[test]
fn unknown_type_is_an_error_with_the_type_name() {
    let e = error_of(json!({ "type": "slider", "id": "s" }));
    assert!(e.contains("slider"), "{e}");
    let e = error_of(json!({ "type": "box", "children": [{ "type": "gauge" }] }));
    assert!(e.contains("gauge"), "{e}");
}

#[test]
fn a_button_without_a_label_is_an_error() {
    let e = error_of(json!({ "type": "button", "id": "ok" }));
    assert!(e.contains("button") && e.contains("ok") && e.contains("label"), "{e}");
}

#[test]
fn handler_ids_are_kept() {
    let tree = json!({ "type": "box", "children": [
        { "type": "button", "label": "Go", "onClick": { "handler": 3 } },
        { "type": "entry", "text": "a", "onChange": { "handler": 4 } },
        { "type": "switch", "active": true, "onToggle": { "handler": 5.0 } },
        { "type": "list", "items": [{ "text": "one", "onClick": { "handler": 6 } }, { "text": "two" }] },
        { "type": "canvas", "width": 10, "height": 20, "draw": { "handler": 7 } },
        { "type": "button", "label": "None", "onClick": null }
    ] });
    let node = parse(&tree).unwrap();
    let Kind::Box { children, .. } = node.kind else { panic!("not a box") };
    assert_eq!(children[0].kind, Kind::Button { label: "Go".into(), on_click: Some(3) });
    assert_eq!(children[1].kind, Kind::Entry { text: "a".into(), on_change: Some(4) });
    assert_eq!(children[2].kind, Kind::Switch { active: true, on_toggle: Some(5) });
    assert_eq!(
        children[3].kind,
        Kind::List { items: vec![ListItem { text: "one".into(), on_click: Some(6) }, ListItem { text: "two".into(), on_click: None }] }
    );
    assert_eq!(children[4].kind, Kind::Canvas { width: 10, height: 20, draw: Some(7) });
    assert_eq!(children[5].kind, Kind::Button { label: "None".into(), on_click: None });
}

#[test]
fn nesting_deeper_than_32_levels_is_rejected() {
    assert!(parse(&nested(MAX_DEPTH)).is_ok());
    let e = error_of(nested(MAX_DEPTH + 1));
    assert!(e.contains("32"), "{e}");
}

#[test]
fn defaults_and_common_props() {
    let node = parse(&json!({ "type": "box", "id": "root", "class": "a  b", "visible": false })).unwrap();
    assert_eq!(node.id.as_deref(), Some("root"));
    assert_eq!(node.classes, ["a", "b"]);
    assert!(!node.visible);
    assert_eq!(node.kind, Kind::Box { orientation: Orientation::Vertical, spacing: 0, children: vec![] });
    let node = parse(&json!({ "type": "box", "orientation": "horizontal", "spacing": 6, "class": ["x"] })).unwrap();
    assert!(node.visible);
    assert_eq!(node.classes, ["x"]);
    assert_eq!(node.kind, Kind::Box { orientation: Orientation::Horizontal, spacing: 6, children: vec![] });
    let node = parse(&json!({ "type": "entry" })).unwrap();
    assert_eq!(node.kind, Kind::Entry { text: String::new(), on_change: None });
    let node = parse(&json!({ "type": "image", "path": "/a/b.png", "width": 16 })).unwrap();
    assert_eq!(node.kind, Kind::Image { path: "/a/b.png".into(), width: Some(16), height: None });
}

#[test]
fn bad_props_are_errors_naming_the_node_and_the_prop() {
    let e = error_of(json!({ "type": "label", "id": "t", "text": 5 }));
    assert!(e.contains("label") && e.contains("\"t\"") && e.contains("text"), "{e}");
    let e = error_of(json!({ "type": "label", "text": "x", "colour": "red" }));
    assert!(e.contains("label") && e.contains("colour"), "{e}");
    let e = error_of(json!({ "type": "box", "orientation": "diagonal" }));
    assert!(e.contains("orientation") && e.contains("diagonal"), "{e}");
    let e = error_of(json!({ "type": "button", "label": "x", "onClick": 3 }));
    assert!(e.contains("onClick"), "{e}");
    let e = error_of(json!({ "type": "canvas", "width": 0, "height": 10 }));
    assert!(e.contains("width"), "{e}");
    let e = error_of(json!({ "type": "image", "path": "rel.png" }));
    assert!(e.contains("path") && e.contains("assetPath"), "{e}");
    let e = error_of(json!({ "type": "list", "id": "l", "items": [{ "text": "a", "onclick": { "handler": 1 } }] }));
    assert!(e.contains("list") && e.contains("onclick"), "{e}");
    let e = error_of(json!({ "type": "label", "text": "x", "visible": "yes" }));
    assert!(e.contains("visible"), "{e}");
    let e = error_of(json!({ "type": "label", "text": "x", "id": "" }));
    assert!(e.contains("id"), "{e}");
    let e = error_of(json!({ "text": "x" }));
    assert!(e.contains("type"), "{e}");
    let e = error_of(json!(["not", "a", "node"]));
    assert!(e.contains("object"), "{e}");
}

#[test]
fn duplicate_ids_are_an_error() {
    let e = error_of(json!({ "type": "box", "children": [{ "type": "label", "id": "x", "text": "1" }, { "type": "label", "id": "x", "text": "2" }] }));
    assert!(e.contains("\"x\"") && e.contains("duplicate"), "{e}");
}

#[test]
fn a_dropdown_keeps_items_selected_and_handler() {
    let node = parse(&json!({ "type": "dropdown", "id": "d", "items": ["a", "b"], "selected": 1.0, "onChange": { "handler": 8 } })).unwrap();
    assert_eq!(node.kind, Kind::Dropdown { items: vec!["a".into(), "b".into()], selected: Some(1), on_change: Some(8) });
    let node = parse(&json!({ "type": "dropdown", "items": [] })).unwrap();
    assert_eq!(node.kind, Kind::Dropdown { items: vec![], selected: None, on_change: None });
}

#[test]
fn a_bad_dropdown_is_an_error_naming_the_prop() {
    let e = error_of(json!({ "type": "dropdown", "id": "d" }));
    assert!(e.contains("dropdown") && e.contains("d") && e.contains("items"), "{e}");
    let e = error_of(json!({ "type": "dropdown", "id": "d", "items": ["a", 3] }));
    assert!(e.contains("items[1]") && e.contains("string"), "{e}");
    for selected in [json!(2), json!(-1), json!(0.5), json!("0")] {
        let e = error_of(json!({ "type": "dropdown", "id": "d", "items": ["a", "b"], "selected": selected }));
        assert!(e.contains("selected"), "{e}");
    }
    let e = error_of(json!({ "type": "dropdown", "items": [], "selected": 0 }));
    assert!(e.contains("selected"), "{e}");
    let e = error_of(json!({ "type": "dropdown", "items": [], "onClick": null, "foo": 1 }));
    assert!(e.contains("foo"), "{e}");
}
