//! The audio icon and its popover, in the right side of the panel.

use crate::audio_center::AudioCenter;
use crate::audio_model::{DragGuard, IconLevel, Request};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_audio::{AudioDevice, Kind};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

/// Size of the speaker icon in pixels.
const ICON_SIZE: i32 = 20;
/// Width of the percentage label, in characters.
const PERCENT_CHARS: i32 = 5;

/// The button and its popover. Hidden while the sound server is not reachable.
pub struct AudioButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    level: Rc<Cell<IconLevel>>,
    button: gtk::MenuButton,
    message: gtk::Label,
    outputs: Section,
    inputs: Section,
}

/// The controls of one kind: device list, slider and mute.
struct Section {
    kind: Kind,
    root: gtk::Box,
    list: gtk::Box,
    scale: gtk::Scale,
    mute: gtk::ToggleButton,
    percent: gtk::Label,
    /// Index of the default device the slider and the mute act on.
    target: Rc<Cell<Option<u32>>>,
    guard: Rc<RefCell<DragGuard>>,
    /// Set while `refresh` changes the mute button, so it does not send a request.
    updating: Rc<Cell<bool>>,
}

impl AudioButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<AudioCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);

        let level = Rc::new(Cell::new(IconLevel::Muted));
        let icon = gtk::DrawingArea::new();
        icon.set_content_width(ICON_SIZE);
        icon.set_content_height(ICON_SIZE);
        {
            let level = level.clone();
            icon.set_draw_func(move |area, cr, w, h| draw_icon(area, cr, w, h, level.get()));
        }
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_width_request(280);
        let message = gtk::Label::new(None);
        message.add_css_class("error");
        message.set_visible(false);
        message.set_xalign(0.0);
        let outputs = Section::new(center, Kind::Output, "Output");
        let inputs = Section::new(center, Kind::Input, "Input");
        content.append(&message);
        content.append(&outputs.root);
        content.append(&inputs.root);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));

        root.append(&button);
        Rc::new(Self { root, icon, level, button, message, outputs, inputs })
    }

    /// The widget to put in the bar.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Whether the widget is still inside a bar window.
    pub fn is_attached(&self) -> bool {
        self.root.parent().is_some()
    }

    /// Show the shared state.
    pub fn refresh(&self, center: &Rc<AudioCenter>) {
        self.root.set_visible(center.is_available());
        self.level.set(center.icon_level());
        self.icon.queue_draw();
        self.button.set_tooltip_text(Some(&center.tooltip()));
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        let now = Instant::now();
        self.outputs.refresh(center, now);
        self.inputs.refresh(center, now);
    }
}

impl Section {
    /// Build the controls of one kind and connect them to the center.
    fn new(center: &Rc<AudioCenter>, kind: Kind, title: &str) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let heading = gtk::Label::new(Some(title));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);

        let target: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
        let guard = Rc::new(RefCell::new(DragGuard::default()));
        let updating = Rc::new(Cell::new(false));

        let mute = gtk::ToggleButton::new();
        mute.set_tooltip_text(Some("Mute"));
        mute.set_child(Some(&mute_icon(&mute)));
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_hexpand(true);
        scale.set_draw_value(false);
        let percent = gtk::Label::new(Some("0%"));
        percent.set_width_chars(PERCENT_CHARS);
        percent.set_xalign(1.0);

        {
            // `change-value` fires for the user only, never for `set_value`,
            // so a refresh cannot loop back into a request.
            let (center, target, guard) = (center.clone(), target.clone(), guard.clone());
            scale.connect_change_value(move |_, _, value| {
                let Some(index) = target.get() else { return glib::Propagation::Proceed };
                guard.borrow_mut().touch(Instant::now());
                let percent = value.round().clamp(0.0, 100.0) as u32;
                center.send(Request::SetVolume { kind, index, percent });
                glib::Propagation::Proceed
            });
        }
        {
            let (center, target, updating) = (center.clone(), target.clone(), updating.clone());
            mute.connect_toggled(move |b| {
                if updating.get() {
                    return;
                }
                let Some(index) = target.get() else { return };
                center.send(Request::SetMute { kind, index, muted: b.is_active() });
            });
        }

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.append(&mute);
        controls.append(&scale);
        controls.append(&percent);
        root.append(&heading);
        root.append(&list);
        root.append(&controls);
        root.set_visible(false);
        Self { kind, root, list, scale, mute, percent, target, guard, updating }
    }

    /// Rebuild the device list and show the default device's state.
    fn refresh(&self, center: &Rc<AudioCenter>, now: Instant) {
        let devices = center.devices_of(self.kind);
        self.root.set_visible(!devices.is_empty());
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for d in &devices {
            self.list.append(&build_row(center, d));
        }
        let default = devices.iter().find(|d| d.is_default);
        self.target.set(default.map(|d| d.index));
        self.scale.set_sensitive(default.is_some());
        self.mute.set_sensitive(default.is_some());
        let Some(d) = default else { return };
        self.updating.set(true);
        self.mute.set_active(d.muted);
        self.updating.set(false);
        self.percent.set_text(&format!("{}%", d.volume_percent));
        // A server event must not yank the slider away from the user's finger.
        if !self.guard.borrow().is_held(now) {
            self.scale.set_value(d.volume_percent as f64);
        }
    }
}

/// One device row: a check mark on the default, then the name. A click makes it the default.
fn build_row(center: &Rc<AudioCenter>, d: &AudioDevice) -> gtk::Button {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let check = gtk::Label::new(Some(if d.is_default { "\u{2713}" } else { "" }));
    check.set_width_chars(2);
    let name = gtk::Label::new(Some(&d.description));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    line.append(&check);
    line.append(&name);
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.set_child(Some(&line));
    let (center, kind, device_name) = (center.clone(), d.kind, d.name.clone());
    button.connect_clicked(move |_| center.send(Request::SetDefault { kind, name: device_name.clone() }));
    button
}

/// The speaker drawn inside a mute toggle: with waves when sound is on, crossed when muted.
fn mute_icon(toggle: &gtk::ToggleButton) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(ICON_SIZE);
    area.set_content_height(ICON_SIZE);
    let weak = toggle.downgrade();
    area.set_draw_func(move |area, cr, w, h| {
        let muted = weak.upgrade().is_some_and(|t| t.is_active());
        draw_icon(area, cr, w, h, if muted { IconLevel::Muted } else { IconLevel::High });
    });
    let area_ref = area.clone();
    toggle.connect_toggled(move |_| area_ref.queue_draw());
    area
}

/// A speaker with 0 to 3 sound waves, or a cross when muted, in the text colour.
fn draw_icon(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32, level: IconLevel) {
    let c = area.color();
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
    let (w, h) = (w as f64, h as f64);
    cr.move_to(1.0, h * 0.38);
    cr.line_to(w * 0.28, h * 0.38);
    cr.line_to(w * 0.50, h * 0.18);
    cr.line_to(w * 0.50, h * 0.82);
    cr.line_to(w * 0.28, h * 0.62);
    cr.line_to(1.0, h * 0.62);
    cr.close_path();
    let _ = cr.fill();
    cr.set_line_width(1.6);
    let waves = match level {
        IconLevel::Muted => {
            cr.move_to(w * 0.62, h * 0.36);
            cr.line_to(w - 1.5, h * 0.64);
            cr.move_to(w - 1.5, h * 0.36);
            cr.line_to(w * 0.62, h * 0.64);
            let _ = cr.stroke();
            0
        }
        IconLevel::Low => 1,
        IconLevel::Medium => 2,
        IconLevel::High => 3,
    };
    for i in 0..waves {
        let radius = w * (0.18 + 0.14 * i as f64);
        cr.new_sub_path();
        cr.arc(w * 0.50, h * 0.50, radius, -std::f64::consts::FRAC_PI_4, std::f64::consts::FRAC_PI_4);
        let _ = cr.stroke();
    }
}
