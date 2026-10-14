//! The window of one calendar day: its appointments on top, a form below to
//! add one or to change / delete the one clicked in the list.

use crate::agenda_center::AgendaCenter;
use crate::agenda_model::{self, Appointment, ALARM_CHOICES};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Width of the window content.
const CONTENT_WIDTH: i32 = 340;
/// Height of the list before it scrolls.
const LIST_MAX_HEIGHT: i32 = 200;

/// The widgets of the window.
struct DayWindow {
    window: gtk::Window,
    center: Rc<AgendaCenter>,
    year: i32,
    month: u32,
    day: u32,
    list: gtk::Box,
    text: gtk::Entry,
    time: gtk::Entry,
    alarm: gtk::DropDown,
    message: gtk::Label,
    delete: gtk::Button,
    save: gtk::Button,
    /// Id of the appointment in the form; 0 for a new one.
    editing: Cell<u64>,
}

/// Open the window of `date` (only its day counts) over `parent`. The first
/// appointment of the day is loaded in the form, else the form is empty.
pub fn open(center: &Rc<AgendaCenter>, parent: Option<&gtk::Window>, date: &glib::DateTime) {
    let (year, month, day) = (date.year(), date.month() as u32, date.day_of_month() as u32);
    let title = date.format("Appointments - %A %-d %B %Y").map(|s| s.to_string()).unwrap_or_else(|_| "Appointments".to_string());
    let window = gtk::Window::builder().title(title.as_str()).resizable(false).build();
    window.set_transient_for(parent);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    for set in [gtk::Widget::set_margin_top, gtk::Widget::set_margin_bottom, gtk::Widget::set_margin_start, gtk::Widget::set_margin_end] {
        set(content.upcast_ref(), 12);
    }
    content.set_size_request(CONTENT_WIDTH, -1);
    let heading = gtk::Label::new(Some(&title));
    heading.add_css_class("heading");
    heading.set_xalign(0.0);
    content.append(&heading);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(LIST_MAX_HEIGHT)
        .child(&list)
        .build();
    content.append(&scroller);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let (form, text, time, alarm) = build_form(date);
    content.append(&form);
    let message = gtk::Label::new(None);
    message.add_css_class("dim-label");
    message.set_xalign(0.0);
    message.set_wrap(true);
    content.append(&message);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let new = gtk::Button::with_label("New");
    let delete = gtk::Button::with_label("Delete");
    delete.add_css_class("destructive-action");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let close = gtk::Button::with_label("Close");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    for b in [new.upcast_ref::<gtk::Widget>(), delete.upcast_ref(), spacer.upcast_ref(), close.upcast_ref(), save.upcast_ref()] {
        buttons.append(b);
    }
    content.append(&buttons);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&save));
    text.set_activates_default(true);
    time.set_activates_default(true);

    let me = Rc::new(DayWindow {
        window: window.clone(),
        center: center.clone(),
        year,
        month,
        day,
        list,
        text,
        time,
        alarm,
        message,
        delete: delete.clone(),
        save: save.clone(),
        editing: Cell::new(0),
    });
    me.wire(&new, &close);
    me.refresh_list();
    me.load(me.center.on_day(year, month, day).first());
    center.set_day_window(&window);
    window.present();
    me.text.grab_focus();
}

/// The form grid: Text, Date (read only), Time and Alarm.
fn build_form(date: &glib::DateTime) -> (gtk::Grid, gtk::Entry, gtk::Entry, gtk::DropDown) {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(10);
    let text = gtk::Entry::new();
    text.set_hexpand(true);
    text.set_placeholder_text(Some("What"));
    let date_label = gtk::Label::new(date.format("%x").ok().as_deref());
    date_label.set_xalign(0.0);
    let time = gtk::Entry::new();
    time.set_placeholder_text(Some("HH:MM"));
    time.set_max_length(5);
    time.set_width_chars(6);
    time.set_halign(gtk::Align::Start);
    let labels: Vec<String> = ALARM_CHOICES.iter().map(|m| agenda_model::alarm_label(*m)).collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let alarm = gtk::DropDown::from_strings(&labels);
    alarm.set_halign(gtk::Align::Start);
    let rows: [(&str, gtk::Widget); 4] = [
        ("Text", text.clone().upcast()),
        ("Date", date_label.upcast()),
        ("Time", time.clone().upcast()),
        ("Alarm", alarm.clone().upcast()),
    ];
    for (row, (name, widget)) in rows.into_iter().enumerate() {
        let label = gtk::Label::new(Some(name));
        label.set_xalign(1.0);
        label.add_css_class("dim-label");
        grid.attach(&label, 0, row as i32, 1, 1);
        grid.attach(&widget, 1, row as i32, 1, 1);
    }
    (grid, text, time, alarm)
}

impl DayWindow {
    /// Connect the buttons, the entries and Escape.
    fn wire(self: &Rc<Self>, new: &gtk::Button, close: &gtk::Button) {
        {
            let me = self.clone();
            new.connect_clicked(move |_| {
                me.load(None);
                me.text.grab_focus();
            });
        }
        {
            let window = self.window.clone();
            close.connect_clicked(move |_| window.close());
        }
        {
            let me = self.clone();
            self.save.connect_clicked(move |_| me.on_save());
        }
        {
            let me = self.clone();
            self.delete.connect_clicked(move |_| me.on_delete());
        }
        for entry in [&self.text, &self.time] {
            let me = self.clone();
            entry.connect_changed(move |_| me.validate());
        }
        let keys = gtk::EventControllerKey::new();
        {
            let window = self.window.clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key != gtk::gdk::Key::Escape { return glib::Propagation::Proceed; }
                window.close();
                glib::Propagation::Stop
            });
        }
        self.window.add_controller(keys);
    }

    /// Fill the form with `appointment`, or empty it for a new one.
    fn load(self: &Rc<Self>, appointment: Option<&Appointment>) {
        let a = appointment.cloned().unwrap_or_default();
        self.editing.set(a.id);
        self.text.set_text(&a.text);
        self.time.set_text(&if a.id == 0 { String::new() } else { a.time_text() });
        let choice = ALARM_CHOICES.iter().position(|m| *m == a.alarm_minutes).unwrap_or(0);
        self.alarm.set_selected(choice as u32);
        self.delete.set_sensitive(a.id != 0);
        self.save.set_label(if a.id == 0 { "Add" } else { "Save" });
        self.validate();
        self.refresh_list();
    }

    /// Enable Save only for a text and a valid time; say what is missing.
    fn validate(&self) {
        let has_text = !self.text.text().trim().is_empty();
        let time = self.time.text();
        let time_ok = agenda_model::parse_time(&time).is_some();
        self.save.set_sensitive(has_text && time_ok);
        let hint = if !time.is_empty() && !time_ok { "Time must be HH:MM, from 00:00 to 23:59" } else { "" };
        self.message.set_text(hint);
    }

    /// Rebuild the list of the day; the appointment in the form is marked.
    fn refresh_list(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let items = self.center.on_day(self.year, self.month, self.day);
        if items.is_empty() {
            let empty = gtk::Label::new(Some("No appointments"));
            empty.add_css_class("dim-label");
            empty.set_margin_top(6);
            empty.set_margin_bottom(6);
            self.list.append(&empty);
            return;
        }
        for a in items {
            self.list.append(&self.build_row(&a));
        }
    }

    /// One list row: time, text and the alarm; a click loads it in the form.
    fn build_row(self: &Rc<Self>, a: &Appointment) -> gtk::Button {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let time = gtk::Label::new(Some(&a.time_text()));
        time.add_css_class("heading");
        line.append(&time);
        let text = gtk::Label::new(Some(&a.text));
        text.set_xalign(0.0);
        text.set_hexpand(true);
        text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        line.append(&text);
        if a.alarm_minutes > 0 {
            let alarm = gtk::Image::from_icon_name("alarm-symbolic");
            alarm.set_tooltip_text(Some(&format!("Alarm {} before", agenda_model::alarm_label(a.alarm_minutes))));
            line.append(&alarm);
        }
        let row = gtk::Button::new();
        row.add_css_class("flat");
        row.set_child(Some(&line));
        if a.id == self.editing.get() { row.add_css_class("opusdm-agenda-current"); }
        let (me, a) = (self.clone(), a.clone());
        row.connect_clicked(move |_| me.load(Some(&a)));
        row
    }

    /// Store the form and show it in the list.
    fn on_save(self: &Rc<Self>) {
        let Some((hour, minute)) = agenda_model::parse_time(&self.time.text()) else { return };
        let alarm_minutes = ALARM_CHOICES.get(self.alarm.selected() as usize).copied().unwrap_or(0);
        let appointment = Appointment {
            id: self.editing.get(),
            text: self.text.text().trim().to_string(),
            year: self.year,
            month: self.month,
            day: self.day,
            hour,
            minute,
            alarm_minutes,
            notified: false,
        };
        match self.center.save(appointment) {
            Ok(id) => {
                let saved = self.center.on_day(self.year, self.month, self.day).into_iter().find(|a| a.id == id);
                self.load(saved.as_ref());
                self.message.set_text("Saved");
            }
            Err(e) => self.message.set_text(&e),
        }
    }

    /// Delete the appointment in the form and empty the form.
    fn on_delete(self: &Rc<Self>) {
        let id = self.editing.get();
        if id == 0 { return; }
        match self.center.remove(id) {
            Ok(()) => {
                self.load(None);
                self.message.set_text("Deleted");
            }
            Err(e) => self.message.set_text(&e),
        }
    }
}
