# OpusDM Panel - design

Status: approved 2026-09-09. Turns opusdm-hub into a KDE Plasma-style panel:
a separate top-docked bar with an application menu, a virtual-desktop pager,
a per-desktop window list, and CPU / network / clock widgets on the right.

## Goals

- A real panel window: `_NET_WM_WINDOW_TYPE_DOCK` with `_NET_WM_STRUT_PARTIAL`,
  so maximised windows do not draw under it. Top edge. Primary monitor only.
- Application menu: installed `.desktop` entries grouped by freedesktop
  category as submenus, with themed icons; plus Settings / About / Logout /
  Shutdown actions.
- Virtual-desktop pager: one numbered button per desktop (openbox ships 4),
  current highlighted, click switches, tooltip is the desktop name.
- Window list for the current desktop only: icon + title, click activates,
  click on the active window iconifies it.
- Right side: CPU usage (text + sparkline), network usage (down/up split,
  text + sparkline), two-line clock (large time, small date).

## Non-goals

- Multi-monitor panels.
- Configurable panel position/size (fixed: top, 40 px).
- Keeping SRC/DST indicators, job-progress UI, "New Lister" and "Quit" in the
  panel - they are removed from the hub chrome. New Lister / Quit stay on the
  desktop right-click context menu. Job progress still reaches the requesting
  Lister via `ToLister::JobProgress`; only desktop-drop jobs lose their
  progress bar (acceptable).

## Architecture

New crate `crates/panel`, library `opusdm_panel`. Independent of the hub
`server.rs`: the panel needs only X11/EWMH, `/proc`, `.desktop` files and the
ability to spawn processes.

```rust
pub struct PanelHooks {
    /// Opens the hub Settings dialog (kept in the hub, coupled to HubCmd).
    pub on_settings: Box<dyn Fn()>,
}
pub fn attach(app: &gtk::Application, hooks: PanelHooks) -> gtk::ApplicationWindow;
```

The hub, after building the desktop window, calls `opusdm_panel::attach`. Both
windows live on the same `gtk::Application`, same process, no new IPC. About
and Logout/Shutdown live inside the panel crate (self-contained). Only
Settings calls back into the hub.

### Modules

| file | responsibility |
|---|---|
| `lib.rs` | `attach()`, window assembly, event wiring |
| `window.rs` | DOCK window + `_NET_WM_STRUT_PARTIAL` via x11rb (mirrors `hub/src/desktop.rs`); `PANEL_HEIGHT` |
| `ewmh.rs` | x11rb observer thread, `EwmhEvent`, `ClientWin`; action helpers; pure parsers |
| `appmenu.rs` | `.desktop` discovery + parse, category grouping, popover, launch; Settings/About/Logout/Shutdown items |
| `widgets/pager.rs` | numbered desktop buttons |
| `widgets/tasklist.rs` | current-desktop window list |
| `widgets/sysmon.rs` | `/proc/stat` + `/proc/net/dev` sampling, sparkline drawing |
| `widgets/clock.rs` | two-line clock |

### New dependencies

`gtk4`, `x11rb`, `gdk4-x11`, `opusdm-ui` (for `requester`), and a freedesktop
desktop-entry parser crate (exact crate/version chosen at implementation time;
candidate `freedesktop-desktop-entry`). No `chrono` - use `glib::DateTime`.

### Bar layout

Horizontal `gtk::Box`. Start: `[Menu v] [Pager 1..N] [Tasklist -> hexpand]`.
End: `[CPU] [Net] [Clock]`.

## Panel window and struts (`window.rs`)

`gtk::ApplicationWindow`, `decorated(false)`, `resizable(false)`, width = primary
monitor width, height `PANEL_HEIGHT` (40), position `0,0`.

On `realize` and `map`, via x11rb on the surface xid (same pattern as
`hub/src/desktop.rs`):

- `_NET_WM_WINDOW_TYPE` = `_NET_WM_WINDOW_TYPE_DOCK`
- `_NET_WM_STATE` = `_NET_WM_STATE_ABOVE`
- `_NET_WM_STRUT` and `_NET_WM_STRUT_PARTIAL`: `top = PANEL_HEIGHT`,
  `top_start_x = 0`, `top_end_x = mon_w - 1`, rest 0
- `WmSizeHints` position/size `UserSpecified`, min = max = `(mon_w, PANEL_HEIGHT)`,
  then `configure_window` to `(0, 0, mon_w, PANEL_HEIGHT)`

The desktop window (type `DESKTOP`) is unaffected by struts, so the background
still covers the whole screen; the panel draws over its top 40 px, where the
old toolbar strip no longer exists.

Pure function: `strut_partial(height: i32, mon_w: i32) -> [u32; 12]`.

## EWMH observer (`ewmh.rs`)

A thread owns its own `x11rb::RustConnection`, does
`change_window_attributes(root, PropertyChangeMask | SubstructureNotifyMask)`
and loops on `wait_for_event()`. On `PropertyNotify` for a watched atom it
re-reads the property and sends an `EwmhEvent` on an `async-channel::Sender`;
the GTK side drains it with `glib::spawn_future_local` (same bridge as
`server.rs`).

Watched root properties:

- `_NET_NUMBER_OF_DESKTOPS`, `_NET_CURRENT_DESKTOP`, `_NET_DESKTOP_NAMES`
- `_NET_CLIENT_LIST` - on change, re-scan each client window: `_NET_WM_NAME`
  (fallback `WM_NAME`), `_NET_WM_DESKTOP`, `_NET_WM_STATE` (for
  `_NET_WM_STATE_HIDDEN` = minimised), `_NET_WM_WINDOW_TYPE` (skip
  `DOCK`/`DESKTOP`), `_NET_WM_ICON`. Select `PropertyChangeMask` on each new
  client window to catch renames and minimise/restore.
- `_NET_ACTIVE_WINDOW`

X errors on windows that vanished meanwhile are ignored.

```rust
enum EwmhEvent {
    Desktops { count: u32, names: Vec<String>, current: u32 },
    CurrentDesktop(u32),
    Clients(Vec<ClientWin>),
    ActiveWindow(Option<u32>),
}
struct ClientWin { xid: u32, title: String, desktop: i64, minimized: bool, icon: Option<IconRgba> }
struct IconRgba { width: i32, height: i32, argb: Vec<u8> }
```

Actions (called from the GTK thread) use a second `RustConnection` held by the
GTK side, sending `ClientMessage` with `SubstructureRedirect|SubstructureNotify`:

- switch desktop -> `_NET_CURRENT_DESKTOP`
- activate window -> `_NET_ACTIVE_WINDOW` (source indication 2)
- iconify -> `WM_CHANGE_STATE` with `IconicState` (ICCCM; what openbox honours
  for pager-style iconify)

Pure, testable parsers: `parse_desktop_names(&[u8]) -> Vec<String>` (NUL-separated
UTF-8), `parse_window_list(&[u8]) -> Vec<u32>`, `pick_icon(&[u32]) -> Option<IconRgba>`
(best size from `_NET_WM_ICON`).

## Application menu (`appmenu.rs`)

- Discovery: `$XDG_DATA_HOME` (default `~/.local/share`) + `$XDG_DATA_DIRS`
  (default `/usr/local/share:/usr/share`), each `+ /applications`, recursed for
  `*.desktop`. First match by desktop-file id wins (earlier dir shadows later).
- Parse with the freedesktop crate. Keep entries where `Type=Application`,
  not `NoDisplay=true`, not `Hidden=true`. `OnlyShowIn`/`NotShowIn` ignored
  (no meaningful `XDG_CURRENT_DESKTOP` under openbox).
- Group by the first recognised main category into a fixed bucket set:
  Development, Graphics, Internet, Multimedia, Office, Games, System, Settings,
  Utility; unknown -> Other. Buckets in that fixed order, apps sorted by name
  (case-insensitive) within.
- UI: the menu button opens a `gtk::Popover` with a scrollable vertical box.
  One `gtk::Expander` per non-empty bucket; inside, one flat button per app
  with `gtk::Image::from_icon_name(icon)` (or `from_file` for an absolute
  `Icon` path) + left-aligned label. Below a separator, always visible:
  Settings, About, Logout, Shutdown (icon + label buttons).
- Launch: `gio::AppInfo` created from the desktop file, `launch()`. Errors
  logged, not fatal.
- About: `opusdm_ui::requester_rich`, text copied from the current hub About.
- Logout: confirm via `opusdm_ui::requester`, then `openbox --exit`.
- Shutdown: confirm, then `systemctl poweroff`.

## Pager (`widgets/pager.rs`)

Horizontal `gtk::Box` of N `gtk::Button` labelled `1..=N`. The current
desktop's button carries a `.active` CSS class. Click -> `ewmh::switch_desktop(i)`.
Rebuilt on `_NET_NUMBER_OF_DESKTOPS` change; active restyled on
`_NET_CURRENT_DESKTOP`. Tooltip = desktop name from `_NET_DESKTOP_NAMES`.

## Window list (`widgets/tasklist.rs`)

Horizontal `gtk::Box`, `hexpand`. For each `ClientWin` whose `desktop` equals
the current desktop or is `-1` (omnipresent, `0xFFFFFFFF`): a `gtk::Button`
with `gtk::Image` (themed icon; from `_NET_WM_ICON` argb -> `gdk::MemoryTexture`
when present, else a themed icon by `WM_CLASS`, else `application-x-executable`)
and an ellipsised `gtk::Label` (max ~20 chars). Active window -> `.active`
class; minimised -> `.dim-label` + italics. Click: if this window is active and
not minimised -> `ewmh::iconify_window(xid)`; else -> `ewmh::activate_window(xid)`.
Rebuilt on client-list or current-desktop change; restyled on active-window or
per-window state change.

## System monitors (`widgets/sysmon.rs`)

- CPU: parse `/proc/stat` `cpu` line; percent = busy delta / total delta vs the
  previous sample. 1 s `glib::timeout_add_local`.
- Network: parse `/proc/net/dev`; sum rx and tx bytes over all interfaces
  except `lo`; per-second down/up = delta / interval. Human-format (B/K/M/G per
  s).
- Sparkline: ring buffer of the last ~60 samples. A `gtk::DrawingArea`
  (~44 px wide) draws a polyline scaled to the window max. CPU: one trace.
  Net: two traces (down, up) in two shades.
- Per widget: compact `[sparkline][text]` box. CPU text `CPU 23%`. Net text two
  lines `v 1.2M/s` / `^ 340K/s`.

Pure functions: `cpu_percent(prev: &CpuTimes, now: &CpuTimes) -> f64`,
`parse_proc_stat(&str) -> CpuTimes`, `parse_net_dev(&str) -> (u64 rx, u64 tx)`,
`human_rate(bytes_per_s: f64) -> String`.

## Clock (`widgets/clock.rs`)

Vertical `gtk::Box`: bold `%H:%M` label (~16 px) over a small date label
(`glib::DateTime::now_local().format(...)`, locale-aware). A `glib::timeout`
(5 s is enough at minute granularity) sets both labels.

## Changes to the existing hub

`crates/hub/src/toolbar.rs`:

- Drop the top `bar` (New Lister, SRC/DST labels, Settings/About/Quit buttons,
  spacer) and `job_box` from `build_ui`. The root box now holds only the
  full-window desktop area.
- Remove `Widgets` fields `src_label`, `dst_label`, `src_id`, `dst_id`,
  `job_box`, `jobs`, and the `JobRow` struct.
- `apply_event` keeps `RaiseToolbar` and `DesktopSettingsChanged`; `Roles`,
  `JobUpdate`, `JobFinished` become ignored arms. The server keeps emitting
  them - harmless.
- Remove `attach_present_gesture`. Keep `show_about_dialog` for the desktop
  context menu (or delete it and point the context-menu About at the panel's;
  simplest: keep both, they are tiny).
- Keep `show_settings_dialog` and its helpers in the hub; export enough for the
  panel hook. The desktop context menu keeps New Lister / Settings / About /
  Desktop submenu / Quit unchanged.
- Update the module doc comment. Filename left as `toolbar.rs` to limit churn.

`run()` / `build_ui`: after `window.present()` for the desktop window, call
`opusdm_panel::attach(app, PanelHooks { on_settings })`, where `on_settings`
captures `cmd_tx` and a parent `gtk::Window` and calls `show_settings_dialog`.

`crates/hub/Cargo.toml`: add `opusdm-panel = { path = "../panel" }`.

Workspace `Cargo.toml`: add `crates/panel` to `members`; add the desktop-entry
parser crate to `[workspace.dependencies]`.

## Testing

- `ewmh.rs`: `parse_desktop_names`, `parse_window_list`, `pick_icon` over
  sample byte buffers. No X server needed.
- `appmenu.rs`: `.desktop` parse + category mapping over sample entry strings
  (`NoDisplay` filtered, category -> bucket); discovery over a tempdir of fake
  `.desktop` files with dir shadowing.
- `sysmon.rs`: `parse_proc_stat`, `cpu_percent`, `parse_net_dev`, `human_rate`
  over sample text.
- `window.rs`: `strut_partial` tuple values.
- X11 integration (observer thread, actions, DOCK hints) has no automated test
  - it needs a running server - mirroring the untested X code in
  `hub/src/desktop.rs`. Manual check via `run-x11.sh`.
- `CHANGES.md` entry.

## Implementation order

1. Crate skeleton + workspace wiring; `attach` returns an empty DOCK bar with
   correct struts. Verify visually with `run-x11.sh`.
2. Clock widget (self-contained, no X).
3. sysmon widget (pure parsers first, TDD).
4. EWMH observer + pager.
5. Window list.
6. Application menu.
7. Gut the hub toolbar strip; wire `on_settings`.
8. CHANGES.md, final manual pass.
