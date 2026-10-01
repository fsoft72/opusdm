//! "Size Format..." dialog: how this Lister writes the Size column.

use crate::model::SizeFormat;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

/// Open the dialog. The choice applies to this Lister only, for the session.
pub fn open(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Size Format").size(420, 160);

    let labels: Vec<&str> = SizeFormat::ALL.iter().map(|f| f.label()).collect();
    let choice = gtk::DropDown::from_strings(&labels);
    let current = ui.view.size_format.get();
    if let Some(pos) = SizeFormat::ALL.iter().position(|f| *f == current) {
        choice.set_selected(pos as u32);
    }
    shell.content().append(&gtk::Label::new(Some("Show file sizes as (this Lister only)")));
    shell.content().append(&choice);

    let ok_btn = shell.button("OK");
    shell.close_button("Cancel");

    let window = shell.window().clone();
    let ui = ui.clone();
    ok_btn.connect_clicked(move |_| {
        if let Some(fmt) = SizeFormat::ALL.get(choice.selected() as usize) {
            ui.view.size_format.set(*fmt);
            // Sizes are formatted when a row is bound: rebind, no disk access.
            let selected = ui.selected_names();
            ui.touch_rows();
            ui.select_names(&selected);
        }
        window.close();
    });
    shell.present();
}
