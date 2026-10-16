//! Small widget helpers every settings window uses: a section heading, a
//! label + control row, and emptying a container.

use gtk4 as gtk;
use gtk::prelude::*;

/// CSS class of a section heading (bold in the OpusDM chrome).
const HEADING_CLASS: &str = "heading";

/// A left-aligned section heading, e.g. "PANEL" or "Appearance".
pub fn section_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class(HEADING_CLASS);
    label
}

/// How wide the label of a [`labeled_row`] is, so the controls of several
/// rows line up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelWidth {
    /// As wide as its text.
    Natural,
    /// At least this many characters.
    Chars(i32),
    /// At least this many pixels.
    Pixels(i32),
}

/// A horizontal row: `text` as a left-aligned label of `width`, then
/// `widget`, `spacing` pixels apart. The widget keeps its own expand flags.
pub fn labeled_row(text: &str, width: LabelWidth, spacing: i32, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    match width {
        LabelWidth::Natural => {}
        LabelWidth::Chars(chars) => label.set_width_chars(chars),
        LabelWidth::Pixels(px) => label.set_size_request(px, -1),
    }
    row.append(&label);
    row.append(widget);
    row
}

/// A container whose children can be removed one by one.
pub trait RemoveChildren: IsA<gtk::Widget> {
    /// Remove `child` the way this container needs.
    fn remove_child(&self, child: &gtk::Widget);
}

impl RemoveChildren for gtk::Box {
    /// `gtk::Box::remove`.
    fn remove_child(&self, child: &gtk::Widget) {
        self.remove(child);
    }
}

impl RemoveChildren for gtk::ListBox {
    /// `gtk::ListBox::remove`.
    fn remove_child(&self, child: &gtk::Widget) {
        self.remove(child);
    }
}

impl RemoveChildren for gtk::FlowBox {
    /// `gtk::FlowBox::remove`.
    fn remove_child(&self, child: &gtk::Widget) {
        self.remove(child);
    }
}

/// Remove every child of `container`.
pub fn clear_children(container: &impl RemoveChildren) {
    while let Some(child) = container.first_child() {
        container.remove_child(&child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GTK may be used from one test thread only, so every widget check is
    /// in this one test.
    #[test]
    fn the_form_widgets_build_and_clear() {
        if gtk::init().is_err() { return; }
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&gtk::Label::new(None));
        column.append(&gtk::Label::new(None));
        clear_children(&column);
        assert!(column.first_child().is_none());
        let list = gtk::ListBox::new();
        list.append(&gtk::Label::new(None));
        clear_children(&list);
        assert!(list.first_child().is_none());

        let spin = gtk::SpinButton::with_range(0.0, 10.0, 1.0);
        let row = labeled_row("Count", LabelWidth::Chars(12), 8, &spin);
        let label = row.first_child().and_downcast::<gtk::Label>().unwrap();
        assert_eq!(label.text(), "Count");
        assert_eq!(label.width_chars(), 12);
        assert_eq!(row.last_child().as_ref(), Some(spin.upcast_ref::<gtk::Widget>()));
        assert!(section_heading("PANEL").has_css_class(HEADING_CLASS));
    }
}
