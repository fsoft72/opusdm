//! The battery icon and its popover, in the right side of the panel: level,
//! charge state, time left, health, power profile and screen brightness, in
//! the spirit of the KDE Plasma battery applet.

use crate::audio_model::DragGuard;
use crate::battery_center::BatteryCenter;
use crate::battery_model::{health_text, status_text, time_text, IconInfo, Request};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_power::{PowerState, Profile};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

/// Size of the panel icon in pixels.
const ICON_SIZE: i32 = 20;
/// Size of the icon at the top of the popover, in pixels.
const BIG_ICON_SIZE: i32 = 44;
/// Width of the popover, in pixels.
const POPOVER_WIDTH: i32 = 300;
/// Width of the brightness percentage label, in characters.
const PERCENT_CHARS: i32 = 5;
/// The colour of a low battery.
const LOW_RGB: (f64, f64, f64) = (0.91, 0.30, 0.24);
/// Opacity of the fill while a bolt is drawn over it.
const PLUGGED_FILL_ALPHA: f64 = 0.55;

/// The button and its popover. Hidden while there is no battery.
pub struct BatteryButton {
    root: gtk::Box,
    icon: gtk::DrawingArea,
    big_icon: gtk::DrawingArea,
    info: Rc<Cell<IconInfo>>,
    button: gtk::MenuButton,
    message: gtk::Label,
    percent: gtk::Label,
    status: gtk::Label,
    level: gtk::LevelBar,
    time: gtk::Label,
    health: gtk::Label,
    profiles: ProfileRow,
    brightness: BrightnessRow,
}

/// The power profile buttons; one is pressed.
struct ProfileRow {
    root: gtk::Box,
    buttons: Vec<(Profile, gtk::ToggleButton)>,
    /// Set while `refresh` presses a button, so it does not send a request.
    updating: Rc<Cell<bool>>,
}

/// The screen brightness slider.
struct BrightnessRow {
    root: gtk::Box,
    scale: gtk::Scale,
    percent: gtk::Label,
    guard: Rc<RefCell<DragGuard>>,
}

impl BatteryButton {
    /// Build the button; `refresh` fills it.
    pub fn new(center: &Rc<BatteryCenter>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.set_valign(gtk::Align::Center);
        root.set_visible(false);

        let info = Rc::new(Cell::new(IconInfo::default()));
        let icon = battery_area(ICON_SIZE, &info);
        let button = gtk::MenuButton::new();
        button.add_css_class("flat");
        button.set_child(Some(&icon));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(8);
        content.set_margin_bottom(8);
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.set_width_request(POPOVER_WIDTH);
        let message = gtk::Label::new(None);
        message.add_css_class("error");
        message.set_visible(false);
        message.set_xalign(0.0);
        message.set_wrap(true);

        // Header: big icon, name and state, percentage on the right.
        let big_icon = battery_area(BIG_ICON_SIZE, &info);
        let title = gtk::Label::new(Some("Battery"));
        title.set_xalign(0.0);
        title.add_css_class("heading");
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.add_css_class("dim-label");
        let names = gtk::Box::new(gtk::Orientation::Vertical, 2);
        names.set_valign(gtk::Align::Center);
        names.set_hexpand(true);
        names.append(&title);
        names.append(&status);
        let percent = gtk::Label::new(None);
        percent.add_css_class("title-2");
        percent.set_valign(gtk::Align::Center);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.append(&big_icon);
        header.append(&names);
        header.append(&percent);

        let level = gtk::LevelBar::for_interval(0.0, 100.0);
        let time = dim_label();
        let health = dim_label();
        let profiles = ProfileRow::new(center);
        let brightness = BrightnessRow::new(center);

        content.append(&message);
        content.append(&header);
        content.append(&level);
        content.append(&time);
        content.append(&health);
        content.append(&profiles.root);
        content.append(&brightness.root);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        button.set_popover(Some(&popover));

        root.append(&button);
        Rc::new(Self {
            root, icon, big_icon, info, button, message, percent, status, level, time, health, profiles, brightness,
        })
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
    pub fn refresh(&self, center: &Rc<BatteryCenter>) {
        self.root.set_visible(center.is_visible());
        self.info.set(center.icon_info());
        self.icon.queue_draw();
        self.big_icon.queue_draw();
        self.button.set_tooltip_text(Some(&center.tooltip()));
        let msg = center.message();
        self.message.set_visible(msg.is_some());
        self.message.set_text(msg.as_deref().unwrap_or(""));
        let state = center.state();
        self.show_battery(&state);
        self.profiles.refresh(&state);
        self.brightness.refresh(center.brightness(), Instant::now());
    }

    /// Level, state, time left and health of the battery.
    fn show_battery(&self, state: &PowerState) {
        let Some(b) = &state.battery else { return };
        self.percent.set_text(&format!("{}%", b.percent));
        self.status.set_text(status_text(b, state.on_ac));
        self.level.set_value(b.percent as f64);
        show_optional(&self.time, time_text(b));
        show_optional(&self.health, health_text(b));
    }
}

impl ProfileRow {
    /// Build one linked button per profile; a click switches to it.
    fn new(center: &Rc<BatteryCenter>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("Power Profile"));
        let linked = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        linked.add_css_class("linked");
        linked.set_homogeneous(true);
        let updating = Rc::new(Cell::new(false));
        let mut buttons: Vec<(Profile, gtk::ToggleButton)> = Vec::new();
        for profile in Profile::ALL {
            let toggle = gtk::ToggleButton::with_label(profile.label());
            if let Some((_, first)) = buttons.first() {
                toggle.set_group(Some(first));
            }
            let (center, updating) = (center.clone(), updating.clone());
            toggle.connect_toggled(move |t| {
                if updating.get() || !t.is_active() { return; }
                center.send(Request::SetProfile(profile));
            });
            linked.append(&toggle);
            buttons.push((profile, toggle));
        }
        root.append(&linked);
        root.set_visible(false);
        Self { root, buttons, updating }
    }

    /// Press the active profile; hidden without power-profiles-daemon.
    fn refresh(&self, state: &PowerState) {
        self.root.set_visible(!state.profiles.is_empty());
        self.updating.set(true);
        for (profile, toggle) in &self.buttons {
            toggle.set_sensitive(state.profiles.contains(profile));
            toggle.set_active(state.active_profile == Some(*profile));
        }
        self.updating.set(false);
    }
}

impl BrightnessRow {
    /// Build the slider; a drag asks the brightness worker for the value.
    fn new(center: &Rc<BatteryCenter>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading("Display Brightness"));
        let scale = gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            opusdm_brightness::MIN_PERCENT as f64,
            opusdm_brightness::MAX_PERCENT as f64,
            1.0,
        );
        scale.set_hexpand(true);
        scale.set_draw_value(false);
        let percent = gtk::Label::new(None);
        percent.set_width_chars(PERCENT_CHARS);
        percent.set_xalign(1.0);
        let guard = Rc::new(RefCell::new(DragGuard::default()));
        {
            // `change-value` fires for the user only, never for `set_value`,
            // so a refresh cannot loop back into a request.
            let (center, guard, percent) = (center.clone(), guard.clone(), percent.clone());
            scale.connect_change_value(move |_, _, value| {
                guard.borrow_mut().touch(Instant::now());
                let p = opusdm_brightness::clamp_percent(value.round().clamp(0.0, 100.0) as u8);
                percent.set_text(&format!("{p}%"));
                center.set_brightness(p);
                glib::Propagation::Proceed
            });
        }
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.append(&scale);
        controls.append(&percent);
        root.append(&controls);
        root.set_visible(false);
        Self { root, scale, percent, guard }
    }

    /// Show the brightness; hidden while there is no way to change it.
    fn refresh(&self, brightness: Option<u8>, now: Instant) {
        self.root.set_visible(brightness.is_some());
        let Some(p) = brightness else { return };
        // A refresh must not yank the slider away from the user's finger.
        if self.guard.borrow().is_held(now) { return; }
        self.percent.set_text(&format!("{p}%"));
        self.scale.set_value(p as f64);
    }
}

/// A section title in the popover.
fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("heading");
    label
}

/// A dimmed line of text, hidden until it has something to say.
fn dim_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.add_css_class("dim-label");
    label.set_visible(false);
    label
}

/// Show `text` in `label`, or hide the label when there is none.
fn show_optional(label: &gtk::Label, text: Option<String>) {
    label.set_visible(text.is_some());
    label.set_text(text.as_deref().unwrap_or(""));
}

/// A square drawing area that paints the battery `info` describes.
fn battery_area(size: i32, info: &Rc<Cell<IconInfo>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_valign(gtk::Align::Center);
    let info = info.clone();
    area.set_draw_func(move |area, cr, w, h| draw_battery(area, cr, w, h, info.get()));
    area
}

/// Set the cairo source to the widget's text colour, or to red for a low battery.
fn set_color(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, low: bool, alpha: f64) {
    if low {
        return cr.set_source_rgba(LOW_RGB.0, LOW_RGB.1, LOW_RGB.2, alpha);
    }
    let c = area.color();
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * alpha);
}

/// A horizontal battery filled to its level, with a bolt cut over it when
/// plugged in. Proportional, so the same drawing serves every size.
fn draw_battery(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, h: i32, info: IconInfo) {
    let (w, h) = (w as f64, h as f64);
    let line = (w / 12.0).max(1.4);
    let (x, y) = (line / 2.0 + 0.5, h * 0.27);
    let (body_w, body_h) = (w * 0.86 - x, h * 0.46);
    set_color(area, cr, info.low, 1.0);
    cr.set_line_width(line);
    cr.rectangle(x, y, body_w, body_h);
    let _ = cr.stroke();
    // The terminal nub on the right.
    cr.rectangle(x + body_w + line / 2.0, h * 0.40, w * 0.08, h * 0.20);
    let _ = cr.fill();

    let inset = line * 1.5;
    let fill_w = (body_w - 2.0 * inset) * info.percent as f64 / 100.0;
    set_color(area, cr, info.low, if info.plugged { PLUGGED_FILL_ALPHA } else { 1.0 });
    cr.rectangle(x + inset, y + inset, fill_w, body_h - 2.0 * inset);
    let _ = cr.fill();

    if !info.plugged { return; }
    bolt_path(cr, x + body_w / 2.0, h / 2.0, h * 0.62);
    // Cut a clear outline first, so the bolt reads over any fill level.
    cr.set_operator(gtk::cairo::Operator::Clear);
    cr.set_line_width(line * 1.6);
    let _ = cr.stroke_preserve();
    cr.set_operator(gtk::cairo::Operator::Over);
    set_color(area, cr, false, 1.0);
    let _ = cr.fill();
}

/// A lightning bolt `height` tall, centred on `(cx, cy)`.
fn bolt_path(cr: &gtk::cairo::Context, cx: f64, cy: f64, height: f64) {
    const POINTS: [(f64, f64); 6] = [(0.12, -0.5), (-0.28, 0.08), (-0.02, 0.08), (-0.12, 0.5), (0.28, -0.08), (0.02, -0.08)];
    cr.new_path();
    for (i, (px, py)) in POINTS.iter().enumerate() {
        let (x, y) = (cx + px * height, cy + py * height);
        if i == 0 { cr.move_to(x, y) } else { cr.line_to(x, y) }
    }
    cr.close_path();
}
