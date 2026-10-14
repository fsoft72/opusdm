//! Two-line clock: a large `HH:MM` over a small locale-formatted date.
//! A left click opens a month calendar (arrows change month and year); the
//! days with appointments are marked and a click on a day opens its window.

use crate::agenda_center::AgendaCenter;
use crate::agenda_window;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
use std::time::Duration;

/// Tick interval. Five seconds is plenty at minute display granularity and
/// keeps the label from drifting more than a few seconds behind the wall
/// clock after a suspend/resume.
const TICK: Duration = Duration::from_secs(5);
/// CSS class GTK gives the day labels of a `gtk::Calendar`.
const DAY_NUMBER_CLASS: &str = "day-number";

/// Build the clock widget. It refreshes itself for the lifetime of the
/// returned button.
pub fn build(agenda: &Rc<AgendaCenter>) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_valign(gtk::Align::Center);

    let time = gtk::Label::new(None);
    time.add_css_class("opusdm-panel-clock-time");
    time.set_xalign(1.0);

    let date = gtk::Label::new(None);
    date.add_css_class("opusdm-panel-clock-date");
    date.set_xalign(1.0);

    root.append(&time);
    root.append(&date);

    refresh(&time, &date);
    {
        let time = time.clone();
        let date = date.clone();
        glib::timeout_add_local(TICK, move || {
            if time.root().is_none() { return glib::ControlFlow::Break; }
            refresh(&time, &date);
            glib::ControlFlow::Continue
        });
    }

    let button = gtk::MenuButton::new();
    button.add_css_class("flat");
    button.set_valign(gtk::Align::Center);
    button.set_child(Some(&root));
    button.set_tooltip_text(Some("Calendar"));
    let popover = gtk::Popover::new();
    let calendar = gtk::Calendar::new();
    popover.set_child(Some(&calendar));
    button.set_popover(Some(&popover));
    wire_calendar(agenda, &button, &popover, &calendar);
    button.upcast()
}

/// Write the current local time and date into the two labels.
fn refresh(time: &gtk::Label, date: &gtk::Label) {
    let Ok(now) = glib::DateTime::now_local() else { return };
    if let Ok(s) = now.format("%H:%M") {
        time.set_text(&s);
    }
    // Locale-aware abbreviated weekday, day and month, e.g. "Mar 9 set".
    if let Ok(s) = now.format("%a %-d %b") {
        date.set_text(&s);
    }
}

/// Show today on every opening, keep the marks in step with the shown
/// month, and open the day window on a click on a day.
fn wire_calendar(agenda: &Rc<AgendaCenter>, button: &gtk::MenuButton, popover: &gtk::Popover, calendar: &gtk::Calendar) {
    {
        let (agenda, calendar) = (agenda.clone(), calendar.clone());
        popover.connect_show(move |_| {
            if let Ok(now) = glib::DateTime::now_local() { calendar.select_day(&now); }
            mark_busy_days(&agenda, &calendar);
        });
    }
    {
        let agenda = agenda.clone();
        calendar.connect_day_selected(move |c| mark_busy_days(&agenda, c));
    }
    let remark = {
        let agenda = agenda.clone();
        move |c: &gtk::Calendar| mark_busy_days(&agenda, c)
    };
    calendar.connect_next_month(remark.clone());
    calendar.connect_prev_month(remark.clone());
    calendar.connect_next_year(remark.clone());
    calendar.connect_prev_year(remark);

    // Capture phase: the calendar's own gesture would not let a bubbling one
    // see the click. Only clicks on a day number count, not the arrows.
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (agenda, button, popover, calendar) = (agenda.clone(), button.clone(), popover.clone(), calendar.clone());
        click.connect_released(move |_, _, x, y| {
            let on_day = calendar.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w.has_css_class(DAY_NUMBER_CLASS));
            if !on_day { return; }
            // After the calendar has selected the day, and with the popover
            // gone so its grab cannot hold the new window.
            let (agenda, button, popover, calendar) = (agenda.clone(), button.clone(), popover.clone(), calendar.clone());
            glib::idle_add_local_once(move || {
                let date = calendar.date();
                popover.popdown();
                let parent = button.root().and_downcast::<gtk::Window>();
                agenda_window::open(&agenda, parent.as_ref(), &date);
            });
        });
    }
    calendar.add_controller(click);
}

/// Mark the days of the shown month that have appointments.
fn mark_busy_days(agenda: &AgendaCenter, calendar: &gtk::Calendar) {
    let date = calendar.date();
    calendar.clear_marks();
    for day in agenda.busy_days(date.year(), date.month() as u32) {
        calendar.mark_day(day);
    }
}
