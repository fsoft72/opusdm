//! Messages exchanged over the hub socket. `ToHub` travels Lister -> Hub,
//! `ToLister` travels Hub -> Lister. Both are framed JSON (see `frame`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable identifier the hub assigns to each connected Lister.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ListerId(pub u64);

/// One directory a Lister keeps in its cache, as listed by the Cache List.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The Lister that holds it.
    pub lister: ListerId,
    /// The cached directory.
    pub path: PathBuf,
}

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
    /// Copy `srcs` into `dest_dir`, giving source `i` the name `new_names[i]`.
    CopyAs { new_names: Vec<String> },
    /// Move `srcs` into `dest_dir`, giving source `i` the name `new_names[i]`.
    MoveAs { new_names: Vec<String> },
    /// Copy each source next to itself as `name.copy`, `name.copy2`, ...
    Duplicate,
    /// Set the permission bits (`mode & 0o777`) of `srcs`; `recurse` also
    /// changes everything below directories.
    Chmod { mode: u32, recurse: bool },
    /// Set the access and modification time of `srcs` to `unix_secs`;
    /// `recurse` also changes everything below directories.
    Touch { unix_secs: i64, recurse: bool },
    /// Create a link in `dest_dir` to every source: symbolic (`symbolic`) or
    /// hard. `new_names[i]`, when given, names the link to source `i`.
    Link { symbolic: bool, new_names: Option<Vec<String>> },
    /// Concatenate `srcs`, in order, into the new file `target` in `dest_dir`.
    Join { target: String },
    /// Split the single source into `chunk_bytes` parts named `name.001`,
    /// `name.002`, ... in `dest_dir`.
    Split { chunk_bytes: u64 },
    /// Pack `srcs` (all in one folder) into the new archive `archive` in
    /// `dest_dir`; the suffix of `archive` (`.zip`, `.tar.gz`, `.7z`, ...)
    /// picks the format.
    Compress { archive: String },
    /// Encrypt `srcs` with `password` into `dest_dir` as `name.enc` (folders
    /// keep their tree).
    Encrypt { password: Secret },
    /// Decrypt `srcs` (`name.enc`) with `password` into `dest_dir`.
    Decrypt { password: Secret },
}

/// A password carried inside a job. It travels as a plain string but never
/// shows up in `Debug` output, so logs and test failures cannot leak it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<hidden>)")
    }
}

/// Modifiers for a job, carried next to its [`Op`]. Everything off (the
/// `Default`) is the plain behaviour, so payloads without options still work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobOptions {
    /// Copy/Move only files whose name already exists at the destination.
    pub update: bool,
    /// Copy/Move only files missing at the destination or newer than it.
    pub newer: bool,
    /// Run a Copy as a Move when source and destination share a device.
    pub move_when_same: bool,
    /// Suppress warning requesters (never conflict prompts).
    pub quiet: bool,
    /// Only files whose name matches this `;`-separated glob list are
    /// affected; directories are always traversed.
    pub include_glob: Option<String>,
}

impl JobOptions {
    /// Whether the job may leave some source files untouched, so a Move
    /// must delete only what it actually copied.
    pub fn is_partial(&self) -> bool {
        self.update || self.newer || self.include_glob.is_some()
    }
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
    RunJob {
        op: Op,
        srcs: Vec<PathBuf>,
        dest_dir: Option<PathBuf>,
        #[serde(default)]
        options: JobOptions,
    },
    /// Ask whether `srcs` fit in the free space at `dest_dir` (`None` = the
    /// DEST Lister's directory). Answered by `ToLister::CheckFitResult` or
    /// `ToLister::CheckFitFailed`.
    CheckFit { srcs: Vec<PathBuf>, dest_dir: Option<PathBuf> },
    /// Ask the hub to open another Lister.
    SpawnLister { start_path: Option<PathBuf> },
    /// Ask the hub to raise another Lister's window. Used by the Tab binding to
    /// jump between the SOURCE and DEST panes.
    PresentLister(ListerId),
    /// Answer to a `ToLister::ConflictPrompt`.
    ConflictReply { req_id: u64, resolution: Resolution },
    /// Sent by `opusdm-settings` (not a Lister): replace the hub's desktop
    /// settings. The hub sanitises, persists to
    /// `~/.config/opusdm/desktop.json`, and applies live. No reply.
    SetDesktopSettings(crate::desktop::DesktopSettings),
    /// Sent by `opusdm-settings` (not a Lister): replace the hub's whole
    /// `Settings` (theme, appearance, behavior) atomically. The hub
    /// sanitises the theme's background, persists to settings.json, applies
    /// live to its own desktop, and broadcasts the new `Settings` to every
    /// connected Lister. No reply.
    SetSettings(Settings),
    /// Run a Function's steps, expanding placeholders against this Lister's
    /// (SOURCE) and the current DEST Lister's selection.
    RunFunction { steps: Vec<crate::function::FnStep> },
    /// Run a saved Function by id: the hub loads it from `functions.json`,
    /// validates it and runs it with this Lister as SOURCE. Used by hotkeys
    /// and the user menu; `RunFunction { steps }` stays for unsaved edits.
    RunFunctionById { id: crate::function::FunctionId },
    /// Leave `paths` out on the desktop as permanent icons. Sent by a Lister
    /// (context menu). The hub refuses the whole request when any path is not
    /// absolute or does not exist, and otherwise emits `HubEvent::LeftOutAdd`.
    /// No reply.
    LeaveOut { paths: Vec<PathBuf> },
    /// Reply to `ToLister::QuerySelection`.
    SelectionReply { paths: Vec<PathBuf>, cwd: PathBuf },
    /// Replace the hotkeys, user menu and script events. The hub sanitizes,
    /// persists to `hotkeys.json`/`usermenu.json`/`scripts.json` and
    /// broadcasts `ToLister::BindingsUpdate` to every Lister. No reply.
    SetBindings(crate::bindings::Bindings),
    /// Replace the saved Lister formats. The hub sanitizes them, persists
    /// `formats.json` and broadcasts `ToLister::FormatsUpdate`. No reply.
    SetFormats(crate::format::FormatStore),
    /// Replace the saved Functions. The hub persists `functions.json` and
    /// broadcasts `ToLister::FunctionsUpdate`. No reply.
    SetFunctions(Vec<crate::function::Function>),
    /// The directories this Lister has cached, most recent first. Replaces the
    /// previous report. No reply.
    CacheReport(Vec<PathBuf>),
    /// Ask for every Lister's cached paths; answered with `ToLister::CacheList`.
    QueryCaches,
    /// Ask every Lister to drop its directory cache (`ToLister::FreeCaches`).
    FreeAllCaches,
    /// Ask `lister` to raise its window and go to `path` (`ToLister::GoTo`).
    /// Ignored when there is no such Lister.
    JumpTo { lister: ListerId, path: PathBuf },
    /// Answer to a `ToLister::ConfirmPrompt`: `ok` is true when the user
    /// confirmed.
    ConfirmReply { req_id: u64, ok: bool },
    /// Tell every Lister that `path` changed (`ToLister::InvalidatePath`), for
    /// changes the hub did not make itself (remote transfers). No reply.
    Invalidate(PathBuf),
    /// Ask the hub to open the button bank manager window. No reply.
    OpenBankManager,
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
            // Dark since ThemeMode::Dark is now this app's own default (see
            // `ThemeMode`) - matches `opusdm_ui::chrome_css`'s dark surface
            // colour, so a fresh install's file list agrees with the rest
            // of the window instead of staying white against it.
            dir_bg: "#2b2d31".to_string(),
            dir_fg: "#e3e3e3".to_string(),
            file_bg: "#2b2d31".to_string(),
            file_fg: "#e3e3e3".to_string(),
            selected_bg: "#3584E4".to_string(),
            selected_fg: "#FFFFFF".to_string(),
        }
    }
}

/// Whether GTK should render every window - buttons, entries, dialog chrome
/// included - in the dark or light variant of the active theme. Unlike
/// [`Appearance::theme_window`] (which only paints OpusDM's own accent
/// colours onto the window background), this is a real GTK theme
/// preference (`gtk-application-prefer-dark-theme`), so it themes generic
/// widgets OpusDM never hand-paints in CSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
}

/// Look-and-feel switches that are not part of the theme (icon set, colours,
/// background) - those live in [`crate::theme::Theme`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Appearance {
    /// Append an `ls -F` style type character (`/ * @ = |`) to each name.
    pub type_indicators: bool,
    /// Render the file list in the system monospace font.
    pub monospace_list: bool,
    /// Carry the file colours into the window chrome too. Off by default so a
    /// user who only picked accent colours keeps their system theme.
    pub theme_window: bool,
    /// Dark or light GTK theme variant, applied application-wide.
    pub theme_mode: ThemeMode,
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
    /// Ask for a file pattern before a recursive Copy/Move/Delete and only
    /// touch the matching files.
    pub recursive_filter: bool,
    /// Format string of the Lister status bar. Codes: `%d`/`%f`/`%b` total
    /// dirs/files/bytes, `%D`/`%F`/`%B` the same for the selection, `%s`
    /// selected entries, `%p` current path, `%%` a literal percent sign.
    pub status_format: String,
    /// How many directory listings each Lister keeps for instant revisits.
    /// 0 turns the cache off. Clamped by [`CACHE_SIZE_RANGE`] when used.
    pub cache_size: usize,
}

/// Directory listings kept per Lister until the user says otherwise.
pub const DEFAULT_CACHE_SIZE: usize = 20;

/// Accepted range for [`Behavior::cache_size`].
pub const CACHE_SIZE_RANGE: (usize, usize) = (0, 200);

/// Status bar format used until the user sets one.
pub const DEFAULT_STATUS_FORMAT: &str = "D: [%D/%d]  F: [%F/%f]  B: [%B/%b]";

/// Accepted range for [`Behavior::auto_refresh_debounce_ms`]. Too low and a
/// build storm reloads the list constantly; too high and it feels broken.
pub const AUTO_REFRESH_DEBOUNCE_RANGE_MS: (u64, u64) = (50, 5_000);

impl Default for Behavior {
    fn default() -> Self {
        Self {
            auto_refresh: true,
            auto_refresh_debounce_ms: 250,
            recursive_filter: false,
            status_format: DEFAULT_STATUS_FORMAT.to_string(),
            cache_size: DEFAULT_CACHE_SIZE,
        }
    }
}

impl Behavior {
    /// The cache size to actually use, clamped into [`CACHE_SIZE_RANGE`] so a
    /// hand-edited settings.json cannot ask for an absurd amount of memory.
    pub fn cache_capacity(&self) -> usize {
        let (lo, hi) = CACHE_SIZE_RANGE;
        self.cache_size.clamp(lo, hi)
    }

    /// The debounce to actually use, clamped into
    /// [`AUTO_REFRESH_DEBOUNCE_RANGE_MS`] so a hand-edited settings.json cannot
    /// make the Lister spin or appear frozen.
    pub fn debounce_ms(&self) -> u64 {
        let (lo, hi) = AUTO_REFRESH_DEBOUNCE_RANGE_MS;
        self.auto_refresh_debounce_ms.clamp(lo, hi)
    }
}

/// The [`Settings::look_version`] written by this build. Version 1: the
/// look (colours, appearance except light / dark, fonts, surfaces) comes only
/// from the active theme; older files may hold values edited by hand in the
/// Settings window, which `opusdm-settings` migrates into a theme once.
pub const CURRENT_LOOK_VERSION: u32 = 1;

/// Every user-configurable setting the hub owns, persists to
/// settings.json, and distributes to every Lister. This one struct is the
/// wire format (`Welcome`/`SettingsUpdate`), the on-disk JSON shape, and the
/// hub's in-memory state - one place to extend when a new setting is added,
/// instead of a new IPC field plus a new HubCmd variant each time.
///
/// The look fields (`theme.colors`, `appearance` except `theme_mode`,
/// `fonts`, `surfaces`) are a cache of the active theme, written only by
/// resolving that theme; everything else is a user preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub theme: crate::theme::Theme,
    pub appearance: Appearance,
    pub behavior: Behavior,
    /// Fonts for the file list and the UI. Empty means system default.
    pub fonts: crate::look::Fonts,
    /// Background pictures for the Lister file list and for requesters.
    pub surfaces: crate::look::Surfaces,
    /// Which look model wrote this file: 0 (missing) for files older than
    /// [`CURRENT_LOOK_VERSION`].
    pub look_version: u32,
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
    /// The current hotkeys, user menu and script events.
    #[serde(default)]
    pub bindings: crate::bindings::Bindings,
    /// The saved Lister formats.
    #[serde(default)]
    pub formats: crate::format::FormatStore,
    /// The saved Functions (the hub owns `functions.json`).
    #[serde(default)]
    pub functions: Vec<crate::function::Function>,
}

/// Messages the hub sends to a Lister once it is registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToLister {
    /// Reply to `Ping`.
    Pong,
    /// Answer to `ToHub::CheckFit`.
    CheckFitResult { dest_dir: PathBuf, needed: u64, free: u64, fits: bool, percent: u64 },
    /// `ToHub::CheckFit` could not be answered.
    CheckFitFailed { message: String },
    /// Ask this Lister to raise and focus its window.
    Present,
    /// The desktop was hidden (`true`) or revealed (`false`): a Lister that
    /// was on screen minimizes itself, or restores itself if the hub
    /// minimized it.
    SetHidden(bool),
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
    /// Ask this Lister for its current selection and cwd, for Function
    /// argument expansion. Reply with `ToHub::SelectionReply`.
    QuerySelection,
    /// One line of output from a running Function step (internal-command
    /// progress, or external-command stdout/stderr).
    FunctionOutput { step_idx: usize, line: String },
    /// A Function step finished successfully.
    FunctionStepDone { step_idx: usize, exit_code: Option<i32> },
    /// A Function step failed; execution stopped here.
    FunctionFailed { step_idx: usize, error: String },
    /// Every step of a running Function finished successfully.
    FunctionDone,
    /// The bindings changed: the full new `Bindings`, replacing the ones
    /// from `Welcome`.
    BindingsUpdate(crate::bindings::Bindings),
    /// The saved Lister formats changed: the full new store.
    FormatsUpdate(crate::format::FormatStore),
    /// The saved Functions changed: the full new list.
    FunctionsUpdate(Vec<crate::function::Function>),
    /// Snapshot of every Lister's cached directories, in Lister id order.
    CacheList(Vec<CacheEntry>),
    /// Drop the directory cache.
    FreeCaches,
    /// Raise the window and navigate to `path`.
    GoTo { path: PathBuf },
    /// A Function's `Confirm` step asks the user; reply with
    /// `ToHub::ConfirmReply`.
    ConfirmPrompt { req_id: u64, message: String },
    /// A `Run` step with an output window started a program: open the
    /// window `id` titled `title`.
    OutputOpen { id: u64, title: String },
    /// One line of output for window `id`.
    OutputLine { id: u64, line: String },
    /// The program of window `id` ended (`None` = killed by a signal).
    OutputEnd { id: u64, exit: Option<i32> },
    /// A function's `ChangeDir` step: go to `path`, switch to the `details`
    /// or `icons` view when `mode` is given, and select the entry `select`.
    ScanDir { path: PathBuf, select: Option<String>, mode: Option<String> },
    /// A job the requester submitted touches an ftp:// / ftps:// / sftp://
    /// location, which only the Lister (it holds the connections) can run.
    /// `dest_dir` is already resolved by the hub, one message per target.
    RemoteTransfer { op: Op, srcs: Vec<PathBuf>, dest_dir: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{recv, send};
    use std::io::Cursor;

    #[test]
    fn secrets_round_trip_as_plain_strings_but_hide_in_debug() {
        let op = Op::Encrypt { password: Secret("hunter2".into()) };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("hunter2"));
        assert_eq!(serde_json::from_str::<Op>(&json).unwrap(), op);
        assert!(!format!("{op:?}").contains("hunter2"));
    }

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
            options: JobOptions::default(),
        });
        roundtrip_tohub(ToHub::RunJob {
            op: Op::CopyAs { new_names: vec!["x".into()] },
            srcs: vec!["/a".into()],
            dest_dir: None,
            options: JobOptions {
                newer: true,
                include_glob: Some("*.rs".into()),
                ..JobOptions::default()
            },
        });
        roundtrip_tohub(ToHub::RunJob {
            op: Op::Duplicate,
            srcs: vec![],
            dest_dir: None,
            options: JobOptions::default(),
        });
        roundtrip_tohub(ToHub::RunJob {
            op: Op::Move,
            srcs: vec![],
            dest_dir: None,
            options: JobOptions::default(),
        });
        roundtrip_tohub(ToHub::CheckFit { srcs: vec!["/a".into()], dest_dir: None });
        roundtrip_tohub(ToHub::SpawnLister { start_path: Some("/etc".into()) });
        roundtrip_tohub(ToHub::PresentLister(ListerId(5)));
        roundtrip_tohub(ToHub::ConflictReply { req_id: 7, resolution: Resolution::OverwriteAll });
        roundtrip_tohub(ToHub::SetDesktopSettings(crate::desktop::DesktopSettings::default()));
        roundtrip_tohub(ToHub::SetSettings(Settings::default()));
        roundtrip_tohub(ToHub::RunFunction {
            steps: vec![crate::function::FnStep::External {
                argv: vec!["notify-send".into(), "done".into()],
                cwd_source: true,
            }],
        });
        roundtrip_tohub(ToHub::SelectionReply { paths: vec!["/a".into()], cwd: "/home/x".into() });
        roundtrip_tohub(ToHub::RunFunctionById { id: "fn-1".into() });
        roundtrip_tohub(ToHub::SetFunctions(vec![crate::function::Function::new("f")]));
        roundtrip_tohub(ToHub::SetBindings(crate::bindings::Bindings {
            hotkeys: vec![crate::bindings::Hotkey { key: "Ctrl+u".into(), function: "f".into(), global: true }],
            ..crate::bindings::Bindings::default()
        }));
        roundtrip_tohub(ToHub::SetFormats(crate::format::FormatStore::default()));
        roundtrip_tohub(ToHub::CacheReport(vec!["/a".into()]));
        roundtrip_tohub(ToHub::QueryCaches);
        roundtrip_tohub(ToHub::FreeAllCaches);
        roundtrip_tohub(ToHub::OpenBankManager);
        roundtrip_tohub(ToHub::JumpTo { lister: ListerId(2), path: "/p".into() });
        roundtrip_tohub(ToHub::Invalidate("sftp://h/a".into()));
        roundtrip_tohub(ToHub::ConfirmReply { req_id: 3, ok: true });
    }

    #[test]
    fn set_desktop_settings_carries_the_value() {
        let ds = crate::desktop::DesktopSettings {
            icon_size: 120,
            ..crate::desktop::DesktopSettings::default()
        };
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
            bindings: crate::bindings::Bindings::default(),
            formats: crate::format::FormatStore::default(),
            functions: vec![crate::function::Function::new("Copy and notify")],
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
        roundtrip_tolister(ToLister::QuerySelection);
        roundtrip_tolister(ToLister::CheckFitResult {
            dest_dir: "/d".into(),
            needed: 2,
            free: 1,
            fits: false,
            percent: 50,
        });
        roundtrip_tolister(ToLister::CheckFitFailed { message: "no DEST".into() });
        roundtrip_tolister(ToLister::FunctionOutput { step_idx: 0, line: "hello".into() });
        roundtrip_tolister(ToLister::FunctionStepDone { step_idx: 0, exit_code: Some(0) });
        roundtrip_tolister(ToLister::FunctionFailed { step_idx: 1, error: "boom".into() });
        roundtrip_tolister(ToLister::FunctionDone);
        roundtrip_tolister(ToLister::BindingsUpdate(crate::bindings::Bindings::default()));
        roundtrip_tolister(ToLister::OutputOpen { id: 7, title: "ls -l".into() });
        roundtrip_tolister(ToLister::OutputLine { id: 7, line: "x".into() });
        roundtrip_tolister(ToLister::OutputEnd { id: 7, exit: Some(0) });
        roundtrip_tolister(ToLister::ScanDir { path: "/p".into(), select: Some("f".into()), mode: Some("icons".into()) });
        roundtrip_tolister(ToLister::ConfirmPrompt { req_id: 3, message: "Sure?".into() });
        roundtrip_tolister(ToLister::RemoteTransfer { op: Op::Copy, srcs: vec!["sftp://h/a".into()], dest_dir: "/d".into() });
        roundtrip_tolister(ToLister::FormatsUpdate(crate::format::FormatStore::default()));
        roundtrip_tolister(ToLister::FunctionsUpdate(vec![crate::function::Function::new("f")]));
        roundtrip_tolister(ToLister::CacheList(vec![CacheEntry { lister: ListerId(1), path: "/p".into() }]));
        roundtrip_tolister(ToLister::FreeCaches);
        roundtrip_tolister(ToLister::GoTo { path: "/p".into() });
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
    fn a_behavior_block_without_status_format_gets_the_default_one() {
        let b: Behavior = serde_json::from_str(r#"{"auto_refresh":false}"#).unwrap();
        assert!(!b.auto_refresh);
        assert_eq!(b.status_format, DEFAULT_STATUS_FORMAT);
    }

    #[test]
    fn the_directory_cache_holds_twenty_listings_by_default() {
        assert_eq!(Behavior::default().cache_size, 20);
        assert_eq!(Behavior::default().cache_capacity(), 20);
    }

    #[test]
    fn a_hand_edited_cache_size_is_clamped() {
        let at = |n| Behavior { cache_size: n, ..Behavior::default() }.cache_capacity();
        assert_eq!(at(0), 0, "0 turns the cache off");
        assert_eq!(at(50), 50);
        assert_eq!(at(1_000_000), CACHE_SIZE_RANGE.1);
    }

    #[test]
    fn a_behavior_block_without_cache_size_gets_the_default() {
        let b: Behavior = serde_json::from_str(r#"{"auto_refresh":false}"#).unwrap();
        assert_eq!(b.cache_size, 20);
    }

    #[test]
    fn auto_refresh_is_on_by_default() {
        assert!(Behavior::default().auto_refresh);
    }

    #[test]
    fn file_and_dir_row_colours_default_to_dark_since_dark_is_the_app_default() {
        let c = ColorSettings::default();
        assert_eq!(c.dir_bg, "#2b2d31");
        assert_eq!(c.file_bg, "#2b2d31");
        assert_eq!(c.dir_fg, "#e3e3e3");
        assert_eq!(c.file_fg, "#e3e3e3");
    }

    #[test]
    fn appearance_defaults_to_dark_theme_mode() {
        assert_eq!(Appearance::default().theme_mode, ThemeMode::Dark);
    }

    #[test]
    fn theme_mode_survives_a_json_round_trip() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<ThemeMode>(&json).unwrap(), mode);
        }
    }

    #[test]
    fn an_appearance_json_missing_theme_mode_defaults_to_dark() {
        let a: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(a.theme_mode, ThemeMode::Dark);
    }

    #[test]
    fn a_hand_edited_debounce_is_clamped_to_something_usable() {
        let (lo, hi) = AUTO_REFRESH_DEBOUNCE_RANGE_MS;
        let at = |ms| Behavior { auto_refresh: true, auto_refresh_debounce_ms: ms, ..Behavior::default() }.debounce_ms();
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
    fn a_settings_file_from_before_fonts_and_surfaces_loads() {
        let s: Settings = serde_json::from_str(r#"{"appearance":{"monospace_list":true}}"#).unwrap();
        assert!(s.appearance.monospace_list);
        assert_eq!(s.fonts, crate::look::Fonts::default());
        assert_eq!(s.surfaces, crate::look::Surfaces::default());
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
                theme_mode: ThemeMode::Light,
            },
            behavior: Behavior {
                auto_refresh: false,
                auto_refresh_debounce_ms: 900,
                recursive_filter: true,
                status_format: "%f files, %b".to_string(),
                cache_size: 7,
            },
            fonts: crate::look::Fonts {
                list: crate::look::FontSpec { family: "Iosevka".into(), size: 12 },
                ui: crate::look::FontSpec::default(),
            },
            surfaces: crate::look::Surfaces {
                lister: crate::look::SurfaceImage {
                    image_path: Some("/tmp/l.png".into()),
                    fit: crate::desktop::ImageFit::Tile,
                },
                requester: crate::look::SurfaceImage::default(),
            },
            look_version: CURRENT_LOOK_VERSION,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
    }

    #[test]
    fn run_job_without_options_still_parses() {
        let json = r#"{"RunJob":{"op":"Copy","srcs":["/a"],"dest_dir":null}}"#;
        let msg: ToHub = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ToHub::RunJob {
                op: Op::Copy,
                srcs: vec!["/a".into()],
                dest_dir: None,
                options: JobOptions::default(),
            }
        );
    }

    #[test]
    fn settings_without_recursive_filter_default_to_off() {
        let s: Settings = serde_json::from_str(r#"{"behavior":{"auto_refresh":false}}"#).unwrap();
        assert!(!s.behavior.recursive_filter);
    }

    #[test]
    fn partial_options_are_detected() {
        assert!(!JobOptions::default().is_partial());
        assert!(JobOptions { update: true, ..JobOptions::default() }.is_partial());
        assert!(JobOptions { include_glob: Some("*".into()), ..JobOptions::default() }.is_partial());
        assert!(!JobOptions { quiet: true, ..JobOptions::default() }.is_partial());
    }

    #[test]
    fn leave_out_roundtrips() {
        roundtrip_tohub(ToHub::LeaveOut { paths: vec!["/etc/fstab".into(), "/home/x".into()] });
    }

    #[test]
    fn set_hidden_roundtrips() {
        roundtrip_tolister(ToLister::SetHidden(true));
        roundtrip_tolister(ToLister::SetHidden(false));
    }
}
