//! The private D-Bus interface between `opusdm-notify` (the notification
//! server) and the panel's bell: names, and the history entry as it travels
//! on the bus. No zbus here: an entry crosses the bus as a plain tuple.

/// Well-known bus name `opusdm-notify` owns for the panel.
pub const CONTROL_NAME: &str = "org.opusdm.Notifications1";
/// Object path of the control interface.
pub const CONTROL_PATH: &str = "/org/opusdm/Notifications1";
/// Interface name of the control interface.
pub const CONTROL_IFACE: &str = "org.opusdm.Notifications1";
/// Signal sent after every change of the history, the unread count, DND or
/// the mute list. Carries nothing: the panel asks `GetState` again.
pub const CHANGED_SIGNAL: &str = "Changed";

/// One history entry as `GetState` sends it: id, app name, icon name or
/// path, summary, body (plain text), arrival time (seconds since the epoch),
/// whether it has a default action.
pub type EntryTuple = (u32, String, String, String, String, i64, bool);

/// The answer of `GetState`: DND, unread count, history (newest first),
/// muted app names.
pub type StateTuple = (bool, u32, Vec<EntryTuple>, Vec<String>);

/// One notification kept in the history.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HistoryEntry {
    pub id: u32,
    pub app_name: String,
    /// An icon name of the theme or an absolute file path; empty when none.
    pub icon: String,
    pub summary: String,
    /// The body without markup.
    pub body: String,
    /// Arrival time, seconds since the epoch.
    pub time: i64,
    /// Whether clicking it runs an action of the application.
    pub has_default: bool,
}

impl HistoryEntry {
    /// The entry as it is sent on the bus.
    pub fn to_tuple(&self) -> EntryTuple {
        (self.id, self.app_name.clone(), self.icon.clone(), self.summary.clone(), self.body.clone(), self.time, self.has_default)
    }

    /// The entry read from the bus.
    pub fn from_tuple(t: EntryTuple) -> Self {
        let (id, app_name, icon, summary, body, time, has_default) = t;
        Self { id, app_name, icon, summary, body, time, has_default }
    }
}

/// What the panel shows: the state of the notification server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotifyState {
    pub dnd: bool,
    pub unread: u32,
    /// Newest first.
    pub history: Vec<HistoryEntry>,
    pub muted: Vec<String>,
}

impl NotifyState {
    /// The state as it is sent on the bus.
    pub fn to_tuple(&self) -> StateTuple {
        (self.dnd, self.unread, self.history.iter().map(HistoryEntry::to_tuple).collect(), self.muted.clone())
    }

    /// The state read from the bus.
    pub fn from_tuple(t: StateTuple) -> Self {
        let (dnd, unread, history, muted) = t;
        Self { dnd, unread, history: history.into_iter().map(HistoryEntry::from_tuple).collect(), muted }
    }

    /// Whether `app_name` is muted (names compare without case).
    pub fn is_muted(&self, app_name: &str) -> bool {
        self.muted.iter().any(|m| m.eq_ignore_ascii_case(app_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_survives_the_trip_through_tuples() {
        let entry = HistoryEntry { id: 7, app_name: "Brave".into(), icon: "brave-browser".into(), summary: "Anna".into(), body: "hi".into(), time: 1_700_000_000, has_default: true };
        let state = NotifyState { dnd: true, unread: 3, history: vec![entry], muted: vec!["Spotify".into()] };
        assert_eq!(NotifyState::from_tuple(state.to_tuple()), state);
    }

    #[test]
    fn mute_matching_ignores_case() {
        let state = NotifyState { muted: vec!["Spotify".into()], ..NotifyState::default() };
        assert!(state.is_muted("spotify"));
        assert!(!state.is_muted("Brave"));
    }
}
