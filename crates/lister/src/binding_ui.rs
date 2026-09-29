//! Small widgets shared by the hotkey, user-menu and script-event editors.

use gtk4 as gtk;
use opusdm_ipc::function::{Function, FunctionId};

/// A drop-down listing every function by name, with `selected` (a function
/// id) pre-selected. Nothing is selected when the id is not in the list: a
/// dangling reference, which the editors flag as an error.
pub fn function_dropdown(functions: &[Function], selected: &str) -> gtk::DropDown {
    let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&names);
    let position = functions
        .iter()
        .position(|f| f.id == selected)
        .map(|p| p as u32)
        .unwrap_or(gtk::INVALID_LIST_POSITION);
    dropdown.set_selected(position);
    dropdown
}

/// The id of the function chosen in `dropdown`, or `None` when nothing is
/// selected.
pub fn selected_function(dropdown: &gtk::DropDown, functions: &[Function]) -> Option<FunctionId> {
    functions.get(dropdown.selected() as usize).map(|f| f.id.clone())
}
