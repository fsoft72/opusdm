# Dock as a separate process - design

Date: 2026-11-08. Status: approved in chat by the owner.

## Goals

1. **Freeze isolation**: a stuck hub main loop (see `FREEZE.md`) must not
   freeze the dock, and the dock must not be able to freeze the hub.
2. **Independent restart / deploy**: the dock can be killed, restarted or
   updated without touching the hub.
3. **No memory when unused**: with the dock turned off, no dock process runs.
4. **Dock settings live in the dock**: the Dock tab leaves `opusdm-settings`
   and becomes a window of the dock, opened from the dock background menu
   ("Add Application...", "Settings...").

Trade-off accepted: with the dock on, a separate GTK4 process costs more
memory than the embedded dock (tens of MB of RSS).

## Data: `dock.json` is the single source of the dock

- `opusdm_ipc::dock::DockConfig` grows from `{ apps }` to
  `{ apps, position, icon_size, visible, style }`, all `serde(default)`.
  `layout_migrated: bool` marks a file that already took the layout fields.
- The `dock_*` fields leave `DesktopSettings`; old keys in `desktop.json` are
  ignored when read.
- **One-time migration** `opusdm_ipc::dock::migrate_from_desktop()`: when
  `dock.json` has no layout yet, take the raw `dock_*` keys of `desktop.json`
  (read as JSON, not through the struct) and save. Idempotent. Called by the
  hub at startup (before deciding whether to start the dock) and by the dock
  at startup (standalone runs).
- **Write rule**: nobody keeps a long-lived copy of the file. Every write
  goes through `update_dock_config(|cfg| ...)`: read fresh, change only its
  own fields, save atomically. Writers: the dock (drag and drop, Settings
  window), the "Show the dock" check box of `opusdm-settings`, the hub desktop
  menu item.
- The dock still reads `desktop.json`, only for the panel edge.

## Process `opusdm-dock`

`crates/dock/src/main.rs` becomes the real entry point:

1. Run the migration, load `dock.json`. `visible == false`: exit at once with
   `EXIT_DISABLED` (new, in `opusdm_ipc::instance`, next to
   `EXIT_ALREADY_RUNNING`), before building any window.
2. Own the single-instance socket `dock.sock` (next to the hub socket,
   override `OPUSDM_DOCK_SOCK`), same scheme as `applets/instance.rs` with a
   protocol probe. A live owner: exit with `EXIT_ALREADY_RUNNING`. A bind
   failure is logged and the dock starts anyway, without Hide All.
3. Apply the theme mode from `settings.json` (`opusdm_ui::apply_theme_mode`),
   build the dock with `attach`.
4. Watch with `gio::FileMonitor`:
   - `dock.json`: apps, layout, style; `visible` becoming false logs and
     exits with `EXIT_DISABLED`.
   - `desktop.json`: panel edge.
   - `settings.json`: theme mode.

### Settings window inside the dock

- `crates/desktop-config/src/dock_ui.rs` moves to the dock crate
  (`settings_window.rs`) with its three sections: edge and icon size, look
  (with "Show the dock"), pinned apps. Cancel and Apply as in today's tab.
- Background menu: "Add Application...", "Settings...". An open window is
  raised instead of opening a second one.
- Apply with "Show the dock" off saves and the dock exits.

## Protocol (`opusdm_ipc::dock`)

Length-prefixed JSON frames (`opusdm_ipc::frame`), one `Reply` per `Request`.

```rust
pub const SOCK_FILE_NAME: &str = "dock.sock";
pub const ENV_SOCK: &str = "OPUSDM_DOCK_SOCK";
pub enum DockRequest { Ping, SetHidden(bool) }
pub enum DockReply { Hidden(bool), Error(String) }
pub fn socket_path() -> PathBuf;
pub fn request_at(path: &Path, req: &DockRequest) -> io::Result<DockReply>; // 1 s timeouts
```

- Dock side: thread `opusdm-dock-ipc` accepts, reads, forwards to the GTK
  main loop through an `async_channel` with a reply sender; the main loop
  applies `set_hidden` + `set_visible(is_shown())` and answers
  `Hidden(state)`. The client times out after 1 s.
- The dock always starts not hidden and never asks the hub anything.

## Hub: `dock_host.rs`

- The supervisor loop of `applet_host.rs` becomes generic (tool name and
  thread name as parameters) and gains the `EXIT_DISABLED` state.
- Thread `opusdm-dock-supervisor` owns the `Child` for the whole run:

| Exit | Action |
|---|---|
| crash / signal | restart with backoff, max 3 in 60 s, then `err_log!` and stop |
| `EXIT_ALREADY_RUNNING` | retry every 30 s |
| `EXIT_DISABLED` | park on a wake channel (no polling) |
| binary missing | `err_log!`, stop |

- Wake: the GTK side of the hub keeps a `gio::FileMonitor` on `dock.json`
  that only sends a wake to the supervisor. The supervisor thread reads
  `dock.json` (off the main loop) and starts the dock when `visible`.
- Hide All: `apply_hidden` calls `dock_host::set_hidden(hidden)`, which stores
  `DOCK_HIDDEN: AtomicBool` and wakes thread `opusdm-dock-link`; the link
  sends `SetHidden(current)`. The hub main loop never does socket I/O to the
  dock. Failure: `err_log!` when the dock should be alive, `debug_log!` when
  it is turned off.
- Startup: after a spawn the supervisor polls `Ping` every 100 ms for up to
  5 s, then asks the link to push the current hidden state (a dock restarted
  during Hide All comes back hidden). No answer in 5 s: `err_log!`, the
  process stays supervised.
- Desktop menu: "Show Dock" / "Hide Dock" writes `visible` through the
  `io_worker`.
- Removed from the hub: the `opusdm-dock` dependency, `attach`,
  `set_dock_layout`, `DesktopChanges::dock_layout`, `opusdm_dock::set_hidden`
  and `is_shown`.

Ownership: the supervisor thread owns the child; the link thread owns its
receiver and reads the shared atomic. When the hub exits, both threads die
and the dock keeps running; a new hub sees `EXIT_ALREADY_RUNNING` and waits.
If the supervisor gives up it says so in the log; the dock stays absent until
the hub restarts or the setting is toggled.

## opusdm-settings

- The Dock tab is removed; a "Show the dock" check box stays in the Desktop
  tab and writes `visible` with `update_dock_config`.
- `desktop-config` and `theme-config` no longer depend on `opusdm-dock`.

## Errors (no silent failure)

- Dock: unreadable `dock.json` -> `err_log!`, defaults, file not overwritten;
  unreadable `desktop.json` -> `err_log!`, panel at top; socket bind failure
  -> `err_log!`, dock runs without Hide All; IPC accept / recv errors logged,
  loop continues; closed GTK channel -> `err_log!`, loop ends; exit for
  `visible: false` -> `debug_log!`.
- Hub: every dock exit logged with code / signal; give up, parking and wake
  logged; failed `FileMonitor` -> `err_log!`; failed `SetHidden` logged as
  above; failed migration -> `err_log!`, defaults, `desktop.json` untouched.

## Tests

- `opusdm_ipc::dock`: new config roundtrip; old `{apps}` file reads with
  defaults; migration takes `dock_*`, is idempotent, does not overwrite a
  migrated file; `update_dock_config` keeps fields it does not touch;
  protocol roundtrip.
- Supervisor (pure logic): `EXIT_DISABLED` parks; wake with visible restarts,
  without stays parked; crash backoff and give up; already running retry;
  missing binary stops.
- Dock: socket probe / acquire; server answers `Hidden(x)` after
  `SetHidden(x)`; handler alive after N requests; `visible=false` exits with
  `EXIT_DISABLED` without windows.
- Settings: moved `dock_ui` tests; the check box writes only `visible`.

## Live verification (CLAUDE.md rule 4)

Sandbox `run-x11.sh`, then live session (`live-update.sh --all`):

1. Separate `opusdm-dock` process; launch, drag and drop, Add Application,
   Settings (edge, icon size, opacity, zoom; Apply and Cancel).
2. Hide All / Reveal, also after killing the dock during Hide All.
3. Turn off from the dock window (process gone, `pgrep`); turn on from
   Settings and from the desktop menu; `kill -9` -> restarted.
4. Hub restart with the dock alive: no second dock, Hide All still works.
5. `kill -STOP` of the hub: the dock keeps working.
6. Struts / snap work area, dock on every virtual desktop.
7. Rule 4 checklist (Lister, global hotkeys, hub roles and jobs).
8. Wayland (`run-wayland.sh`) if possible; otherwise noted in `CHANGES.md`.

## Rollout (one risky change per commit)

1. `opusdm_ipc::dock`: config, migration, `update_dock_config`, protocol.
2. Standalone dock complete (socket, watches, theme mode, `EXIT_DISABLED`);
   the hub still embeds it.
3. Settings window in the dock, menu item.
4. Switch: the hub stops embedding the dock, `dock_host` with supervisor and
   link. Ownership answer in the commit body.
5. Settings tab removed, check box in Desktop, desktop menu item.
6. Cleanup and docs (`FEATURES.md`, `OpusDM-Features-Guide.md`, `CHANGES.md`).
