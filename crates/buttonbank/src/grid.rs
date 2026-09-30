//! The grid of buttons shared by the bank window and the editor preview.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{Btn, ButtonBank, MouseButton};
use std::path::Path;
use std::rc::Rc;

/// Pixel size of a button icon.
const ICON_PIXELS: i32 = 32;
/// Smallest size of a button, so an empty one is still clickable.
const MIN_BUTTON_WIDTH: i32 = 48;
const MIN_BUTTON_HEIGHT: i32 = 32;
/// Space between buttons and around the grid.
const GRID_SPACING: u32 = 4;
const GRID_MARGIN: i32 = 4;
/// Tooltip of a button whose Functions were all deleted.
pub const MISSING_TOOLTIP: &str = "Function not found (it may have been deleted)";

/// Whether `btn` has at least one bound Function and none of them exists
/// (`exists` says whether a Function id is still saved). Such a button is
/// shown insensitive. A button with nothing bound is left alone.
pub fn all_bound_functions_missing(btn: &Btn, exists: &dyn Fn(&str) -> bool) -> bool {
    let bound: Vec<&str> = [&btn.left, &btn.right, &btn.middle].into_iter().flatten().map(String::as_str).collect();
    !bound.is_empty() && bound.iter().all(|id| !exists(id))
}

/// One button: the icon (when the file is there) above the label.
fn build_button(btn: &Btn) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
    if let Some(icon) = btn.icon.as_deref().filter(|p| Path::new(p).is_file()) {
        let image = gtk::Image::from_file(icon);
        image.set_pixel_size(ICON_PIXELS);
        content.append(&image);
    }
    if !btn.label.is_empty() {
        content.append(&gtk::Label::new(Some(&btn.label)));
    }
    let button = gtk::Button::new();
    button.set_child(Some(&content));
    button.set_size_request(MIN_BUTTON_WIDTH, MIN_BUTTON_HEIGHT);
    button
}

/// Build the grid of `bank`'s buttons. `on_click(index, mouse_button)` is
/// called for a left, right or middle click on button `index`.
/// `function_exists` decides which buttons are shown insensitive.
pub fn build_grid(
    bank: &ButtonBank,
    on_click: Rc<dyn Fn(usize, MouseButton)>,
    function_exists: &dyn Fn(&str) -> bool,
) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(GRID_SPACING);
    grid.set_column_spacing(GRID_SPACING);
    grid.set_margin_start(GRID_MARGIN);
    grid.set_margin_end(GRID_MARGIN);
    grid.set_margin_top(GRID_MARGIN);
    grid.set_margin_bottom(GRID_MARGIN);
    for (index, btn) in bank.buttons.iter().enumerate() {
        let (row, column) = bank.grid_position(index);
        let button = build_button(btn);
        if all_bound_functions_missing(btn, function_exists) {
            button.set_sensitive(false);
            button.set_tooltip_text(Some(MISSING_TOOLTIP));
        }
        let gesture = gtk::GestureClick::new();
        gesture.set_button(0);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let on_click = on_click.clone();
        gesture.connect_released(move |g, _, _, _| {
            let mouse = match g.current_button() {
                1 => MouseButton::Left,
                2 => MouseButton::Middle,
                3 => MouseButton::Right,
                _ => return,
            };
            on_click(index, mouse);
        });
        button.add_controller(gesture);
        grid.attach(&button, column as i32, row as i32, 1, 1);
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::Btn;

    fn btn(left: Option<&str>, right: Option<&str>) -> Btn {
        Btn { left: left.map(String::from), right: right.map(String::from), ..Btn::default() }
    }

    #[test]
    fn a_button_with_no_functions_is_not_disabled() {
        assert!(!all_bound_functions_missing(&btn(None, None), &|_| false));
    }

    #[test]
    fn a_button_is_disabled_only_when_every_bound_function_is_gone() {
        let exists = |id: &str| id == "alive";
        assert!(all_bound_functions_missing(&btn(Some("dead"), None), &exists));
        assert!(all_bound_functions_missing(&btn(Some("dead"), Some("gone")), &exists));
        assert!(!all_bound_functions_missing(&btn(Some("dead"), Some("alive")), &exists));
        assert!(!all_bound_functions_missing(&btn(Some("alive"), None), &exists));
    }
}
