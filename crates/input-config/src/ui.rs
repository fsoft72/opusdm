//! The Input window: Mouse, Touchpad and Keyboard tabs, live apply, Save and Cancel.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::mpsc;
use std::time::Duration;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_input_config::backend::{for_session, GroupStatus, Status};
use opusdm_input_config::parse::{load_xkb_lists, XkbLists};
use opusdm_input_config::worker::{self, Reply, Request};
use opusdm_input_config::{
    store, InputError, InputSettings, LayoutEntry, MAX_LAYOUTS, REPEAT_DELAY_MAX_MS, REPEAT_DELAY_MIN_MS, REPEAT_RATE_MAX, REPEAT_RATE_MIN,
    SPEED_MAX, SPEED_MIN,
};

/// Wait this long after the last slider move before applying: every apply
/// starts the X tools, so a drag must not start one per pixel.
const DEBOUNCE_MS: u64 = 150;
/// Default window size.
const WINDOW_W: i32 = 520;
const WINDOW_H: i32 = 460;
/// Gap between the widgets of a page.
const PAGE_SPACING: i32 = 10;
/// Padding of a page.
const PAGE_MARGIN: i32 = 12;
/// Width of the label column.
const LABEL_CHARS: i32 = 16;
/// Shown until the worker has read the devices.
const LOADING_TEXT: &str = "Reading the input devices...";

/// An edit of the settings, applied to a copy of the current ones.
type Edit = Box<dyn FnOnce(&mut InputSettings)>;

/// What every callback of the window shares.
struct State {
    /// Feeds the worker thread that owns the live session.
    requests: mpsc::Sender<Request>,
    /// The settings the controls show; every change is sent to the worker.
    current: RefCell<InputSettings>,
    window: gtk::Window,
    save: gtk::Button,
    status: gtk::Label,
    pending: RefCell<Option<Edit>>,
    timer: RefCell<Option<glib::SourceId>>,
    /// The window may close now: saved, reverted, or the worker is gone.
    finished: Cell<bool>,
}

impl State {
    /// Show the errors of the last apply in the status line (clear it when none).
    fn show_errors(&self, errors: &[InputError]) {
        self.status.set_text(&errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n"));
    }

    /// Hand `request` to the worker. Returns whether it was sent.
    fn send(&self, request: Request) -> bool {
        let sent = self.requests.send(request).is_ok();
        if !sent { self.status.set_text("The input settings worker stopped: close the window and open it again."); }
        sent
    }

    /// Apply `edit` to a copy of the current settings, live. The worker runs
    /// the commands; its errors come back as a reply.
    fn update(&self, edit: impl FnOnce(&mut InputSettings)) {
        let next = {
            let mut current = self.current.borrow_mut();
            edit(&mut current);
            current.clone()
        };
        self.send(Request::Change(next));
    }

    /// Like `update`, after a short pause: a slider drag sends many values.
    fn update_debounced(self: &Rc<Self>, edit: Edit) {
        *self.pending.borrow_mut() = Some(edit);
        self.cancel_timer();
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(DEBOUNCE_MS), move || {
            let Some(this) = weak.upgrade() else { return };
            this.timer.borrow_mut().take();
            this.flush();
        });
        *self.timer.borrow_mut() = Some(id);
    }

    /// Stop the debounce timer, if it runs.
    fn cancel_timer(&self) {
        let old = self.timer.borrow_mut().take();
        if let Some(id) = old { id.remove(); }
    }

    /// Apply the waiting edit now (Save calls this so the last move is not lost).
    fn flush(&self) {
        self.cancel_timer();
        let edit = self.pending.borrow_mut().take();
        if let Some(edit) = edit { self.update(edit); }
    }

    /// Save: apply the waiting edit, then write the file. The window closes
    /// when the worker reports the file written.
    fn save(&self) {
        self.flush();
        if self.send(Request::Commit(store::input_path())) { self.save.set_sensitive(false); }
    }

    /// The window was closed without Save: drop the waiting edit and ask the
    /// worker to put the snapshot back. Returns whether the window may close
    /// now; otherwise it closes when the worker reports the revert.
    fn discard_and_revert(&self) -> bool {
        self.cancel_timer();
        self.pending.borrow_mut().take();
        if !self.send(Request::Revert) { return true; }
        self.window.set_sensitive(false);
        false
    }

    /// One reply of the worker; `slot` holds the tabs.
    fn on_reply(self: &Rc<Self>, reply: Reply, slot: &gtk::Box) {
        match reply {
            Reply::Opened { status, current } => {
                *self.current.borrow_mut() = current;
                self.fill(slot, &status);
                self.save.set_sensitive(true);
            }
            Reply::Applied(errors) => self.show_errors(&errors),
            Reply::Committed(Ok(())) => self.close_now(),
            Reply::Committed(Err(e)) => {
                self.show_errors(&[e]);
                self.save.set_sensitive(true);
            }
            Reply::Reverted(errors) => {
                for e in errors {
                    opusdm_ipc::err_log!("opusdm-input-config", "revert: {e}");
                }
                self.close_now();
            }
        }
    }

    /// Close the window without asking the worker anything more.
    fn close_now(&self) {
        self.finished.set(true);
        self.window.close();
    }

    /// Put the three tabs in `slot`, in place of the loading text.
    fn fill(self: &Rc<Self>, slot: &gtk::Box, status: &Status) {
        while let Some(child) = slot.first_child() { slot.remove(&child); }
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        notebook.append_page(&mouse_page(self, &status.mouse), Some(&gtk::Label::new(Some("Mouse"))));
        notebook.append_page(&touchpad_page(self, &status.touchpad), Some(&gtk::Label::new(Some("Touchpad"))));
        notebook.append_page(&keyboard_page(self, &status.keyboard, Rc::new(load_xkb_lists())), Some(&gtk::Label::new(Some("Keyboard"))));
        slot.append(&notebook);
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
    let m = state.current.borrow().mouse.clone();
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
    let t = state.current.borrow().touchpad.clone();
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
    let layouts = state.current.borrow().keyboard.layouts.clone();
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
    let k = state.current.borrow().keyboard.clone();

    let rows = gtk::Box::new(gtk::Orientation::Vertical, PAGE_SPACING);
    build_layout_rows(state, &lists, &rows);
    page.append(&rows);

    let add = gtk::Button::with_label("Add layout");
    add.set_halign(gtk::Align::Start);
    {
        let (state, lists, rows) = (state.clone(), lists.clone(), rows.clone());
        add.connect_clicked(move |_| {
            if state.current.borrow().keyboard.layouts.len() >= MAX_LAYOUTS { return; }
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

/// Open the Input window. The backend of the session is built and run on a
/// worker thread; the tabs appear once it has read the devices.
pub fn open(app: &gtk::Application) {
    let (reply_tx, reply_rx) = async_channel::unbounded();
    let requests = worker::spawn(for_session, reply_tx);
    let status_label = gtk::Label::new(None);
    status_label.set_wrap(true);
    status_label.set_xalign(0.0);
    let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    slot.set_vexpand(true);
    slot.append(&gtk::Label::new(Some(LOADING_TEXT)));

    let shell = opusdm_ui::DialogShell::standalone(app, "Input").modal(false).size(WINDOW_W, WINDOW_H);
    shell.content().append(&slot);
    shell.content().append(&status_label);
    shell.close_button("Cancel");
    let save = shell.button("Save");
    save.set_sensitive(false);
    let window = shell.window().clone();
    let state = Rc::new(State {
        requests,
        current: RefCell::new(InputSettings::default()),
        window: window.clone(),
        save: save.clone(),
        status: status_label,
        pending: RefCell::new(None),
        timer: RefCell::new(None),
        finished: Cell::new(false),
    });
    listen(&state, reply_rx, slot);

    // Any way of closing the window except Save puts the old values back.
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            if state.finished.get() || state.discard_and_revert() { return glib::Propagation::Proceed; }
            glib::Propagation::Stop
        });
    }
    {
        let state = state.clone();
        save.connect_clicked(move |_| state.save());
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

/// Hand the replies of the worker to `state` until the window is gone.
fn listen(state: &Rc<State>, replies: async_channel::Receiver<Reply>, slot: gtk::Box) {
    let weak: Weak<State> = Rc::downgrade(state);
    glib::spawn_future_local(async move {
        while let Ok(reply) = replies.recv().await {
            let Some(state) = weak.upgrade() else { return };
            state.on_reply(reply, &slot);
        }
        // The worker ended: nothing can be reverted any more.
        if let Some(state) = weak.upgrade() { state.finished.set(true); }
    });
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
