//! Appointments of the clock calendar: the data, the rules that pick the
//! alarms to ring, and the file they live in
//! (`~/.config/opusdm/appointments.json`). No GTK here.

use opusdm_ipc::settings::{config_dir, load_json, save_json};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Minutes before the start an alarm can ring at; 0 is "no alarm".
pub const ALARM_CHOICES: [u32; 6] = [0, 5, 10, 20, 30, 60];
/// An alarm missed while the panel was not running still rings up to this
/// many minutes after the start; later it is dropped without a word.
pub const LATE_ALARM_MINUTES: i64 = 60;
/// Minutes in a day.
const DAY_MINUTES: i64 = 24 * 60;

/// One appointment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Appointment {
    /// Unique in the file, never 0 once stored.
    pub id: u64,
    pub text: String,
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    /// 0 to 23.
    pub hour: u32,
    /// 0 to 59.
    pub minute: u32,
    /// Minutes before the start the alarm rings; 0 is no alarm.
    pub alarm_minutes: u32,
    /// Whether the alarm already rang (or was dropped as too late).
    pub notified: bool,
}

impl Appointment {
    /// The start, in local minutes (see [`local_minutes`]).
    pub fn start_minutes(&self) -> i64 {
        local_minutes(self.year, self.month, self.day, self.hour, self.minute)
    }

    /// When the alarm rings, in local minutes; `None` without an alarm.
    pub fn alarm_at(&self) -> Option<i64> {
        if self.alarm_minutes == 0 { return None; }
        Some(self.start_minutes() - i64::from(self.alarm_minutes))
    }

    /// The start as `HH:MM`.
    pub fn time_text(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// Whether the appointment is on `year`-`month`-`day`.
    pub fn is_on(&self, year: i32, month: u32, day: u32) -> bool {
        self.year == year && self.month == month && self.day == day
    }
}

/// Every appointment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Agenda {
    pub items: Vec<Appointment>,
}

impl Agenda {
    /// The appointments of one day, by start time.
    pub fn on_day(&self, year: i32, month: u32, day: u32) -> Vec<Appointment> {
        let mut out: Vec<Appointment> = self.items.iter().filter(|a| a.is_on(year, month, day)).cloned().collect();
        out.sort_by_key(|a| (a.hour, a.minute, a.id));
        out
    }

    /// The days of `year`-`month` with at least one appointment, ascending.
    pub fn busy_days(&self, year: i32, month: u32) -> Vec<u32> {
        let mut days: Vec<u32> = self.items.iter().filter(|a| a.year == year && a.month == month).map(|a| a.day).collect();
        days.sort_unstable();
        days.dedup();
        days
    }

    /// Store `appointment`: a new one (id 0) gets a fresh id, an existing id
    /// is replaced. A replaced appointment rings again only if the moment of
    /// its alarm changed. Returns the id.
    pub fn save(&mut self, mut appointment: Appointment) -> u64 {
        if let Some(old) = self.items.iter_mut().find(|a| a.id == appointment.id && a.id != 0) {
            appointment.notified = old.notified && old.alarm_at() == appointment.alarm_at();
            *old = appointment;
            return old.id;
        }
        appointment.id = self.items.iter().map(|a| a.id).max().unwrap_or(0) + 1;
        appointment.notified = false;
        let id = appointment.id;
        self.items.push(appointment);
        id
    }

    /// Remove appointment `id`; `false` when there is none.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|a| a.id != id);
        self.items.len() != before
    }

    /// The alarms to ring at `now` (local minutes), marked as rung. Alarms
    /// more than [`LATE_ALARM_MINUTES`] past the start are marked without
    /// ringing. Returns the ones to ring and whether anything changed.
    pub fn take_due(&mut self, now: i64) -> (Vec<Appointment>, bool) {
        let mut due = Vec::new();
        let mut changed = false;
        for a in self.items.iter_mut().filter(|a| !a.notified) {
            let Some(at) = a.alarm_at() else { continue };
            if at > now { continue; }
            a.notified = true;
            changed = true;
            if now <= a.start_minutes() + LATE_ALARM_MINUTES { due.push(a.clone()); }
        }
        (due, changed)
    }
}

/// A wall clock moment as minutes since 1970-01-01 00:00 local time, so two
/// moments compare and subtract without a time zone.
pub fn local_minutes(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    days_from_civil(year, month, day) * DAY_MINUTES + i64::from(hour) * 60 + i64::from(minute)
}

/// Days from 1970-01-01 to `year`-`month`-`day` (proleptic Gregorian).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Read a time typed by the user: `H:MM`, `HH:MM`, `H.MM` or `HH.MM`
/// (surrounding spaces ignored). `None` when it is not a valid time.
pub fn parse_time(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.trim().split_once([':', '.'])?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 { return None; }
    if !h.chars().chain(m.chars()).all(|c| c.is_ascii_digit()) { return None; }
    let (hour, minute) = (h.parse().ok()?, m.parse().ok()?);
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// The label of an alarm choice in the drop-down.
pub fn alarm_label(minutes: u32) -> String {
    match minutes {
        0 => "No alarm".to_string(),
        60 => "1 hour".to_string(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} mins"),
    }
}

/// Path to `appointments.json`.
pub fn agenda_path() -> PathBuf {
    config_dir().join("appointments.json")
}

/// Load the appointments from `path` (empty when missing; a file that does
/// not parse is backed up to `.bak` by [`load_json`]).
pub fn load_agenda_from(path: &Path) -> Agenda {
    load_json(path, "opusdm-panel", "appointments")
}

/// Save the appointments to `path`.
pub fn save_agenda_to(path: &Path, agenda: &Agenda) -> io::Result<()> {
    save_json(path, agenda)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32, alarm: u32) -> Appointment {
        Appointment { text: "x".into(), year: 2026, month: 10, day, hour, minute, alarm_minutes: alarm, ..Default::default() }
    }

    #[test]
    fn local_minutes_counts_from_the_epoch() {
        assert_eq!(local_minutes(1970, 1, 1, 0, 0), 0);
        assert_eq!(local_minutes(1970, 1, 2, 1, 30), DAY_MINUTES + 90);
        // 2000-03-01 is day 11017; checks the leap day before it.
        assert_eq!(local_minutes(2000, 3, 1, 0, 0), 11_017 * DAY_MINUTES);
        assert_eq!(local_minutes(2026, 1, 1, 0, 0) - local_minutes(2025, 12, 31, 23, 59), 1);
    }

    #[test]
    fn times_are_parsed_strictly() {
        assert_eq!(parse_time("9:05"), Some((9, 5)));
        assert_eq!(parse_time(" 23.59 "), Some((23, 59)));
        assert_eq!(parse_time("00:00"), Some((0, 0)));
        for bad in ["", "9", "24:00", "12:60", "1:5", "123:00", "ab:cd", "-1:00", "12:5x"] {
            assert_eq!(parse_time(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn alarm_labels() {
        let labels: Vec<String> = ALARM_CHOICES.iter().map(|m| alarm_label(*m)).collect();
        assert_eq!(labels, ["No alarm", "5 mins", "10 mins", "20 mins", "30 mins", "1 hour"]);
    }

    #[test]
    fn save_gives_ids_and_replaces() {
        let mut agenda = Agenda::default();
        let a = agenda.save(at(14, 9, 0, 0));
        let b = agenda.save(at(14, 8, 0, 0));
        assert_eq!((a, b), (1, 2));
        let mut edited = agenda.on_day(2026, 10, 14)[1].clone();
        assert_eq!(edited.id, a, "sorted by time: 08:00 first");
        edited.text = "changed".into();
        assert_eq!(agenda.save(edited), a);
        assert_eq!(agenda.items.len(), 2);
        assert!(agenda.remove(a));
        assert!(!agenda.remove(a));
        assert_eq!(agenda.save(at(1, 0, 0, 0)), 3, "ids are not reused while a bigger one exists");
    }

    #[test]
    fn busy_days_are_unique_and_of_the_month() {
        let mut agenda = Agenda::default();
        for a in [at(20, 9, 0, 0), at(3, 9, 0, 0), at(20, 10, 0, 0)] {
            agenda.save(a);
        }
        agenda.save(Appointment { month: 11, ..at(5, 0, 0, 0) });
        assert_eq!(agenda.busy_days(2026, 10), [3, 20]);
        assert_eq!(agenda.busy_days(2026, 11), [5]);
        assert!(agenda.busy_days(2025, 10).is_empty());
    }

    #[test]
    fn alarms_ring_once_from_their_moment() {
        let mut agenda = Agenda::default();
        let id = agenda.save(at(14, 10, 0, 10));
        agenda.save(at(14, 10, 0, 0));
        let start = local_minutes(2026, 10, 14, 10, 0);
        assert_eq!(agenda.take_due(start - 11), (vec![], false));
        let (due, changed) = agenda.take_due(start - 10);
        assert!(changed);
        assert_eq!(due.iter().map(|a| a.id).collect::<Vec<_>>(), [id]);
        assert_eq!(agenda.take_due(start), (vec![], false), "rings only once");
    }

    #[test]
    fn a_missed_alarm_rings_late_or_is_dropped() {
        let start = local_minutes(2026, 10, 14, 10, 0);
        let mut agenda = Agenda::default();
        agenda.save(at(14, 10, 0, 5));
        assert_eq!(agenda.take_due(start + LATE_ALARM_MINUTES).0.len(), 1);
        let mut agenda = Agenda::default();
        agenda.save(at(14, 10, 0, 5));
        let (due, changed) = agenda.take_due(start + LATE_ALARM_MINUTES + 1);
        assert!(due.is_empty() && changed, "too late: marked, not rung");
    }

    #[test]
    fn editing_rearms_only_a_moved_alarm() {
        let start = local_minutes(2026, 10, 14, 10, 0);
        let mut agenda = Agenda::default();
        let id = agenda.save(at(14, 10, 0, 10));
        agenda.take_due(start);
        let mut a = agenda.items[0].clone();
        a.text = "renamed".into();
        agenda.save(a.clone());
        assert!(agenda.items[0].notified, "same moment: stays rung");
        a.hour = 11;
        agenda.save(a);
        assert!(!agenda.items[0].notified, "moved: rings again");
        assert_eq!(agenda.items[0].id, id);
    }

    #[test]
    fn the_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("appointments.json");
        assert_eq!(load_agenda_from(&path), Agenda::default());
        let mut agenda = Agenda::default();
        agenda.save(at(14, 9, 30, 20));
        save_agenda_to(&path, &agenda).unwrap();
        assert_eq!(load_agenda_from(&path), agenda);
    }
}
