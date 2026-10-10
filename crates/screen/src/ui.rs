//! The Screens window: a canvas to arrange the monitors, a panel for the
//! selected monitor, the Extend / Duplicate / Only buttons and Apply with a
//! "keep these settings?" countdown.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_screen::backend::ScreenBackend;
use opusdm_screen::geometry::{fit_view, snap_position, Rect, View};
use opusdm_screen::session::{Session, State};
use opusdm_screen::{modes, same_rate, store, Layout, Monitor, MonitorConfig, Rotation, ScreenError};

/// Seconds to answer "keep these settings?" before the old layout returns.
const CONFIRM_SECS: u32 = 15;
/// Default window size.
const WINDOW_W: i32 = 760;
const WINDOW_H: i32 = 620;
/// Gap between the widgets.
const SPACING: i32 = 8;
/// Padding of the window.
const MARGIN: i32 = 12;
/// A dragged monitor snaps to an edge this close (screen pixels of the canvas).
const SNAP_SCREEN_PX: f64 = 12.0;
/// Tag of the log lines.
const APP_TAG: &str = "opusdm-screen";
/// Scales offered in the Scale list (the current one is added when missing).
const SCALES: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Text of the Rate list entry that lets `xrandr` choose.
const AUTO_RATE: &str = "Automatic";

/// A monitor being dragged.
struct Drag {
    name: String,
    start_pos: (i32, i32),
}

/// What every callback of the window shares.
struct Ui {
    session: RefCell<Session<Box<dyn ScreenBackend>>>,
    selected: RefCell<Option<String>>,
    view: Cell<View>,
    drag: RefCell<Option<Drag>>,
    /// Set while the widgets are filled from the session, so they do not
    /// answer their own changes.
    updating: Cell<bool>,
    timer: RefCell<Option<glib::SourceId>>,
    remaining: Cell<u32>,
    scale_options: RefCell<Vec<f64>>,
    sizes: RefCell<Vec<(u32, u32)>>,
    rates: RefCell<Vec<Option<f64>>>,
    area: gtk::DrawingArea,
    status: gtk::Label,
    pick: gtk::DropDown,
    enabled: gtk::CheckButton,
    resolution: gtk::DropDown,
    rate: gtk::DropDown,
    rotation: gtk::DropDown,
    scale: gtk::DropDown,
    primary: gtk::CheckButton,
    only_box: gtk::Box,
    remember: gtk::CheckButton,
    edit_bar: gtk::Box,
    confirm_bar: gtk::Box,
    confirm_label: gtk::Label,
    apply: gtk::Button,
    reset: gtk::Button,
    detect: gtk::Button,
    extend: gtk::Button,
    duplicate: gtk::Button,
}

/// A window that only says why the screens cannot be configured.
pub fn open_problem(app: &gtk::Application, problem: &str) {
    let window = gtk::ApplicationWindow::builder().application(app).title("Screens").default_width(420).build();
    let label = gtk::Label::new(Some(problem));
    label.set_wrap(true);
    label.set_margin_top(MARGIN);
    label.set_margin_bottom(MARGIN);
    label.set_margin_start(MARGIN);
    label.set_margin_end(MARGIN);
    window.set_child(Some(&label));
    window.present();
}

/// Open the Screens window on `backend`.
pub fn open(app: &gtk::Application, backend: Box<dyn ScreenBackend>) {
    match Session::open(backend) {
        Ok(session) => build(app, session),
        Err(e) => open_problem(app, &e.to_string()),
    }
}

/// A label and a widget in a row.
fn labeled(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
    let label = gtk::Label::new(Some(text));
    label.set_width_chars(12);
    label.set_xalign(0.0);
    row.append(&label);
    widget.set_hexpand(true);
    row.append(widget);
    row
}

/// A drop down with these entries.
fn drop_down(items: &[&str]) -> gtk::DropDown {
    gtk::DropDown::from_strings(items)
}

/// Show these entries in a drop down and select `index`. The model is only
/// replaced when the entries differ and the selection only when it moves:
/// this runs while the drop down is handling its own selection, and changing
/// its model then freezes the open list.
fn set_items(dropdown: &gtk::DropDown, items: &[String], index: usize) {
    let same = dropdown.model().and_then(|m| m.downcast::<gtk::StringList>().ok()).is_some_and(|list| {
        list.n_items() as usize == items.len() && items.iter().enumerate().all(|(i, text)| list.string(i as u32).as_deref() == Some(text.as_str()))
    });
    if !same {
        let refs: Vec<&str> = items.iter().map(String::as_str).collect();
        dropdown.set_model(Some(&gtk::StringList::new(&refs)));
    }
    if dropdown.selected() != index as u32 { dropdown.set_selected(index as u32); }
}

/// The text of a refresh rate.
fn rate_text(rate: f64) -> String {
    format!("{rate:.2} Hz")
}

/// The text of a scale.
fn scale_text(scale: f64) -> String {
    format!("{scale:.2}x")
}

fn build(app: &gtk::Application, session: Session<Box<dyn ScreenBackend>>) {
    let area = gtk::DrawingArea::new();
    area.set_content_height(240);
    area.set_hexpand(true);
    area.set_vexpand(true);
    let ui = Rc::new(Ui {
        session: RefCell::new(session),
        selected: RefCell::new(None),
        view: Cell::new(View { scale: 0.1, ox: 0.0, oy: 0.0 }),
        drag: RefCell::new(None),
        updating: Cell::new(false),
        timer: RefCell::new(None),
        remaining: Cell::new(0),
        scale_options: RefCell::new(Vec::new()),
        sizes: RefCell::new(Vec::new()),
        rates: RefCell::new(Vec::new()),
        area,
        status: gtk::Label::new(None),
        pick: drop_down(&[]),
        enabled: gtk::CheckButton::with_label("Enabled"),
        resolution: drop_down(&[]),
        rate: drop_down(&[]),
        rotation: drop_down(&Rotation::ALL.map(Rotation::label)),
        scale: drop_down(&[]),
        primary: gtk::CheckButton::with_label("Primary monitor"),
        only_box: gtk::Box::new(gtk::Orientation::Horizontal, SPACING),
        remember: gtk::CheckButton::with_label("Remember for these monitors"),
        edit_bar: gtk::Box::new(gtk::Orientation::Horizontal, SPACING),
        confirm_bar: gtk::Box::new(gtk::Orientation::Horizontal, SPACING),
        confirm_label: gtk::Label::new(None),
        apply: gtk::Button::with_label("Apply"),
        reset: gtk::Button::with_label("Reset"),
        detect: gtk::Button::with_label("Detect monitors"),
        extend: gtk::Button::with_label("Extend"),
        duplicate: gtk::Button::with_label("Duplicate"),
    });

    let window = gtk::ApplicationWindow::builder().application(app).title("Screens").default_width(WINDOW_W).default_height(WINDOW_H).build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    root.set_margin_top(MARGIN);
    root.set_margin_bottom(MARGIN);
    root.set_margin_start(MARGIN);
    root.set_margin_end(MARGIN);

    // Mode buttons.
    let modes_bar = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
    modes_bar.append(&ui.extend);
    modes_bar.append(&ui.duplicate);
    modes_bar.append(&ui.only_box);
    root.append(&modes_bar);
    root.append(&ui.area);

    // Panel of the selected monitor.
    ui.scale.set_tooltip_text(Some("Pixels drawn per screen pixel: 2.00x makes everything look smaller, 0.50x larger"));
    ui.primary.set_tooltip_text(Some("The monitor that gets the panel and new windows"));
    let panel = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    panel.append(&labeled("Monitor", &ui.pick));
    panel.append(&ui.enabled);
    panel.append(&labeled("Resolution", &ui.resolution));
    panel.append(&labeled("Refresh rate", &ui.rate));
    panel.append(&labeled("Rotation", &ui.rotation));
    panel.append(&labeled("Scale", &ui.scale));
    panel.append(&ui.primary);
    // Wayland has no primary monitor.
    ui.primary.set_visible(!opusdm_ipc::session::is_wayland());
    root.append(&panel);

    ui.status.set_xalign(0.0);
    ui.status.set_wrap(true);
    root.append(&ui.status);

    // Bottom bars: Apply, or the question after Apply.
    ui.remember.set_active(true);
    ui.edit_bar.append(&ui.detect);
    ui.edit_bar.append(&ui.remember);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    ui.edit_bar.append(&spacer);
    ui.edit_bar.append(&ui.reset);
    ui.edit_bar.append(&ui.apply);
    root.append(&ui.edit_bar);

    let keep = gtk::Button::with_label("Keep");
    let revert = gtk::Button::with_label("Revert");
    ui.confirm_label.set_hexpand(true);
    ui.confirm_label.set_xalign(0.0);
    ui.confirm_bar.append(&ui.confirm_label);
    ui.confirm_bar.append(&revert);
    ui.confirm_bar.append(&keep);
    ui.confirm_bar.set_visible(false);
    root.append(&ui.confirm_bar);

    window.set_child(Some(&root));
    connect(&ui, &window, &keep, &revert);
    ui.rebuild_monitors();
    window.present();
}

/// Wire every widget to the window state.
fn connect(ui: &Rc<Ui>, window: &gtk::ApplicationWindow, keep: &gtk::Button, revert: &gtk::Button) {
    let u = ui.clone();
    ui.area.set_draw_func(move |_, cr, w, h| u.draw(cr, w, h));

    let drag = gtk::GestureDrag::new();
    let u = ui.clone();
    drag.connect_drag_begin(move |_, x, y| u.drag_begin(x, y));
    let u = ui.clone();
    drag.connect_drag_update(move |_, dx, dy| u.drag_update(dx, dy));
    let u = ui.clone();
    drag.connect_drag_end(move |_, _, _| u.drag_end());
    ui.area.add_controller(drag);

    let u = ui.clone();
    ui.pick.connect_selected_notify(move |d| u.on_pick(d.selected()));
    let u = ui.clone();
    ui.enabled.connect_toggled(move |c| u.on_enabled(c.is_active()));
    let u = ui.clone();
    ui.resolution.connect_selected_notify(move |d| u.on_resolution(d.selected()));
    let u = ui.clone();
    ui.rate.connect_selected_notify(move |d| u.on_rate(d.selected()));
    let u = ui.clone();
    ui.rotation.connect_selected_notify(move |d| u.on_rotation(d.selected()));
    let u = ui.clone();
    ui.scale.connect_selected_notify(move |d| u.on_scale(d.selected()));
    let u = ui.clone();
    ui.primary.connect_toggled(move |c| u.on_primary(c.is_active()));

    let u = ui.clone();
    ui.extend.connect_clicked(move |_| u.on_mode(modes::extend));
    let u = ui.clone();
    ui.duplicate.connect_clicked(move |_| u.on_mode(modes::mirror));
    let u = ui.clone();
    ui.reset.connect_clicked(move |_| {
        u.session.borrow_mut().reset();
        u.status.set_text("");
        u.refresh();
    });
    let u = ui.clone();
    ui.detect.connect_clicked(move |_| u.on_detect());
    let u = ui.clone();
    ui.apply.connect_clicked(move |_| u.on_apply());
    let u = ui.clone();
    keep.connect_clicked(move |_| u.on_keep());
    let u = ui.clone();
    revert.connect_clicked(move |_| u.on_revert());

    // Closing the window while the question is open goes back.
    let u = ui.clone();
    window.connect_close_request(move |_| {
        u.on_revert();
        glib::Propagation::Proceed
    });
}

impl Ui {
    /// The monitor that is selected, if any.
    fn selected_name(&self) -> Option<String> {
        self.selected.borrow().clone()
    }

    /// Show an error in the status line and log it.
    fn show_error(&self, e: &ScreenError) {
        opusdm_ipc::err_log!(APP_TAG, "{e}");
        self.status.set_text(&e.to_string());
    }

    /// The rectangles of the monitors that are on, with their names.
    fn rects(&self) -> Vec<(String, Rect)> {
        let session = self.session.borrow();
        session.edited().enabled().map(|c| (c.name.clone(), rect_of(c))).collect()
    }

    /// Fill the monitor list and the Only buttons from the session, and pick
    /// a monitor.
    fn rebuild_monitors(self: &Rc<Self>) {
        let names: Vec<String> = self.session.borrow().monitors().iter().map(|m| m.name.clone()).collect();
        self.updating.set(true);
        set_items(&self.pick, &names, 0);
        self.updating.set(false);
        let keep = self.selected_name().filter(|n| names.contains(n));
        *self.selected.borrow_mut() = keep.or_else(|| names.first().cloned());
        while let Some(child) = self.only_box.first_child() {
            self.only_box.remove(&child);
        }
        for name in &names {
            let button = gtk::Button::with_label(&format!("Only {name}"));
            let (u, n) = (self.clone(), name.clone());
            button.connect_clicked(move |_| u.on_mode(|m, l| modes::only(m, l, &n)));
            self.only_box.append(&button);
        }
        self.refresh();
    }

    /// Fill every widget from the session and redraw.
    fn refresh(&self) {
        self.updating.set(true);
        self.fill_panel();
        self.updating.set(false);
        let session = self.session.borrow();
        let pending = session.state() == State::Pending;
        let dirty = session.is_dirty();
        self.apply.set_sensitive(dirty && !pending);
        self.reset.set_sensitive(dirty && !pending);
        self.detect.set_sensitive(!pending);
        let free = !pending;
        self.extend.set_sensitive(free);
        self.duplicate.set_sensitive(free);
        self.only_box.set_sensitive(free);
        self.area.queue_draw();
    }

    /// Fill the panel from the selected monitor.
    fn fill_panel(&self) {
        let session = self.session.borrow();
        let selected = self.selected_name();
        let monitor = selected.as_deref().and_then(|n| session.monitors().iter().find(|m| m.name == n));
        let config = selected.as_deref().and_then(|n| session.edited().get(n));
        let (Some(monitor), Some(config)) = (monitor, config) else {
            self.enabled.set_sensitive(false);
            return;
        };
        let editable = session.state() == State::Idle;
        let on = config.enabled && editable;
        if let Some(index) = session.monitors().iter().position(|m| m.name == monitor.name) {
            self.pick.set_selected(index as u32);
        }
        self.pick.set_sensitive(editable);
        self.enabled.set_active(config.enabled);
        self.enabled.set_sensitive(editable);

        let sizes: Vec<(u32, u32)> = monitor.modes.iter().map(|m| (m.width, m.height)).collect();
        let texts: Vec<String> = sizes.iter().map(|(w, h)| format!("{w}x{h}")).collect();
        set_items(&self.resolution, &texts, sizes.iter().position(|s| *s == config.mode).unwrap_or(0));
        *self.sizes.borrow_mut() = sizes;

        let mut rates: Vec<Option<f64>> = vec![None];
        rates.extend(monitor.mode(config.mode).map(|m| m.rates.iter().copied().map(Some).collect::<Vec<_>>()).unwrap_or_default());
        let texts: Vec<String> = rates.iter().map(|r| r.map_or(AUTO_RATE.to_string(), rate_text)).collect();
        let index = rates.iter().position(|r| match (r, config.rate) {
            (None, None) => true,
            (Some(a), Some(b)) => same_rate(*a, b),
            _ => false,
        });
        set_items(&self.rate, &texts, index.unwrap_or(0));
        *self.rates.borrow_mut() = rates;

        self.rotation.set_selected(Rotation::ALL.iter().position(|r| *r == config.rotation).unwrap_or(0) as u32);
        let mut scales = SCALES.to_vec();
        if !scales.iter().any(|s| same_rate(*s, config.scale)) {
            scales.push(config.scale);
            scales.sort_by(|a, b| a.total_cmp(b));
        }
        let texts: Vec<String> = scales.iter().map(|s| scale_text(*s)).collect();
        set_items(&self.scale, &texts, scales.iter().position(|s| same_rate(*s, config.scale)).unwrap_or(0));
        *self.scale_options.borrow_mut() = scales;

        self.primary.set_active(config.primary);
        for widget in [self.resolution.upcast_ref::<gtk::Widget>(), self.rate.upcast_ref(), self.rotation.upcast_ref(), self.scale.upcast_ref(), self.primary.upcast_ref()] {
            widget.set_sensitive(on);
        }
    }

    /// Change the layout being built: `change` edits a copy and may refuse
    /// with a message.
    fn change(&self, change: impl FnOnce(&mut Layout, &[Monitor], &str) -> Result<(), String>) {
        if self.updating.get() { return; }
        let Some(name) = self.selected_name() else { return };
        let (mut layout, monitors) = {
            let session = self.session.borrow();
            (session.edited().clone(), session.monitors().to_vec())
        };
        match change(&mut layout, &monitors, &name) {
            Ok(()) => {
                self.status.set_text("");
                self.session.borrow_mut().edit(layout);
            }
            Err(message) => self.status.set_text(&message),
        }
        self.refresh();
    }

    /// The monitor picked in the list.
    fn on_pick(&self, index: u32) {
        if self.updating.get() { return; }
        let name = self.session.borrow().monitors().get(index as usize).map(|m| m.name.clone());
        *self.selected.borrow_mut() = name;
        self.refresh();
    }

    /// The Enabled box. A monitor that is turned on goes to the right of the others.
    fn on_enabled(&self, on: bool) {
        self.change(|layout, monitors, name| {
            if !on && layout.enabled().all(|c| c.name == name) {
                return Err("At least one monitor must stay on.".to_string());
            }
            let right = layout.enabled().filter(|c| c.name != name).map(|c| c.pos.0 + c.desktop_size().0 as i32).max().unwrap_or(0);
            let monitor = monitors.iter().find(|m| m.name == name).ok_or_else(|| format!("Unknown monitor {name}"))?;
            let config = layout.get_mut(name).ok_or_else(|| format!("Unknown monitor {name}"))?;
            if on && !config.enabled {
                if monitor.mode(config.mode).is_none() {
                    config.mode = monitor.default_size().ok_or_else(|| format!("{name} lists no resolution"))?;
                }
                config.pos = (right, 0);
            }
            config.enabled = on;
            if !on { config.primary = false; }
            Ok(())
        });
    }

    /// A resolution picked in the list; a rate the new one lacks goes back to Automatic.
    fn on_resolution(&self, index: u32) {
        let Some(size) = self.sizes.borrow().get(index as usize).copied() else { return };
        self.change(|layout, monitors, name| {
            let monitor = monitors.iter().find(|m| m.name == name).ok_or_else(|| format!("Unknown monitor {name}"))?;
            let config = layout.get_mut(name).ok_or_else(|| format!("Unknown monitor {name}"))?;
            config.mode = size;
            let rates = monitor.mode(size).map(|m| m.rates.clone()).unwrap_or_default();
            if config.rate.is_some_and(|r| !rates.iter().any(|x| same_rate(*x, r))) { config.rate = None; }
            Ok(())
        });
    }

    /// A refresh rate picked in the list.
    fn on_rate(&self, index: u32) {
        let Some(rate) = self.rates.borrow().get(index as usize).copied() else { return };
        self.change(|layout, _, name| {
            layout.get_mut(name).ok_or_else(|| format!("Unknown monitor {name}"))?.rate = rate;
            Ok(())
        });
    }

    /// A rotation picked in the list.
    fn on_rotation(&self, index: u32) {
        let Some(rotation) = Rotation::ALL.get(index as usize).copied() else { return };
        self.change(|layout, _, name| {
            layout.get_mut(name).ok_or_else(|| format!("Unknown monitor {name}"))?.rotation = rotation;
            Ok(())
        });
    }

    /// A scale picked in the list.
    fn on_scale(&self, index: u32) {
        let Some(scale) = self.scale_options.borrow().get(index as usize).copied() else { return };
        self.change(|layout, _, name| {
            layout.get_mut(name).ok_or_else(|| format!("Unknown monitor {name}"))?.scale = scale;
            Ok(())
        });
    }

    /// The Primary box: one monitor at most.
    fn on_primary(&self, on: bool) {
        self.change(|layout, _, name| {
            for config in &mut layout.monitors {
                config.primary = on && config.name == name;
            }
            Ok(())
        });
    }

    /// One of the Extend / Duplicate / Only buttons.
    fn on_mode(&self, make: impl FnOnce(&[Monitor], &Layout) -> Result<Layout, ScreenError>) {
        let result = {
            let session = self.session.borrow();
            make(session.monitors(), session.edited())
        };
        match result {
            Ok(layout) => {
                self.status.set_text("");
                self.session.borrow_mut().edit(layout);
            }
            Err(e) => self.status.set_text(&e.to_string()),
        }
        self.refresh();
    }

    /// Read the monitors again.
    fn on_detect(self: &Rc<Self>) {
        let result = self.session.borrow_mut().refresh();
        match result {
            Ok(()) => self.status.set_text(""),
            Err(e) => self.show_error(&e),
        }
        self.rebuild_monitors();
    }

    /// Apply and ask whether to keep the result.
    fn on_apply(self: &Rc<Self>) {
        let result = self.session.borrow_mut().apply();
        if let Err(e) = result {
            self.show_error(&e);
            self.refresh();
            return;
        }
        self.status.set_text("");
        self.edit_bar_visible(false);
        self.remaining.set(CONFIRM_SECS);
        self.show_countdown();
        let u = self.clone();
        let id = glib::timeout_add_local(Duration::from_secs(1), move || u.tick());
        *self.timer.borrow_mut() = Some(id);
        self.refresh();
    }

    /// Show the Apply bar (true) or the question bar (false).
    fn edit_bar_visible(&self, edit: bool) {
        self.edit_bar.set_visible(edit);
        self.confirm_bar.set_visible(!edit);
    }

    /// Put the seconds left in the question.
    fn show_countdown(&self) {
        self.confirm_label.set_text(&format!("Keep these display settings? Going back in {} s", self.remaining.get()));
    }

    /// One second of the countdown; at zero the old layout returns.
    fn tick(&self) -> glib::ControlFlow {
        let left = self.remaining.get().saturating_sub(1);
        self.remaining.set(left);
        if left > 0 {
            self.show_countdown();
            return glib::ControlFlow::Continue;
        }
        self.timer.borrow_mut().take();
        self.on_revert();
        glib::ControlFlow::Break
    }

    /// Stop the countdown.
    fn stop_timer(&self) {
        let old = self.timer.borrow_mut().take();
        if let Some(id) = old { id.remove(); }
    }

    /// Keep the applied layout.
    fn on_keep(&self) {
        self.stop_timer();
        let path = store::profiles_path();
        let result = self.session.borrow_mut().confirm(self.remember.is_active().then_some(path.as_path()));
        if let Err(e) = result { self.show_error(&e); }
        self.edit_bar_visible(true);
        self.refresh();
    }

    /// Put the previous layout back (also when the window closes).
    fn on_revert(&self) {
        self.stop_timer();
        let result = self.session.borrow_mut().revert();
        if let Err(e) = result { self.show_error(&e); }
        self.edit_bar_visible(true);
        self.refresh();
    }

    /// Press on the canvas: select the monitor under the pointer and start dragging it.
    fn drag_begin(&self, x: f64, y: f64) {
        if self.session.borrow().state() == State::Pending { return; }
        let view = self.view.get();
        let hit = self.rects().into_iter().rev().find(|(_, r)| view.hits(r, x, y));
        let Some((name, rect)) = hit else { return };
        *self.selected.borrow_mut() = Some(name.clone());
        *self.drag.borrow_mut() = Some(Drag { name, start_pos: (rect.x, rect.y) });
        self.refresh();
    }

    /// The pointer moved by `dx`, `dy` canvas pixels since the press.
    fn drag_update(&self, dx: f64, dy: f64) {
        let drag = self.drag.borrow();
        let Some(drag) = drag.as_ref() else { return };
        let view = self.view.get();
        let mut layout = self.session.borrow().edited().clone();
        let others: Vec<Rect> = self.rects().into_iter().filter(|(n, _)| *n != drag.name).map(|(_, r)| r).collect();
        let Some(config) = layout.get_mut(&drag.name) else { return };
        let pos = (drag.start_pos.0 + (dx / view.scale).round() as i32, drag.start_pos.1 + (dy / view.scale).round() as i32);
        config.pos = snap_position(pos, config.desktop_size(), &others, (SNAP_SCREEN_PX / view.scale).round() as i32);
        self.session.borrow_mut().edit(layout);
        self.refresh();
    }

    /// Release: the monitor stays where it is.
    fn drag_end(&self) {
        self.drag.borrow_mut().take();
        self.refresh();
    }

    /// Draw the monitors.
    fn draw(&self, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let rects = self.rects();
        if self.drag.borrow().is_none() {
            let all: Vec<Rect> = rects.iter().map(|(_, r)| *r).collect();
            self.view.set(fit_view(&all, width as f64, height as f64));
        }
        let view = self.view.get();
        let fg = self.area.color();
        let selected = self.selected_name();
        let session = self.session.borrow();
        for (name, rect) in &rects {
            let (x, y, w, h) = view.rect_to_canvas(rect);
            let is_selected = selected.as_deref() == Some(name.as_str());
            cr.rectangle(x, y, w, h);
            if is_selected { cr.set_source_rgba(0.2, 0.5, 0.9, 0.35) } else { cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.12) }
            let _ = cr.fill_preserve();
            if is_selected { cr.set_source_rgba(0.2, 0.5, 0.9, 1.0) } else { cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.6) }
            cr.set_line_width(if is_selected { 2.5 } else { 1.5 });
            let _ = cr.stroke();
            let Some(config) = session.edited().get(name) else { continue };
            if config.primary {
                cr.rectangle(x, y, w, (h * 0.06).clamp(3.0, 8.0));
                let _ = cr.fill();
            }
            // Duplicated monitors share a rectangle and one label.
            let mut same = rects.iter().filter(|(_, r)| r == rect).map(|(n, _)| n.as_str());
            if same.next() != Some(name.as_str()) { continue; }
            let label = rects.iter().filter(|(_, r)| r == rect).map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(" + ");
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, fg.alpha() as f64);
            cr.select_font_face("Sans", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Bold);
            centered_text(cr, &label, x + w / 2.0, y + h / 2.0 - 4.0, 14.0);
            cr.select_font_face("Sans", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            let detail = format!("{}x{}{}", config.mode.0, config.mode.1, config.rate.map_or(String::new(), |r| format!(" @ {r:.0} Hz")));
            centered_text(cr, &detail, x + w / 2.0, y + h / 2.0 + 14.0, 11.0);
        }
    }
}

/// The desktop rectangle of a monitor setting.
fn rect_of(config: &MonitorConfig) -> Rect {
    let (w, h) = config.desktop_size();
    Rect { x: config.pos.0, y: config.pos.1, w, h }
}

/// Draw `text` centered on `cx`, with its baseline at `y`.
fn centered_text(cr: &gtk::cairo::Context, text: &str, cx: f64, y: f64, size: f64) {
    cr.set_font_size(size);
    let Ok(extents) = cr.text_extents(text) else { return };
    cr.move_to(cx - extents.width() / 2.0 - extents.x_bearing(), y);
    let _ = cr.show_text(text);
}
