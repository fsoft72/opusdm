//! Output windows of `Run` steps (F-46): a read-only text window per program,
//! filled as it prints and left open after it ends.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

/// What the window says when the program ended.
pub fn end_text(exit: Option<i32>) -> String {
    match exit {
        Some(0) => "\n[finished]\n".to_string(),
        Some(code) => format!("\n[finished with exit code {code}]\n"),
        None => "\n[stopped by a signal]\n".to_string(),
    }
}

/// Open the window `id`. The buffer is registered while the window is open,
/// so output arriving after it was closed is dropped.
pub fn open(ui: &Rc<Ui>, id: u64, title: &str) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, title).modal(false).size(640, 360);
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    let buffer = view.buffer();
    let scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();
    shell.content().append(&scroll);
    shell.close_button("Close");
    ui.output_windows.borrow_mut().insert(id, buffer);
    let window = shell.present();
    let ui = ui.clone();
    window.connect_close_request(move |_| {
        ui.output_windows.borrow_mut().remove(&id);
        gtk::glib::Propagation::Proceed
    });
}

/// Add `text` at the end of window `id`, if it is still open.
pub fn append(ui: &Rc<Ui>, id: u64, text: &str) {
    if let Some(buffer) = ui.output_windows.borrow().get(&id) {
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, text);
    }
}

/// The program of window `id` ended: say so and stop tracking the window.
pub fn end(ui: &Rc<Ui>, id: u64, exit: Option<i32>) {
    append(ui, id, &end_text(exit));
    ui.output_windows.borrow_mut().remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_closing_line_names_the_exit() {
        assert!(end_text(Some(0)).contains("finished]"));
        assert!(end_text(Some(3)).contains("exit code 3"));
        assert!(end_text(None).contains("signal"));
    }
}
