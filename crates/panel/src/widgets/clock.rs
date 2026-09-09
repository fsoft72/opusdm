//! Two-line clock: a large `HH:MM` over a small locale-formatted date.

use gtk4 as gtk;
use gtk::prelude::*;
use std::time::Duration;

/// Tick interval. Five seconds is plenty at minute display granularity and
/// keeps the label from drifting more than a few seconds behind the wall
/// clock after a suspend/resume.
const TICK: Duration = Duration::from_secs(5);

/// Build the clock widget. It refreshes itself for the lifetime of the
/// returned box.
pub fn build() -> gtk::Widget {
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
        gtk::glib::timeout_add_local(TICK, move || {
            refresh(&time, &date);
            gtk::glib::ControlFlow::Continue
        });
    }

    root.upcast()
}

/// Write the current local time and date into the two labels.
fn refresh(time: &gtk::Label, date: &gtk::Label) {
    let Ok(now) = gtk::glib::DateTime::now_local() else { return };
    if let Ok(s) = now.format("%H:%M") {
        time.set_text(&s);
    }
    // Locale-aware abbreviated weekday, day and month, e.g. "Mar 9 set".
    if let Ok(s) = now.format("%a %-d %b") {
        date.set_text(&s);
    }
}
