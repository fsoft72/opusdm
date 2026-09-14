//! Messages exchanged over the hub socket. `ToHub` travels Lister -> Hub,
//! `ToLister` travels Hub -> Lister. Both are framed JSON (see `frame`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable identifier the hub assigns to each connected Lister.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ListerId(pub u64);

/// Which global role a Lister currently holds, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// The Lister that Copy/Move read from.
    Source,
    /// The Lister that Copy/Move write into.
    Dest,
    /// No role.
    None,
}

/// Minimal metadata shown in the overwrite-conflict prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    /// File size in bytes.
    pub size: u64,
    /// Modification time as whole seconds since the Unix epoch.
    pub modified_unix: i64,
}

/// A filesystem operation the hub can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// Recursively copy `srcs` into `dest_dir`.
    Copy,
    /// Move `srcs` into `dest_dir` (rename fast-path, else copy+delete).
    Move,
    /// Permanently delete `srcs` (no trash).
    Delete,
    /// Create one directory named `name` inside `dest_dir`.
    MakeDir { name: String },
    /// Create one empty file named `name` inside `dest_dir`.
    NewFile { name: String },
}

/// The user's answer to an overwrite conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    /// Skip this one entry.
    Skip,
    /// Overwrite this one entry.
    Overwrite,
    /// Skip this and every later conflict in the job.
    SkipAll,
    /// Overwrite this and every later conflict in the job.
    OverwriteAll,
    /// Abort the whole job now.
    Cancel,
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// Ran to completion.
    Ok { copied: u64, skipped: u64 },
    /// Stopped early on user cancel; `copied`/`skipped` are partial counts.
    Cancelled { copied: u64, skipped: u64 },
    /// Failed before doing useful work (bad destination, all sources gone).
    Error { message: String },
}

/// Messages a Lister sends to the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHub {
    /// First message on a fresh connection.
    Register { pid: u32, initial_path: PathBuf },
    /// Single-instance probe; hub replies `ToLister::Pong`.
    Ping,
    /// Sent by a redundant hub invocation asking the running hub to raise its toolbar.
    RaiseToolbar,
    /// Clean shutdown of this Lister.
    Goodbye,
    /// Explicit role button pressed.
    SetRole(Role),
    /// Pin this Lister's current role against focus changes.
    LockRole,
    /// Release a previous `LockRole`.
    UnlockRole,
    /// This Lister's window gained focus.
    FocusGained,
    /// This Lister navigated to a new directory.
    PathChanged(PathBuf),
    /// Request a filesystem job. `dest_dir` is `None` for a button Copy/Move
    /// (the hub fills it from the DEST Lister's current path) and `Some` for
    /// an explicit destination (drag-drop, Delete, MakeDir, NewFile).
    RunJob { op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf> },
    /// Ask the hub to open another Lister.
    SpawnLister { start_path: Option<PathBuf> },
    /// Ask the hub to raise another Lister's window. Used by the Tab binding to
    /// jump between the SOURCE and DEST panes.
    PresentLister(ListerId),
    /// Answer to a `ToLister::ConflictPrompt`.
    ConflictReply { req_id: u64, resolution: Resolution },
    /// Sent by `opusdm-theme` (not a Lister): replace the hub's desktop
    /// settings. The hub sanitises, persists to
    /// `~/.config/opusdm/desktop.json`, and applies live. No reply.
    SetDesktopSettings(crate::desktop::DesktopSettings),
    /// Sent by `opusdm-theme` (not a Lister): replace the hub's whole
    /// `Settings` (theme, appearance, behavior) atomically. The hub
    /// sanitises the theme's background, persists to settings.json, applies
    /// live to its own desktop, and broadcasts the new `Settings` to every
    /// connected Lister. No reply.
    SetSettings(Settings),
}

/// Background/text colors for the things a Lister paints: the SOURCE and DEST
/// role stat bar, directory/file rows in the file list, and the selection
/// highlight. Hex strings (e.g. "#90EE90"), persisted in the hub's
/// settings.json and distributed to every Lister on connect and on every
/// settings change.
///
/// `#[serde(default)]` fills in fields absent from an older settings.json from
/// [`Default`], so adding a colour never invalidates a user's config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorSettings {
    pub source_bg: String,
    pub source_fg: String,
    pub dest_bg: String,
    pub dest_fg: String,
    pub dir_bg: String,
    pub dir_fg: String,
    pub file_bg: String,
    pub file_fg: String,
    pub selected_bg: String,
    pub selected_fg: String,
}

impl Default for ColorSettings {
    fn default() -> Self {
        Self {
            source_bg: "#90EE90".to_string(),
            source_fg: "#000000".to_string(),
            dest_bg: "#FFB6C1".to_string(),
            dest_fg: "#000000".to_string(),
            dir_bg: "#FFFFFF".to_string(),
            dir_fg: "#000000".to_string(),
            file_bg: "#FFFFFF".to_string(),
            file_fg: "#000000".to_string(),
            selected_bg: "#3584E4".to_string(),
            selected_fg: "#FFFFFF".to_string(),
        }
    }
}

/// Look-and-feel switches that are not part of the theme (icon set, colours,
/// background) - those live in [`crate::theme::Theme`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Append an `ls -F` style type character (`/ * @ = |`) to each name.
    pub type_indicators: bool,
    /// Render the file list in the system monospace font.
    pub monospace_list: bool,
    /// Carry the file colours into the window chrome too. Off by default so a
    /// user who only picked accent colours keeps their system theme.
    pub theme_window: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { type_indicators: false, monospace_list: false, theme_window: false }
    }
}

/// Non-visual behaviour switches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    /// Watch the open directory with inotify and reload it when it changes on
    /// disk, without waiting for a manual refresh.
    pub auto_refresh: bool,
    /// How long a burst of filesystem events is allowed to keep extending
    /// itself before the Lister reloads. Clamped by
    /// [`AUTO_REFRESH_DEBOUNCE_RANGE_MS`] when used.
    pub auto_refresh_debounce_ms: u64,
}

/// Accepted range for [`Behavior::auto_refresh_debounce_ms`]. Too low and a
/// build storm reloads the list constantly; too high and it feels broken.
pub const AUTO_REFRESH_DEBOUNCE_RANGE_MS: (u64, u64) = (50, 5_000);

impl Default for Behavior {
    fn default() -> Self {
        Self { auto_refresh: true, auto_refresh_debounce_ms: 250 }
    }
}

impl Behavior {
    /// The debounce to actually use, clamped into
    /// [`AUTO_REFRESH_DEBOUNCE_RANGE_MS`] so a hand-edited settings.json cannot
    /// make the Lister spin or appear frozen.
    pub fn debounce_ms(&self) -> u64 {
        let (lo, hi) = AUTO_REFRESH_DEBOUNCE_RANGE_MS;
        self.auto_refresh_debounce_ms.clamp(lo, hi)
    }
}

/// Every user-configurable setting the hub owns, persists to
/// settings.json, and distributes to every Lister. This one struct is the
/// wire format (`Welcome`/`SettingsUpdate`), the on-disk JSON shape, and the
/// hub's in-memory state - one place to extend when a new setting is added,
/// instead of a new IPC field plus a new HubCmd variant each time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub theme: crate::theme::Theme,
    pub appearance: Appearance,
    pub behavior: Behavior,
}

/// Reply to `Register`, sent once as its own frame before the connection
/// switches to steady-state `ToLister` traffic. Kept out of `ToLister` so the
/// handshake can't be represented as a message every `ToLister` match has to
/// account for (and then never see, since `Client::connect` consumes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub lister_id: ListerId,
    pub source: Option<ListerId>,
    pub dests: Vec<ListerId>,
    pub source_locked: bool,
    pub settings: Settings,
}

/// Messages the hub sends to a Lister once it is registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToLister {
    /// Reply to `Ping`.
    Pong,
    /// Ask this Lister to raise and focus its window.
    Present,
    /// Global roles changed. There is at most one SOURCE and zero or more
    /// DEST Listers (a locked DEST persists; the unlocked one is transient).
    /// `source_locked` tells a Lister that isn't SOURCE whether trying to
    /// become SOURCE would be rejected, so it can skip that step in its
    /// own role-cycling UI instead of retrying a doomed request forever.
    RoleChanged {
        source: Option<ListerId>,
        dests: Vec<ListerId>,
        source_locked: bool,
    },
    /// Progress tick for a running job.
    JobProgress {
        job_id: u64,
        op: Op,
        current_file: PathBuf,
        bytes_done: u64,
        bytes_total: u64,
        files_done: u64,
        files_total: u64,
        /// Bytes written so far for `current_file` alone. `0` when not tracked.
        file_bytes_done: u64,
        /// Size of `current_file` alone. `0` when not tracked.
        file_bytes_total: u64,
    },
    /// A job finished.
    JobDone {
        job_id: u64,
        outcome: Outcome,
        errors: Vec<(PathBuf, String)>,
    },
    /// A destination entry exists; the Lister must prompt and reply.
    ConflictPrompt {
        req_id: u64,
        job_id: u64,
        path: PathBuf,
        src: FileMeta,
        dst: FileMeta,
    },
    /// A directory changed on disk; reload if showing it.
    InvalidatePath(PathBuf),
    /// Settings changed: the full new `Settings`, replacing the ones from
    /// `Welcome`.
    SettingsUpdate(Settings),
    /// Graceful-exit request.
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{recv, send};
    use std::io::Cursor;

    fn roundtrip_tohub(msg: ToHub) {
        let mut buf = Vec::new();
        send(&mut buf, &msg).unwrap();
        let got: Option<ToHub> = recv(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, Some(msg));
    }

    fn roundtrip_tolister(msg: ToLister) {
        let mut buf = Vec::new();
        send(&mut buf, &msg).unwrap();
        let got: Option<ToLister> = recv(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, Some(msg));
    }

    #[test]
    fn every_tohub_variant_roundtrips() {
        roundtrip_tohub(ToHub::Register { pid: 42, initial_path: "/home/x".into() });
        roundtrip_tohub(ToHub::Ping);
        roundtrip_tohub(ToHub::RaiseToolbar);
        roundtrip_tohub(ToHub::Goodbye);
        roundtrip_tohub(ToHub::SetRole(Role::Dest));
        roundtrip_tohub(ToHub::LockRole);
        roundtrip_tohub(ToHub::UnlockRole);
        roundtrip_tohub(ToHub::FocusGained);
        roundtrip_tohub(ToHub::PathChanged("/tmp".into()));
        roundtrip_tohub(ToHub::RunJob {
            op: Op::Copy,
            srcs: vec!["/a".into(), "/b".into()],
            dest_dir: Some("/dst".into()),
        });
        roundtrip_tohub(ToHub::RunJob { op: Op::Move, srcs: vec![], dest_dir: None });
        roundtrip_tohub(ToHub::SpawnLister { start_path: Some("/etc".into()) });
        roundtrip_tohub(ToHub::PresentLister(ListerId(5)));
        roundtrip_tohub(ToHub::ConflictReply { req_id: 7, resolution: Resolution::OverwriteAll });
        roundtrip_tohub(ToHub::SetDesktopSettings(crate::desktop::DesktopSettings::default()));
        roundtrip_tohub(ToHub::SetSettings(Settings::default()));
    }

    #[test]
    fn set_desktop_settings_carries_the_value() {
        let mut ds = crate::desktop::DesktopSettings::default();
        ds.icon_size = 120;
        let mut buf = Vec::new();
        send(&mut buf, &ToHub::SetDesktopSettings(ds.clone())).unwrap();
        let got: Option<ToHub> = recv(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, Some(ToHub::SetDesktopSettings(ds)));
    }

    #[test]
    fn welcome_roundtrips() {
        let msg = Welcome {
            lister_id: ListerId(1),
            source: Some(ListerId(1)),
            dests: vec![],
            source_locked: false,
            settings: Settings::default(),
        };
        let mut buf = Vec::new();
        send(&mut buf, &msg).unwrap();
        let got: Option<Welcome> = recv(&mut Cursor::new(buf)).unwrap();
        assert_eq!(got, Some(msg));
    }

    #[test]
    fn every_tolister_variant_roundtrips() {
        roundtrip_tolister(ToLister::Pong);
        roundtrip_tolister(ToLister::Present);
        roundtrip_tolister(ToLister::RoleChanged {
            source: None,
            dests: vec![ListerId(3), ListerId(4)],
            source_locked: true,
        });
        roundtrip_tolister(ToLister::JobProgress {
            job_id: 1,
            op: Op::Move,
            current_file: "/a/b".into(),
            bytes_done: 10,
            bytes_total: 100,
            files_done: 1,
            files_total: 9,
            file_bytes_done: 3,
            file_bytes_total: 30,
        });
        roundtrip_tolister(ToLister::JobDone {
            job_id: 1,
            outcome: Outcome::Ok { copied: 9, skipped: 0 },
            errors: vec![("/x".into(), "permission denied".into())],
        });
        roundtrip_tolister(ToLister::ConflictPrompt {
            req_id: 2,
            job_id: 1,
            path: "/dst/f".into(),
            src: FileMeta { size: 1, modified_unix: 10 },
            dst: FileMeta { size: 2, modified_unix: 20 },
        });
        roundtrip_tolister(ToLister::InvalidatePath("/dst".into()));
        roundtrip_tolister(ToLister::SettingsUpdate(Settings::default()));
        roundtrip_tolister(ToLister::Shutdown);
    }

    #[test]
    fn a_partial_settings_file_falls_back_to_defaults_for_missing_fields() {
        let partial = r##"{
            "theme": {
                "colors": {
                    "source_bg": "#111111", "source_fg": "#222222",
                    "dest_bg": "#333333", "dest_fg": "#444444",
                    "dir_bg": "#555555", "dir_fg": "#666666",
                    "file_bg": "#777777", "file_fg": "#888888"
                }
            }
        }"##;
        let s: Settings = serde_json::from_str(partial).expect("partial settings must still parse");
        assert_eq!(s.theme.colors.source_bg, "#111111");
        assert_eq!(s.theme.colors.file_fg, "#888888");
        assert_eq!(s.theme.colors.selected_bg, ColorSettings::default().selected_bg);
        assert_eq!(s.appearance, Appearance::default());
        assert_eq!(s.behavior, Behavior::default());
    }

    #[test]
    fn auto_refresh_is_on_by_default() {
        assert!(Behavior::default().auto_refresh);
    }

    #[test]
    fn a_hand_edited_debounce_is_clamped_to_something_usable() {
        let (lo, hi) = AUTO_REFRESH_DEBOUNCE_RANGE_MS;
        let at = |ms| Behavior { auto_refresh: true, auto_refresh_debounce_ms: ms }.debounce_ms();
        assert_eq!(at(0), lo, "0 would spin the watcher");
        assert_eq!(at(u64::MAX), hi, "a huge value would look frozen");
        assert_eq!(at(lo), lo);
        assert_eq!(at(hi), hi);
        assert_eq!(at(250), 250);
        assert_eq!(Behavior::default().debounce_ms(), 250);
    }

    #[test]
    fn an_empty_settings_file_yields_the_defaults() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn settings_survive_a_full_json_round_trip() {
        let s = Settings {
            theme: crate::theme::Theme {
                icon_set: crate::theme::IconSetId::Custom("breeze-dark".to_string()),
                colors: ColorSettings {
                    source_bg: "#111111".into(),
                    file_fg: "#888888".into(),
                    ..ColorSettings::default()
                },
                background: crate::desktop::Background::default(),
            },
            appearance: Appearance {
                type_indicators: true,
                monospace_list: true,
                theme_window: true,
            },
            behavior: Behavior { auto_refresh: false, auto_refresh_debounce_ms: 900 },
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
    }
}
