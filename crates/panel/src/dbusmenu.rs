//! `com.canonical.dbusmenu` client: fetches an SNI item's menu tree and
//! renders it as a `gtk::PopoverMenu`. Companion to `sni.rs` - most SNI
//! apps set `ItemIsMenu=true` and rely on the host (us) to show this menu
//! rather than reacting to a plain click.

use std::collections::HashMap;
use zbus::zvariant::OwnedValue;

#[derive(Clone, Debug, PartialEq)]
pub struct MenuNode {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub is_separator: bool,
    pub children: Vec<MenuNode>,
}

/// Build one `MenuNode` from an already-decoded property map and
/// already-parsed children, applying the DBusMenu spec's fallback
/// defaults for absent properties (`label`="", `enabled`/`visible`=true,
/// `type`!="separator" unless explicitly set).
pub fn parse_layout_node(id: i32, props: &HashMap<String, OwnedValue>, children: Vec<MenuNode>) -> MenuNode {
    let label = props
        .get("label")
        .and_then(|v| String::try_from(v.clone()).ok())
        .unwrap_or_default();
    let enabled = props
        .get("enabled")
        .and_then(|v| bool::try_from(v.clone()).ok())
        .unwrap_or(true);
    let visible = props
        .get("visible")
        .and_then(|v| bool::try_from(v.clone()).ok())
        .unwrap_or(true);
    let is_separator = props
        .get("type")
        .and_then(|v| String::try_from(v.clone()).ok())
        .is_some_and(|t| t == "separator");
    MenuNode { id, label, enabled, visible, is_separator, children }
}

/// Call `AboutToShow` (many apps lazily populate their menu here, per
/// spec convention) then `GetLayout` on `service`'s dbusmenu object at
/// `path`, and parse the result into a `MenuNode` tree.
pub fn fetch_layout(
    conn: &zbus::blocking::Connection,
    service: &str,
    path: &zbus::zvariant::OwnedObjectPath,
) -> Result<MenuNode, Box<dyn std::error::Error>> {
    let proxy = zbus::blocking::Proxy::new(conn, service, path.as_str(), "com.canonical.dbusmenu")?;
    let _ = proxy.call_method("AboutToShow", &(0_i32,));

    let (_revision, layout): (u32, LayoutTuple) =
        proxy.call("GetLayout", &(0_i32, -1_i32, Vec::<String>::new()))?;

    Ok(parse_layout_tuple(layout))
}

/// Recursively turn one `(id, properties, children)` zvariant tuple (each
/// child itself wrapped in a `Variant` since the array is heterogeneous by
/// depth) into a `MenuNode` tree, via `parse_layout_node` for the leaf work.
fn parse_layout_tuple(node: LayoutTuple) -> MenuNode {
    let (id, props, raw_children) = node;
    let children = raw_children
        .into_iter()
        .filter_map(|v| v.try_into().ok())
        .map(parse_layout_tuple)
        .collect();
    parse_layout_node(id, &props, children)
}

/// Send a `clicked` event for `id` back to `service`'s dbusmenu object -
/// call this right after the user activates a popover row.
pub fn send_event_clicked(
    conn: &zbus::blocking::Connection,
    service: &str,
    path: &zbus::zvariant::OwnedObjectPath,
    id: i32,
) {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let proxy = zbus::blocking::Proxy::new(conn, service, path.as_str(), "com.canonical.dbusmenu")?;
        let data = zbus::zvariant::Value::from(0_i32);
        proxy.call_method("Event", &(id, "clicked", data, 0_u32))?;
        Ok(())
    };
    if let Err(e) = run() {
        opusdm_ipc::err_log!("opusdm-panel", "dbusmenu Event failed for {service}: {e}");
    }
}

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::gio;

/// The `(id, properties, children)` tuple of a dbusmenu layout node.
type LayoutTuple = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// Build a `gtk::PopoverMenu` from `root`'s children (the root node itself
/// is never shown - it's the menu's invisible container). Each visible,
/// non-separator leaf/branch becomes one action; separators become menu
/// section breaks. Submenus nest as `gio::Menu` submenus. `on_click` fires
/// with the clicked node's dbusmenu id.
pub fn build_popover(root: &MenuNode, on_click: impl Fn(i32) + 'static) -> gtk::PopoverMenu {
    let actions = gio::SimpleActionGroup::new();
    let menu = build_gio_menu(&root.children, &actions, &std::rc::Rc::new(on_click));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.insert_action_group("dbusmenu", Some(&actions));
    popover
}

fn build_gio_menu(
    nodes: &[MenuNode],
    actions: &gio::SimpleActionGroup,
    on_click: &std::rc::Rc<impl Fn(i32) + 'static>,
) -> gio::Menu {
    let menu = gio::Menu::new();
    let mut section = gio::Menu::new();
    for node in nodes {
        if !node.visible {
            continue;
        }
        if node.is_separator {
            if section.n_items() > 0 {
                menu.append_section(None, &section);
                section = gio::Menu::new();
            }
            continue;
        }
        let action_name = format!("item{}", node.id);
        if node.children.is_empty() {
            let action = gio::SimpleAction::new(&action_name, None);
            action.set_enabled(node.enabled);
            {
                let on_click = on_click.clone();
                let id = node.id;
                action.connect_activate(move |_, _| on_click(id));
            }
            actions.add_action(&action);
            section.append(Some(&node.label), Some(&format!("dbusmenu.{action_name}")));
        } else {
            let submenu = build_gio_menu(&node.children, actions, on_click);
            section.append_submenu(Some(&node.label), &submenu);
        }
    }
    if section.n_items() > 0 {
        menu.append_section(None, &section);
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, OwnedValue)]) -> HashMap<String, OwnedValue> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn s(v: &str) -> OwnedValue {
        zbus::zvariant::Value::from(v).try_into().unwrap()
    }

    fn b(v: bool) -> OwnedValue {
        zbus::zvariant::Value::from(v).try_into().unwrap()
    }

    #[test]
    fn parse_layout_node_reads_label() {
        let p = props(&[("label", s("File"))]);
        let node = parse_layout_node(1, &p, vec![]);
        assert_eq!(node.label, "File");
        assert!(node.enabled);
        assert!(node.visible);
        assert!(!node.is_separator);
    }

    #[test]
    fn parse_layout_node_defaults_when_properties_absent() {
        let node = parse_layout_node(2, &HashMap::new(), vec![]);
        assert_eq!(node.label, "");
        assert!(node.enabled);
        assert!(node.visible);
    }

    #[test]
    fn parse_layout_node_detects_separator_type() {
        let p = props(&[("type", s("separator"))]);
        let node = parse_layout_node(3, &p, vec![]);
        assert!(node.is_separator);
    }

    #[test]
    fn parse_layout_node_respects_enabled_and_visible_false() {
        let p = props(&[
            ("enabled", b(false)),
            ("visible", b(false)),
        ]);
        let node = parse_layout_node(4, &p, vec![]);
        assert!(!node.enabled);
        assert!(!node.visible);
    }

    #[test]
    fn parse_layout_node_keeps_children() {
        let child = parse_layout_node(10, &HashMap::new(), vec![]);
        let node = parse_layout_node(1, &HashMap::new(), vec![child.clone()]);
        assert_eq!(node.children, vec![child]);
    }
}
