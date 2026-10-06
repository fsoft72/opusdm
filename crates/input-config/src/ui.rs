//! The Input window: Mouse, Touchpad and Keyboard tabs, live apply, Save and Cancel.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_input_config::backend::{GroupStatus, XBackend};
use opusdm_input_config::parse::{load_xkb_lists, XkbLists};
use opusdm_input_config::session::Session;
use opusdm_input_config::{
    store, InputError, InputSettings, LayoutEntry, MAX_LAYOUTS, REPEAT_DELAY_MAX_MS, REPEAT_DELAY_MIN_MS, REPEAT_RATE_MAX, REPEAT_RATE_MIN,
    SPEED_MAX, SPEED_MIN,
};

/// Wait this long after the last slider move before applying.
const DEBOUNCE_MS: u64 = 30;
/// Default window size.
const WINDOW_W: i32 = 520;
const WINDOW_H: i32 = 460;
/// Gap between the widgets of a page.
const PAGE_SPACING: i32 = 10;
/// Padding of a page.
const PAGE_MARGIN: i32 = 12;
/// Width of the label column.
const LABEL_CHARS: i32 = 16;

/// An edit of the settings, applied to a copy of the current ones.
type Edit = Box<dyn FnOnce(&mut InputSettings)>;

/// What every callback of the window shares.
struct State {
    session: RefCell<Session<XBackend>>,
    status: gtk::Label,
    pending: RefCell<Option<Edit>>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl State {
    /// Show the errors of the last apply in the status line (clear it when none).
    fn show_errors(&self, errors: &[InputError]) {
        self.status.set_text(&errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n"));
    }

    /// Apply `edit` to a copy of the current settings, live.
    fn update(&self, edit: impl FnOnce(&mut InputSettings)) {
        let mut next = self.session.borrow().current().clone();
        edit(&mut next);
        let errors = self.session.borrow_mut().change(next);
        self.show_errors(&errors);
    }

    /// Like `update`, after a short pause: a slider drag sends many values.
    fn update_debounced(self: &Rc<Self>, edit: Edit) {
        *self.pending.borrow_mut() = Some(edit);
        let old = self.timer.borrow_mut().take();
        if let Some(id) = old { id.remove(); }
        let this = self.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(DEBOUNCE_MS), move || {
            this.timer.borrow_mut().take();
            this.flush();
        });
        *self.timer.borrow_mut() = Some(id);
    }

    /// Apply the waiting edit now (Save calls this so the last move is not lost).
    fn flush(&self) {
        let old = self.timer.borrow_mut().take();
        if let Some(id) = old { id.remove(); }
        let edit = self.pending.borrow_mut().take();
        if let Some(edit) = edit { self.update(edit); }
    }

    /// Drop the waiting edit and put the snapshot back (window closed without Save).
    fn discard_and_revert(&self) {
        let old = self.timer.borrow_mut().take();
        if let Some(id) = old { id.remove(); }
        self.pending.borrow_mut().take();
        let errors = self.session.borrow_mut().revert();
        for e in errors {
            opusdm_ipc::err_log!("opusdm-input-config", "revert: {e}");
        }
    }
}

/// A label, a slider and the value, in one row. `on_change` gets the new value.
fn speed_row(label: &str, initial: f64, on_change: impl Fn(f64) + 'static) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);
    let name = gtk::Label::new(Some(label));
    name.set_width_chars(LABEL_CHARS);
    name.set_xalign(0.0);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, SPEED_MIN, SPEED_MAX, 0.05);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_digits(2);
    scale.set_value(initial);
    scale.connect_value_changed(move |s| on_change(s.value()));
    row.append(&name);
    row.append(&scale);
    row
}

/// A check button that reports its state.
fn check_row(label: &str, initial: bool, on_toggle: impl Fn(bool) + 'static) -> gtk::CheckButton {
    let check = gtk::CheckButton::with_label(label);
    check.set_active(initial);
    check.connect_toggled(move |c| on_toggle(c.is_active()));
    check
}

/// A label and a spin button for a whole number.
fn spin_row(label: &str, initial: u32, min: u32, max: u32, step: u32, on_change: impl Fn(u32) + 'static) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);
    let name = gtk::Label::new(Some(label));
    name.set_width_chars(LABEL_CHARS);
    name.set_xalign(0.0);
    let spin = gtk::SpinButton::with_range(min as f64, max as f64, step as f64);
    spin.set_value(initial as f64);
    spin.connect_value_changed(move |s| on_change(s.value() as u32));
    row.append(&name);
    row.append(&spin);
    row
}

/// A vertical page. A group that cannot be used shows its reason first.
fn page(status: &GroupStatus) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, PAGE_SPACING);
    page.set_margin_top(PAGE_MARGIN);
    page.set_margin_bottom(PAGE_MARGIN);
    page.set_margin_start(PAGE_MARGIN);
    page.set_margin_end(PAGE_MARGIN);
    if !status.available {
        let reason = gtk::Label::new(Some(&status.reason));
        reason.set_wrap(true);
        reason.set_xalign(0.0);
        page.append(&reason);
    }
    page
}

/// Grey out the controls of `page` (everything after the reason) when its group cannot be used.
fn finish_page(page: &gtk::Box, status: &GroupStatus) {
    if status.available { return; }
    let mut child = page.first_child().and_then(|c| c.next_sibling());
    while let Some(widget) = child {
        widget.set_sensitive(false);
        child = widget.next_sibling();
    }
}

/// The Mouse tab.
fn mouse_page(state: &Rc<State>, status: &GroupStatus) -> gtk::Box {
    let page = page(status);
    let m = state.session.borrow().current().mouse.clone();
    let s = state.clone();
    page.append(&speed_row("Pointer speed", m.speed, move |v| s.update_debounced(Box::new(move |all| all.mouse.speed = v))));
    let s = state.clone();
    page.append(&check_row("Flat acceleration (no acceleration curve)", m.flat_profile, move |v| s.update(|all| all.mouse.flat_profile = v)));
    let s = state.clone();
    page.append(&check_row("Left handed (swap the buttons)", m.left_handed, move |v| s.update(|all| all.mouse.left_handed = v)));
    let s = state.clone();
    page.append(&check_row("Natural scrolling", m.natural_scroll, move |v| s.update(|all| all.mouse.natural_scroll = v)));
    finish_page(&page, status);
    page
}

/// The Touchpad tab.
fn touchpad_page(state: &Rc<State>, status: &GroupStatus) -> gtk::Box {
    let page = page(status);
    let t = state.session.borrow().current().touchpad.clone();
    let s = state.clone();
    page.append(&speed_row("Pointer speed", t.speed, move |v| s.update_debounced(Box::new(move |all| all.touchpad.speed = v))));
    let s = state.clone();
    page.append(&check_row("Tap to click", t.tap_to_click, move |v| s.update(|all| all.touchpad.tap_to_click = v)));
    let s = state.clone();
    page.append(&check_row("Natural scrolling", t.natural_scroll, move |v| s.update(|all| all.touchpad.natural_scroll = v)));
    let s = state.clone();
    page.append(&check_row("Disable while typing", t.disable_while_typing, move |v| s.update(|all| all.touchpad.disable_while_typing = v)));
    finish_page(&page, status);
    page
}

/// A searchable drop-down over `labels`.
fn dropdown(labels: &[String], selected: usize) -> gtk::DropDown {
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let dd = gtk::DropDown::from_strings(&refs);
    dd.set_enable_search(true);
    dd.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    dd.set_selected(selected as u32);
    dd
}

/// Fill `rows` with one row per layout: layout, variant and a remove button.
/// Rebuilt after every add, remove or layout change.
fn build_layout_rows(state: &Rc<State>, lists: &Rc<XkbLists>, rows: &gtk::Box) {
    while let Some(child) = rows.first_child() { rows.remove(&child); }
    let layouts = state.session.borrow().current().keyboard.layouts.clone();
    let rebuild = {
        let (state, lists, rows) = (state.clone(), lists.clone(), rows.clone());
        move || {
            let (state, lists, rows) = (state.clone(), lists.clone(), rows.clone());
            // Not inside the signal handler of a widget that is about to be removed.
            glib::idle_add_local_once(move || build_layout_rows(&state, &lists, &rows));
        }
    };
    for (i, entry) in layouts.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);

        let layout_labels: Vec<String> = lists.layouts.iter().map(|l| format!("{} - {}", l.code, l.description)).collect();
        let layout_pos = lists.layouts.iter().position(|l| l.code == entry.layout).unwrap_or(0);
        let layout_dd = dropdown(&layout_labels, layout_pos);
        layout_dd.set_hexpand(true);

        let variants: Vec<(String, String)> =
            lists.variants.iter().filter(|v| v.layout == entry.layout).map(|v| (v.code.clone(), v.description.clone())).collect();
        let mut variant_labels = vec!["(default)".to_string()];
        variant_labels.extend(variants.iter().map(|(c, d)| format!("{c} - {d}")));
        let variant_pos = variants.iter().position(|(c, _)| *c == entry.variant).map(|p| p + 1).unwrap_or(0);
        let variant_dd = dropdown(&variant_labels, variant_pos);
        variant_dd.set_hexpand(true);

        {
            let (state, lists, rebuild) = (state.clone(), lists.clone(), rebuild.clone());
            layout_dd.connect_selected_notify(move |dd| {
                let Some(item) = lists.layouts.get(dd.selected() as usize) else { return };
                let code = item.code.clone();
                state.update(|all| {
                    if let Some(l) = all.keyboard.layouts.get_mut(i) { *l = LayoutEntry { layout: code, variant: String::new() }; }
                });
                rebuild();
            });
        }
        {
            let state = state.clone();
            let variants = variants.clone();
            variant_dd.connect_selected_notify(move |dd| {
                let variant = match dd.selected() {
                    0 => String::new(),
                    n => variants.get(n as usize - 1).map(|(c, _)| c.clone()).unwrap_or_default(),
                };
                state.update(|all| {
                    if let Some(l) = all.keyboard.layouts.get_mut(i) { l.variant = variant; }
                });
            });
        }

        let remove = gtk::Button::with_label("Remove");
        remove.set_sensitive(layouts.len() > 1);
        {
            let (state, rebuild) = (state.clone(), rebuild.clone());
            remove.connect_clicked(move |_| {
                state.update(|all| {
                    if i < all.keyboard.layouts.len() { all.keyboard.layouts.remove(i); }
                });
                rebuild();
            });
        }

        row.append(&layout_dd);
        row.append(&variant_dd);
        row.append(&remove);
        rows.append(&row);
    }
}

/// The Keyboard tab.
fn keyboard_page(state: &Rc<State>, status: &GroupStatus, lists: Rc<XkbLists>) -> gtk::Box {
    let page = page(status);
    let k = state.session.borrow().current().keyboard.clone();

    let rows = gtk::Box::new(gtk::Orientation::Vertical, PAGE_SPACING);
    build_layout_rows(state, &lists, &rows);
    page.append(&rows);

    let add = gtk::Button::with_label("Add layout");
    add.set_halign(gtk::Align::Start);
    {
        let (state, lists, rows) = (state.clone(), lists.clone(), rows.clone());
        add.connect_clicked(move |_| {
            if state.session.borrow().current().keyboard.layouts.len() >= MAX_LAYOUTS { return; }
            state.update(|all| all.keyboard.layouts.push(LayoutEntry { layout: "us".to_string(), variant: String::new() }));
            build_layout_rows(&state, &lists, &rows);
        });
    }
    page.append(&add);

    // The switch key: "None" first, then the grp: options; the current one is
    // kept in the list even when evdev.lst does not know it.
    let mut codes = vec![String::new()];
    let mut labels = vec!["None".to_string()];
    for o in &lists.switch_options {
        codes.push(o.code.clone());
        labels.push(o.description.clone());
    }
    if !k.switch_option.is_empty() && !codes.contains(&k.switch_option) {
        codes.push(k.switch_option.clone());
        labels.push(k.switch_option.clone());
    }
    let selected = codes.iter().position(|c| *c == k.switch_option).unwrap_or(0);
    let switch_dd = dropdown(&labels, selected);
    switch_dd.set_hexpand(true);
    {
        let state = state.clone();
        switch_dd.connect_selected_notify(move |dd| {
            let code = codes.get(dd.selected() as usize).cloned().unwrap_or_default();
            state.update(|all| all.keyboard.switch_option = code);
        });
    }
    let switch_row = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_SPACING);
    let switch_label = gtk::Label::new(Some("Switch layout with"));
    switch_label.set_width_chars(LABEL_CHARS);
    switch_label.set_xalign(0.0);
    switch_row.append(&switch_label);
    switch_row.append(&switch_dd);
    page.append(&switch_row);

    let s = state.clone();
    page.append(&spin_row("Repeat delay (ms)", k.repeat_delay_ms, REPEAT_DELAY_MIN_MS, REPEAT_DELAY_MAX_MS, 50, move |v| {
        s.update(|all| all.keyboard.repeat_delay_ms = v)
    }));
    let s = state.clone();
    page.append(&spin_row("Repeat rate (per s)", k.repeat_rate, REPEAT_RATE_MIN, REPEAT_RATE_MAX, 1, move |v| {
        s.update(|all| all.keyboard.repeat_rate = v)
    }));

    finish_page(&page, status);
    page
}

/// Open the Input window on `backend`.
pub fn open(app: &gtk::Application, backend: XBackend) {
    let session = Session::open(backend);
    let status = session.status();
    let status_label = gtk::Label::new(None);
    status_label.set_wrap(true);
    status_label.set_xalign(0.0);
    let state = Rc::new(State {
        session: RefCell::new(session),
        status: status_label.clone(),
        pending: RefCell::new(None),
        timer: RefCell::new(None),
    });
    let lists = Rc::new(load_xkb_lists());

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    notebook.append_page(&mouse_page(&state, &status.mouse), Some(&gtk::Label::new(Some("Mouse"))));
    notebook.append_page(&touchpad_page(&state, &status.touchpad), Some(&gtk::Label::new(Some("Touchpad"))));
    notebook.append_page(&keyboard_page(&state, &status.keyboard, lists), Some(&gtk::Label::new(Some("Keyboard"))));

    let shell = opusdm_ui::DialogShell::standalone(app, "Input").modal(false).size(WINDOW_W, WINDOW_H);
    shell.content().append(&notebook);
    shell.content().append(&status_label);
    shell.close_button("Cancel");
    let save = shell.button("Save");
    let window = shell.window().clone();

    // Any way of closing the window except Save puts the old values back.
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            state.discard_and_revert();
            glib::Propagation::Proceed
        });
    }
    {
        let (state, window) = (state.clone(), window.clone());
        save.connect_clicked(move |_| {
            state.flush();
            let result = state.session.borrow_mut().commit(&store::input_path());
            match result {
                Ok(()) => window.close(),
                Err(e) => state.show_errors(&[e]),
            }
        });
    }
    {
        let window_for_key = window.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, k, _, _| {
            if k != gtk::gdk::Key::Escape { return glib::Propagation::Proceed; }
            window_for_key.close();
            glib::Propagation::Stop
        });
        window.add_controller(key);
    }
    shell.present();
}

/// A window that only explains why the settings cannot be edited.
pub fn open_problem(app: &gtk::Application, message: &str) {
    let shell = opusdm_ui::DialogShell::standalone(app, "Input").modal(false).size(WINDOW_W, 120);
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    shell.content().append(&label);
    shell.close_button("Close");
    shell.present();
}
