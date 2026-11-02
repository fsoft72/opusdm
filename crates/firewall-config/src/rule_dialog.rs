//! The "Add rule" dialog: the form of one new rule. It only hands the checked
//! rule to the caller; nothing changes on the system until Apply.

use crate::model::{Action, Direction, NewRule, Protocol};
use gtk4 as gtk;
use gtk::prelude::*;

/// Size of the dialog.
const DIALOG_W: i32 = 420;
const DIALOG_H: i32 = 300;
/// Gap between the fields.
const SPACING: i32 = 6;
/// What the dialog says about the order of the rules.
const ORDER_HINT: &str = "The new rule goes after the existing ones, and ufw uses the first rule that matches: a deny after a wider allow has no effect.";

/// Add a row with a caption and `field` to `grid`.
fn add_row(grid: &gtk::Grid, row: i32, caption: &str, field: &impl IsA<gtk::Widget>) {
    let label = gtk::Label::new(Some(caption));
    label.set_xalign(0.0);
    grid.attach(&label, 0, row, 1, 1);
    field.set_hexpand(true);
    grid.attach(field, 1, row, 1, 1);
}

/// Open the dialog on `direction`. `on_add` gets the rule when the form is
/// valid and the dialog closes; an invalid form says why and stays open.
pub fn open(parent: &gtk::Window, direction: Direction, on_add: impl Fn(NewRule) + 'static) {
    let shell = opusdm_ui::DialogShell::new(parent, "Add Rule").size(DIALOG_W, DIALOG_H);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(SPACING as u32);
    grid.set_column_spacing(SPACING as u32 * 2);
    let dir_field = gtk::DropDown::from_strings(&["IN", "OUT"]);
    dir_field.set_selected(if direction == Direction::In { 0 } else { 1 });
    let port = gtk::Entry::new();
    port.set_placeholder_text(Some("for example 8080"));
    let proto = gtk::DropDown::from_strings(&["tcp", "udp", "tcp and udp"]);
    let action = gtk::DropDown::from_strings(&["allow", "deny"]);
    let address = gtk::Entry::new();
    address.set_placeholder_text(Some("anywhere, or 192.168.1.0/24"));
    add_row(&grid, 0, "Direction", &dir_field);
    add_row(&grid, 1, "Port", &port);
    add_row(&grid, 2, "Protocol", &proto);
    add_row(&grid, 3, "Action", &action);
    add_row(&grid, 4, "Address", &address);
    let hint = gtk::Label::new(Some(ORDER_HINT));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("dim-label");
    let error = gtk::Label::new(None);
    error.set_wrap(true);
    error.set_xalign(0.0);
    error.add_css_class("error");
    error.set_visible(false);
    let root = shell.content();
    root.append(&grid);
    root.append(&hint);
    root.append(&error);

    let add = shell.button("Add");
    shell.close_button("Cancel");
    let window = shell.window().clone();
    let submit = {
        let (port, address) = (port.clone(), address.clone());
        move || {
            let direction = if dir_field.selected() == 0 { Direction::In } else { Direction::Out };
            let proto = match proto.selected() { 0 => Protocol::Tcp, 1 => Protocol::Udp, _ => Protocol::Both };
            let action = if action.selected() == 0 { Action::Allow } else { Action::Deny };
            match NewRule::from_form(direction, &port.text(), proto, action, &address.text()) {
                Ok(rule) => {
                    on_add(rule);
                    window.close();
                }
                Err(why) => {
                    error.set_text(&why);
                    error.set_visible(true);
                }
            }
        }
    };
    let submit = std::rc::Rc::new(submit);
    {
        let submit = submit.clone();
        add.connect_clicked(move |_| submit());
    }
    {
        let submit = submit.clone();
        port.connect_activate(move |_| submit());
    }
    address.connect_activate(move |_| submit());
    shell.present();
    port.grab_focus();
}
