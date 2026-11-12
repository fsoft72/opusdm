//! The tray icon (a StatusNotifierItem with a dbusmenu) shown while the
//! keyring has a problem.

use crate::health::Reason;
use ksni::menu::{MenuItem, StandardItem};
use std::sync::mpsc::Sender;

/// Tag used in log lines.
const TAG: &str = "opusdm-keyring-watch";
/// The themed icon of the tray item.
const ICON: &str = "dialog-password";
/// The title of the tray item.
const TITLE: &str = "Keyring problem";

/// What a menu entry asks the watcher to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Unlock through the Secret Service (the provider shows its prompt).
    Unlock,
    /// Open the password window (gnome-keyring only).
    UnlockWithPassword,
    /// Restart gnome-keyring, asking the password first.
    Restart,
    /// Write, read and delete a test item.
    Test,
    /// Run the check now.
    CheckNow,
}

/// The entries of the menu for `reason`, after the disabled first line that
/// shows the reason. Unlock and Test are for a locked keyring (Test is for a
/// keyring that is good or only locked); Restart and the password window are
/// gnome-keyring only; Check now is always there.
pub fn menu_labels(reason: Reason, gnome: bool) -> Vec<(Cmd, &'static str)> {
    let mut labels = Vec::new();
    if reason == Reason::Locked { labels.push((Cmd::Unlock, "Unlock")); }
    if reason == Reason::Locked && gnome { labels.push((Cmd::UnlockWithPassword, "Unlock with password...")); }
    if reason == Reason::Locked { labels.push((Cmd::Test, "Test")); }
    if gnome && matches!(reason, Reason::NoService | Reason::Locked | Reason::Timeout) { labels.push((Cmd::Restart, "Restart gnome-keyring...")); }
    labels.push((Cmd::CheckNow, "Check now"));
    labels
}

/// The tray item: shows the reason and sends the chosen [`Cmd`] to the
/// watcher.
pub struct KeyringTray {
    /// What is wrong now.
    pub reason: Reason,
    /// Whether the Secret Service is gnome-keyring.
    pub gnome: bool,
    /// Where the chosen commands go.
    pub tx: Sender<Cmd>,
}

impl ksni::Tray for KeyringTray {
    /// See [`ksni::Tray::id`].
    fn id(&self) -> String { "opusdm-keyring-watch".into() }

    /// See [`ksni::Tray::icon_name`].
    fn icon_name(&self) -> String { ICON.into() }

    /// See [`ksni::Tray::title`].
    fn title(&self) -> String { TITLE.into() }

    /// See [`ksni::Tray::status`]: always shown.
    fn status(&self) -> ksni::Status { ksni::Status::Active }

    /// See [`ksni::Tray::category`].
    fn category(&self) -> ksni::Category { ksni::Category::SystemServices }

    /// See [`ksni::Tray::menu`]: the reason, then the entries of [`menu_labels`].
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = vec![StandardItem { label: self.reason.text().into(), enabled: false, ..Default::default() }.into(), MenuItem::Separator];
        for (cmd, label) in menu_labels(self.reason, self.gnome) {
            items.push(
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |tray: &mut Self| {
                        if let Err(e) = tray.tx.send(cmd) { opusdm_ipc::err_log!(TAG, "cannot send {cmd:?} to the watcher: {e}"); }
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(reason: Reason, gnome: bool) -> Vec<Cmd> {
        menu_labels(reason, gnome).into_iter().map(|(c, _)| c).collect()
    }

    #[test]
    fn locked_gnome_has_every_entry() {
        assert_eq!(cmds(Reason::Locked, true), vec![Cmd::Unlock, Cmd::UnlockWithPassword, Cmd::Test, Cmd::Restart, Cmd::CheckNow]);
    }

    #[test]
    fn locked_other_provider_has_no_gnome_entries() {
        assert_eq!(cmds(Reason::Locked, false), vec![Cmd::Unlock, Cmd::Test, Cmd::CheckNow]);
    }

    #[test]
    fn no_service_offers_restart_only_on_gnome() {
        assert_eq!(cmds(Reason::NoService, true), vec![Cmd::Restart, Cmd::CheckNow]);
        assert_eq!(cmds(Reason::NoService, false), vec![Cmd::CheckNow]);
    }

    #[test]
    fn timeout_offers_restart_on_gnome() {
        assert_eq!(cmds(Reason::Timeout, true), vec![Cmd::Restart, Cmd::CheckNow]);
    }

    #[test]
    fn no_bus_and_no_collection_only_check_now() {
        for reason in [Reason::NoBus, Reason::NoCollection] {
            for gnome in [true, false] { assert_eq!(cmds(reason, gnome), vec![Cmd::CheckNow]); }
        }
    }
}
