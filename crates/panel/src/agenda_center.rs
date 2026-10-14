//! The panel side of the appointments: the agenda in memory, its file, the
//! timer that rings the alarms as desktop notifications, and the open day
//! window. Owned by `attach`, so a rebuild of the bars keeps it.

use crate::agenda_model::{self, Agenda, Appointment};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// How often the alarms are checked.
const ALARM_TICK: Duration = Duration::from_secs(15);
/// Name and icon of the alarm notifications.
const NOTIFY_APP_NAME: &str = "Calendar";
const NOTIFY_ICON: &str = "x-office-calendar";
/// Urgency hint value: critical pops even in Do not disturb and stays until
/// dismissed.
const URGENCY_CRITICAL: u8 = 2;

/// Shared state of the clock calendars.
pub struct AgendaCenter {
    agenda: RefCell<Agenda>,
    path: PathBuf,
    /// The day window, so a second click on a day replaces it.
    day_window: RefCell<Option<glib::WeakRef<gtk::Window>>>,
}

impl AgendaCenter {
    /// Load the appointments and start the alarm timer.
    pub fn new() -> Rc<Self> {
        let path = agenda_model::agenda_path();
        let agenda = agenda_model::load_agenda_from(&path);
        let center = Rc::new(Self { agenda: RefCell::new(agenda), path, day_window: RefCell::new(None) });
        center.check_alarms();
        {
            let weak = Rc::downgrade(&center);
            glib::timeout_add_local(ALARM_TICK, move || {
                let Some(center) = weak.upgrade() else { return glib::ControlFlow::Break };
                center.check_alarms();
                glib::ControlFlow::Continue
            });
        }
        center
    }

    /// The appointments of one day, by start time.
    pub fn on_day(&self, year: i32, month: u32, day: u32) -> Vec<Appointment> {
        self.agenda.borrow().on_day(year, month, day)
    }

    /// The days of `year`-`month` with appointments.
    pub fn busy_days(&self, year: i32, month: u32) -> Vec<u32> {
        self.agenda.borrow().busy_days(year, month)
    }

    /// Store `appointment` (id 0 adds it) and write the file. Returns the id.
    pub fn save(&self, appointment: Appointment) -> Result<u64, String> {
        let id = self.agenda.borrow_mut().save(appointment);
        self.write()?;
        // An alarm already due (e.g. set 5 minutes before a start 2 minutes
        // away) rings now, not at the next tick.
        self.check_alarms();
        Ok(id)
    }

    /// Delete appointment `id` and write the file.
    pub fn remove(&self, id: u64) -> Result<(), String> {
        if !self.agenda.borrow_mut().remove(id) { return Err(format!("appointment {id} not found")); }
        self.write()
    }

    /// Remember `window` as the open day window, closing the previous one.
    pub fn set_day_window(&self, window: &gtk::Window) {
        let old = self.day_window.replace(Some(window.downgrade()));
        if let Some(old) = old.and_then(|w| w.upgrade()) { old.close(); }
    }

    /// Write the agenda to its file.
    fn write(&self) -> Result<(), String> {
        agenda_model::save_agenda_to(&self.path, &self.agenda.borrow()).map_err(|e| {
            let msg = format!("cannot save {}: {e}", self.path.display());
            opusdm_ipc::err_log!("opusdm-panel", "agenda: {msg}");
            msg
        })
    }

    /// Ring the alarms that are due and remember that they rang.
    fn check_alarms(&self) {
        let Some(now) = now_minutes() else { return };
        let (due, changed) = self.agenda.borrow_mut().take_due(now);
        if changed { let _ = self.write(); }
        for appointment in due {
            send_alarm(&appointment, now);
        }
    }
}

/// The local wall clock now, in [`agenda_model::local_minutes`].
fn now_minutes() -> Option<i64> {
    let now = glib::DateTime::now_local().ok()?;
    Some(agenda_model::local_minutes(now.year(), now.month() as u32, now.day_of_month() as u32, now.hour() as u32, now.minute() as u32))
}

/// The body of an alarm notification seen at `now`.
pub fn alarm_body(appointment: &Appointment, now: i64) -> String {
    let left = appointment.start_minutes() - now;
    let when = match left {
        l if l > 0 => format!("in {l} min"),
        0 => "now".to_string(),
        l => format!("{} min ago", -l),
    };
    format!("{} ({when})", appointment.time_text())
}

/// Send the alarm of `appointment` to the notification server on a short
/// thread; a failure is logged.
fn send_alarm(appointment: &Appointment, now: i64) {
    let summary = appointment.text.clone();
    let body = alarm_body(appointment, now);
    let spawned = std::thread::Builder::new().name("opusdm-panel-agenda-alarm".into()).spawn(move || {
        if let Err(e) = notify(&summary, &body) {
            opusdm_ipc::err_log!("opusdm-panel", "agenda: alarm {summary:?} not sent: {e}");
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-panel", "agenda: cannot start a thread for an alarm: {e}");
    }
}

/// `org.freedesktop.Notifications.Notify` with a critical urgency.
fn notify(summary: &str, body: &str) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    let actions: Vec<&str> = Vec::new();
    let mut hints: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
    hints.insert("urgency", zbus::zvariant::Value::U8(URGENCY_CRITICAL));
    let args = (NOTIFY_APP_NAME, 0u32, NOTIFY_ICON, summary, body, actions, hints, -1i32);
    conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &args,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alarm_bodies() {
        let a = Appointment { year: 2026, month: 10, day: 14, hour: 9, minute: 30, ..Default::default() };
        let start = a.start_minutes();
        assert_eq!(alarm_body(&a, start - 10), "09:30 (in 10 min)");
        assert_eq!(alarm_body(&a, start), "09:30 (now)");
        assert_eq!(alarm_body(&a, start + 3), "09:30 (3 min ago)");
    }
}
