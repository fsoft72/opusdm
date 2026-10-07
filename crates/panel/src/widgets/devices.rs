//! The device notifier icon and its drop-down, in the right side of the panel.

use crate::devices_center::{DeviceCenter, Op};
use crate::devices_model::{Action, Row};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_devices::{human_size, Device};
use std::rc::Rc;

/// Size of the drive icon in pixels.
const ICON_SIZE: i32 = 20;

/// Title of a row: label and size, for example `SEAGATE (16 TB)`.
pub fn row_title(d: &Device) -> String {
    format!("{} ({})", d.label, human_size(d.size))
}

/// Second line of a row: the mount point, or that it is not mounted.
pub fn row_subtitle(d: &Device) -> String {
    match &d.mount_point {
        Some(p) => p.display().to_string(),
        None => "Not mounted".to_string(),
    }
}

/// The button and its popover. Hidden while there is nothing to show.
pub struct DeviceButton {
    root: gtk::Box,
    list: gtk::Box,
    message: gtk::Label,
}

impl DeviceButton {
    /// Build the button; `refresh` fills it.
    pub fn new(_center: &Rc<DeviceCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);

        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_SIZE);
        icon.set_content_height(ICON_SIZE);
        icon.set_draw_func(|area, cr, w, h| draw_icon(area, cr, w, h));
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));
        button.set_tooltip_text(Some("Removable devices"));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);
        let message = gtk::Label::new(None);
        message.add_css_class("error");
        message.set_visible(false);
        message.set_xalign(0.0);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.append(&message);
        content.append(&list);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));

        root.append(&button);
        Rc::new(Self { root, list, message })
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Rebuild the rows from the shared list.
    pub fn refresh(&self, center: &Rc<DeviceCenter>) {
        let rows = center.rows();
        self.root.set_visible(!center.is_empty());
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for row in &rows {
            self.list.append(&build_row(center, row));
        }
    }
}

/// One row: names on the left, the actions (or a spinner) on the right.
fn build_row(center: &Rc<DeviceCenter>, row: &Row) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.set_hexpand(true);
    let title = gtk::Label::new(Some(&row_title(&row.device)));
    title.set_xalign(0.0);
    title.add_css_class("heading");
    let sub = gtk::Label::new(Some(&row_subtitle(&row.device)));
    sub.set_xalign(0.0);
    sub.add_css_class("dim-label");
    names.append(&title);
    names.append(&sub);
    line.append(&names);

    if row.busy {
        let spinner = gtk::Spinner::new();
        spinner.start();
        line.append(&spinner);
        return line;
    }
    for action in &row.actions {
        let (text, op) = match action {
            Action::Mount => ("Mount", Op::Mount(row.device.path.clone())),
            Action::Open => ("Open", Op::MountAndOpen(row.device.path.clone())),
            Action::Unmount => ("Unmount", Op::Unmount(row.device.path.clone())),
            Action::Eject => ("Eject", Op::Eject(row.device.path.clone())),
        };
        let button = gtk::Button::with_label(text);
        let center = center.clone();
        let mount_point = row.device.mount_point.clone();
        button.connect_clicked(move |_| match (&op, &mount_point) {
            // Already mounted: just open it, there is nothing to run.
            (Op::MountAndOpen(_), Some(p)) => (center.on_open())(p),
            _ => center.run(op.clone()),
        });
        line.append(&button);
    }
    line
}

/// A small external drive: a rounded body with a light, in the text colour.
fn draw_icon(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32) {
    let c = area.color();
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
    let (w, h) = (w as f64, h as f64);
    let (x, y, bw, bh, r) = (1.0, h * 0.30, w - 2.0, h * 0.50, 3.0);
    cr.new_sub_path();
    cr.arc(x + bw - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + bw - r, y + bh - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + bh - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
    cr.set_line_width(1.6);
    let _ = cr.stroke();
    cr.arc(w - 5.5, y + bh / 2.0, 1.4, 0.0, 2.0 * std::f64::consts::PI);
    let _ = cr.fill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_devices::Kind;

    fn dev(mount: Option<&str>) -> Device {
        Device {
            path: "/b/sdc1".into(),
            drive_path: "/d/A".into(),
            label: "SEAGATE".into(),
            size: 16_000_898_564_096,
            dev_node: "/dev/sdc1".into(),
            fs_type: "xfs".into(),
            mount_point: mount.map(Into::into),
            kind: Kind::Usb,
            can_eject: false,
            can_power_off: true,
            drive_name: String::new(),
        }
    }

    #[test]
    fn titles_and_subtitles() {
        assert_eq!(row_title(&dev(None)), "SEAGATE (16 TB)");
        assert_eq!(row_subtitle(&dev(None)), "Not mounted");
        assert_eq!(row_subtitle(&dev(Some("/media/fabio/SEAGATE"))), "/media/fabio/SEAGATE");
    }
}
