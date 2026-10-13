//! Every notification that is still open, and the rules that decide where a
//! new one goes. A notification is open while it has a popup on screen (or
//! waiting for room) or sits in the history; it is closed, and the
//! application told so, once it has neither. No GTK here.

use crate::config::Config;
use crate::markup;
use crate::model::{Notification, Urgency};
use opusdm_ipc::notify::{HistoryEntry, NotifyState};

/// Entries kept in the history; the oldest goes when a new one comes.
pub const HISTORY_MAX: usize = 100;
/// Popup time for low urgency notifications that ask for the default.
pub const LOW_TIMEOUT_MS: u32 = 3000;

/// Why a notification closed, as `NotificationClosed` sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

/// Where a new notification goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// A popup, and into the history when `keep`.
    Popup { keep: bool },
    /// Into the history only (Do Not Disturb, muted app).
    HistoryOnly,
    /// Nowhere: it closes at once (transient while popups are held back).
    Drop,
}

/// What [`Store::notify`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub id: u32,
    pub route: Route,
    /// It replaced an open notification that had a popup.
    pub had_popup: bool,
    /// Notifications closed to make room in the history (reason: expired).
    pub evicted: Vec<u32>,
}

/// One open notification.
#[derive(Debug, Clone)]
struct Open {
    n: Notification,
    popup: bool,
    kept: bool,
    unread: bool,
}

/// What [`Store::remove`] took away.
#[derive(Debug, Clone)]
pub struct Removed {
    pub notification: Notification,
    /// It had a popup that must go too.
    pub had_popup: bool,
}

/// The open notifications and the settings.
#[derive(Debug, Default)]
pub struct Store {
    last_id: u32,
    /// Oldest first.
    open: Vec<Open>,
    config: Config,
}

impl Store {
    /// An empty store with `config`.
    pub fn new(config: Config) -> Self {
        Store { last_id: 0, open: Vec::new(), config }
    }

    /// The settings.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Where `n` goes under the current settings.
    pub fn route(&self, n: &Notification) -> Route {
        let held_back = self.config.dnd || self.config.is_muted(&n.app_name);
        match (n.urgency == Urgency::Critical, held_back, n.transient) {
            (true, _, transient) | (false, false, transient) => Route::Popup { keep: !transient },
            (false, true, false) => Route::HistoryOnly,
            (false, true, true) => Route::Drop,
        }
    }

    /// Take in `n`: give it an id (the one of `replaces_id` when that one is
    /// still open) and route it.
    pub fn notify(&mut self, mut n: Notification, replaces_id: u32) -> Added {
        let old = self.position(replaces_id).map(|i| self.open.remove(i));
        n.id = match &old {
            Some(o) => o.n.id,
            None => self.next_id(),
        };
        let had_popup = old.is_some_and(|o| o.popup);
        let route = self.route(&n);
        let id = n.id;
        let (popup, kept) = match route {
            Route::Popup { keep } => (true, keep),
            Route::HistoryOnly => (false, true),
            Route::Drop => (false, false),
        };
        if popup || kept {
            self.open.push(Open { n, popup, kept, unread: kept });
        }
        let evicted = self.trim_history();
        Added { id, route, had_popup, evicted }
    }

    /// The popup of `id` is gone by itself (its time ran out). Returns
    /// `true` when the notification is now closed (it was not in the history).
    pub fn popup_expired(&mut self, id: u32) -> bool {
        let Some(i) = self.position(id) else { return false };
        self.open[i].popup = false;
        if self.open[i].kept { return false; }
        self.open.remove(i);
        true
    }

    /// Close `id` (dismissed, closed by the app, action run, removed from the
    /// history).
    pub fn remove(&mut self, id: u32) -> Option<Removed> {
        let i = self.position(id)?;
        let o = self.open.remove(i);
        Some(Removed { notification: o.n, had_popup: o.popup })
    }

    /// Close every notification of the history. Returns them with whether
    /// they had a popup.
    pub fn clear_history(&mut self) -> Vec<Removed> {
        let (gone, stay): (Vec<Open>, Vec<Open>) = std::mem::take(&mut self.open).into_iter().partition(|o| o.kept);
        self.open = stay;
        gone.into_iter().map(|o| Removed { notification: o.n, had_popup: o.popup }).collect()
    }

    /// The open notification `id`.
    pub fn get(&self, id: u32) -> Option<&Notification> {
        self.position(id).map(|i| &self.open[i].n)
    }

    /// Everything in the history counts as seen.
    pub fn mark_read(&mut self) -> bool {
        let mut changed = false;
        for o in self.open.iter_mut().filter(|o| o.unread) {
            o.unread = false;
            changed = true;
        }
        changed
    }

    /// History entries not seen yet.
    pub fn unread(&self) -> u32 {
        self.open.iter().filter(|o| o.unread).count() as u32
    }

    /// Turn Do Not Disturb on or off; `false` when nothing changed.
    pub fn set_dnd(&mut self, on: bool) -> bool {
        if self.config.dnd == on { return false; }
        self.config.dnd = on;
        true
    }

    /// Mute `app_name`; `false` when nothing changed.
    pub fn mute(&mut self, app_name: &str) -> bool {
        self.config.mute(app_name)
    }

    /// Unmute `app_name`; `false` when nothing changed.
    pub fn unmute(&mut self, app_name: &str) -> bool {
        self.config.unmute(app_name)
    }

    /// What the panel shows.
    pub fn state(&self) -> NotifyState {
        let history = self.open.iter().rev().filter(|o| o.kept).map(|o| entry(&o.n)).collect();
        NotifyState { dnd: self.config.dnd, unread: self.unread(), history, muted: self.config.mute.clone() }
    }

    /// How long the popup of `n` stays, in milliseconds; `None` for ever.
    pub fn expire_ms(&self, n: &Notification) -> Option<u32> {
        if n.urgency == Urgency::Critical { return None; }
        match n.expire_timeout {
            0 => None,
            t if t > 0 => Some(t as u32),
            _ if n.urgency == Urgency::Low => Some(LOW_TIMEOUT_MS.min(self.config.timeout_ms())),
            _ => Some(self.config.timeout_ms()),
        }
    }

    /// Index of the open notification `id`.
    fn position(&self, id: u32) -> Option<usize> {
        if id == 0 { return None; }
        self.open.iter().position(|o| o.n.id == id)
    }

    /// A fresh id: never 0, never one still open.
    fn next_id(&mut self) -> u32 {
        loop {
            self.last_id = self.last_id.wrapping_add(1);
            if self.last_id != 0 && self.position(self.last_id).is_none() { return self.last_id; }
        }
    }

    /// Drop the oldest history entries beyond [`HISTORY_MAX`]. Those without
    /// a popup close; their ids are returned.
    fn trim_history(&mut self) -> Vec<u32> {
        let mut closed = Vec::new();
        while self.open.iter().filter(|o| o.kept).count() > HISTORY_MAX {
            let Some(i) = self.open.iter().position(|o| o.kept) else { break };
            if self.open[i].popup {
                self.open[i].kept = false;
                self.open[i].unread = false;
            } else {
                closed.push(self.open.remove(i).n.id);
            }
        }
        closed
    }
}

/// The history entry of `n`.
fn entry(n: &Notification) -> HistoryEntry {
    HistoryEntry {
        id: n.id,
        app_name: n.app_name.clone(),
        icon: n.icon_hint(),
        summary: n.summary.clone(),
        body: markup::plain_text(&n.body),
        time: n.time,
        has_default: n.default_action().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Action;

    fn note(app: &str) -> Notification {
        Notification { app_name: app.into(), summary: "s".into(), expire_timeout: -1, ..Notification::default() }
    }

    fn critical(app: &str) -> Notification {
        Notification { urgency: Urgency::Critical, ..note(app) }
    }

    fn transient(app: &str) -> Notification {
        Notification { transient: true, ..note(app) }
    }

    #[test]
    fn routing_table() {
        let mut s = Store::default();
        assert_eq!(s.route(&note("a")), Route::Popup { keep: true });
        assert_eq!(s.route(&transient("a")), Route::Popup { keep: false });
        s.set_dnd(true);
        assert_eq!(s.route(&note("a")), Route::HistoryOnly);
        assert_eq!(s.route(&transient("a")), Route::Drop);
        assert_eq!(s.route(&critical("a")), Route::Popup { keep: true });
        s.set_dnd(false);
        s.mute("Spotify");
        assert_eq!(s.route(&note("spotify")), Route::HistoryOnly);
        assert_eq!(s.route(&note("Brave")), Route::Popup { keep: true });
    }

    #[test]
    fn ids_start_at_one_and_replacing_keeps_the_id() {
        let mut s = Store::default();
        let a = s.notify(note("a"), 0);
        let b = s.notify(note("b"), 0);
        assert_eq!((a.id, b.id), (1, 2));
        let again = s.notify(Notification { summary: "new".into(), ..note("a") }, a.id);
        assert_eq!(again.id, 1);
        assert!(again.had_popup);
        assert_eq!(s.get(1).unwrap().summary, "new");
        assert_eq!(s.state().history.len(), 2);
        // The replaced one moved to the top of the history.
        assert_eq!(s.state().history[0].id, 1);
    }

    #[test]
    fn replacing_a_closed_id_gets_a_new_one() {
        let mut s = Store::default();
        let a = s.notify(note("a"), 0);
        s.remove(a.id);
        assert_eq!(s.notify(note("a"), a.id).id, 2);
    }

    #[test]
    fn the_counter_wraps_without_zero_or_open_ids() {
        let mut s = Store { last_id: u32::MAX - 1, ..Store::default() };
        assert_eq!(s.notify(note("a"), 0).id, u32::MAX);
        assert_eq!(s.notify(note("b"), 0).id, 1);
    }

    #[test]
    fn an_expired_popup_stays_in_the_history() {
        let mut s = Store::default();
        let a = s.notify(note("a"), 0);
        assert!(!s.popup_expired(a.id));
        assert_eq!(s.state().history.len(), 1);
        let t = s.notify(transient("a"), 0);
        assert!(s.popup_expired(t.id), "a transient one closes");
        assert!(s.get(t.id).is_none());
    }

    #[test]
    fn dropped_notifications_are_not_kept() {
        let mut s = Store::default();
        s.set_dnd(true);
        let t = s.notify(transient("a"), 0);
        assert_eq!(t.route, Route::Drop);
        assert!(s.get(t.id).is_none());
    }

    #[test]
    fn the_history_is_capped() {
        let mut s = Store::default();
        s.set_dnd(true);
        let first = s.notify(note("a"), 0).id;
        for _ in 1..HISTORY_MAX {
            assert!(s.notify(note("a"), 0).evicted.is_empty());
        }
        let over = s.notify(note("a"), 0);
        assert_eq!(over.evicted, vec![first]);
        assert_eq!(s.state().history.len(), HISTORY_MAX);
    }

    #[test]
    fn unread_counts_history_entries_until_read() {
        let mut s = Store::default();
        let a = s.notify(note("a"), 0).id;
        s.notify(note("b"), 0);
        s.notify(transient("c"), 0);
        assert_eq!(s.unread(), 2);
        s.remove(a);
        assert_eq!(s.unread(), 1);
        assert!(s.mark_read());
        assert!(!s.mark_read());
        assert_eq!(s.unread(), 0);
    }

    #[test]
    fn clearing_the_history_keeps_transient_popups() {
        let mut s = Store::default();
        s.notify(note("a"), 0);
        let t = s.notify(transient("b"), 0).id;
        let gone = s.clear_history();
        assert_eq!(gone.len(), 1);
        assert!(gone[0].had_popup);
        assert!(s.get(t).is_some());
        assert!(s.state().history.is_empty());
    }

    #[test]
    fn timeouts() {
        let s = Store::default();
        assert_eq!(s.expire_ms(&note("a")), Some(crate::config::DEFAULT_TIMEOUT_MS));
        assert_eq!(s.expire_ms(&Notification { urgency: Urgency::Low, ..note("a") }), Some(LOW_TIMEOUT_MS));
        assert_eq!(s.expire_ms(&Notification { expire_timeout: 0, ..note("a") }), None);
        assert_eq!(s.expire_ms(&Notification { expire_timeout: 1500, ..note("a") }), Some(1500));
        assert_eq!(s.expire_ms(&Notification { expire_timeout: 1500, ..critical("a") }), None);
    }

    #[test]
    fn the_state_shows_plain_text_newest_first() {
        let mut s = Store::default();
        s.notify(Notification { body: "<b>hi</b> &amp; bye".into(), ..note("a") }, 0);
        let with_default = Notification { actions: vec![Action { key: "default".into(), label: "Open".into() }], ..note("b") };
        s.notify(with_default, 0);
        let state = s.state();
        assert_eq!(state.history[0].app_name, "b");
        assert!(state.history[0].has_default);
        assert_eq!(state.history[1].body, "hi & bye");
    }
}
