# Applets design

Date: 2026-10-26. Status: approved by the owner.

## Intent

The owner wants small tools, **applets**, that OpusDM loads and places on the
desktop, in the spirit of KDE Plasma widgets. The first examples:

- **site-news**: shows the titles of the latest news of the owner's own site
  (the site exposes RSS), each title clickable.
- **weather-clock**: a graphically rich clock with the weather forecast.

The point is the system, not these two applets. New applets must be easy to
write by the owner: a folder with a manifest and a JavaScript file, no
recompilation of OpusDM.

OpusDM is production software and the owner's daily desktop (see `CLAUDE.md`).
An applet must never be able to break the Lister, the hub or the global
hotkeys. That drives the main architecture choice below.

## Goals

- An applet is a folder; adding one is copying a folder.
- Applets are written in JavaScript on the Boa engine OpusDM already embeds
  (`crates/hub/src/scripting.rs`), with the same `opus` object as scripts.
- Applets can build GTK interfaces and draw on a canvas.
- Applets can go online. Network access is added to the shared `opus` core, so
  the Lister scripts get it too.
- Applets are placed on the desktop by the user: Alt + mouse drag, or a drag
  handle in "edit applets" mode.
- Each applet has a settings UI, generated from its manifest.
- A failing applet says so (its window and the log). Nothing fails silently.

## Non-goals

- No applet store, download, packaging or update mechanism.
- No enforced sandbox or permission prompts for now (see Permissions).
- No applets on panels, docks or the lock screen: desktop only.
- No new scripting language, no WebView, no HTML/CSS rendering.
- **X11 only in v1** (decided by the owner). Wayland (labwc, wayfire) is out of
  scope for now; the applet windows and Alt-drag are built for X11 and must
  not break the existing Wayland code paths.
- **No bridge to the Listers in v1** (decided by the owner). Applets get the
  shared core (`opus.http`, files, notifications, opening a link), timers, UI
  and canvas. `opus.command`, `opus.listers`, `opus.runFunction` and the other
  calls that need the hub come later (phase 5).

## Anti-goals (a failure even if it technically works)

- A looping or crashing applet freezes or kills the hub, the Lister or the
  global hotkeys.
- An applet window that stays blank with an empty log.
- Applet state mixed into the applet folder, or the applet folder rewritten by
  OpusDM.
- Handles and gears visible or clickable outside "edit applets" mode.

## Decisions taken by the owner

1. **Layout.** `~/.config/opusdm/applets/<id>/`, where `<id>` is the folder
   name (`weather-clock`, `site-news`). The folder holds `applet.json`
   (manifest), `main.js` and the resources (images, fonts, dials, in
   `assets/` or wherever the applet wants). The manifest has no `id` field:
   the folder name is the id.
2. **State.** Settings, position and size are saved outside the applet folder,
   in the existing state directory (`opusdm_ipc::settings::state_dir()`, that
   is `~/.local/state/opusdm`), one file per applet:
   `~/.local/state/opusdm/applets/<id>.json`. (The owner first said
   `~/.local/share/opusdm`; the existing temporary values live in
   `state_dir()` and `share` only holds VPN data. The owner was told about
   the difference and accepted `state_dir()`.)
3. **Language and API.** JavaScript on Boa. `opus` is the same object as in
   scripts, plus `opus.applet` for the window, the settings, timers and events.
4. **Network.** Provided by the shared `opus` core (for example `opus.http`),
   not by the applet layer, so Lister scripts have it too.
5. **UI.** GTK interfaces and a canvas.
6. **Moving.** Alt + mouse drag on the applet moves it. In "edit applets" mode
   every applet also shows a pair of small icons in its top right corner, a
   drag handle and a settings gear, and a dashed border around the applet
   (variant A of the mockups). Outside the mode the icons are not drawn and
   not clickable.
7. **Entering the mode.** "Edit Applets" is the **last** entry of the desktop
   right-click menu. It starts a separate application, `opusdm-applet-browser`,
   which lists the available applets and has the switch for the edit mode.
   `opusdm-settings`, tab Advanced, gets an "Edit Applets" button that does the
   same thing.

## Decisions taken by the builder of this design (my calls, to check)

- **Applets run in a separate process from the hub** (`opusdm-applets`, name to
  settle): one process, one Boa `Context` per applet, GTK on its own main
  loop. Why: the hub owns the desktop, the Listers' coordination and the global
  hotkeys; the incident in `CLAUDE.md` came from touching ownership in the hub.
  A buggy applet (infinite loop, panic) can then only take down the applets,
  not the session. The cost is one more process and an IPC path for `opus.*`
  calls that need the hub (commands, listers, functions). A freeze still stops
  all applets together; per-applet watchdog or one process per applet are
  possible later.
- **Why not the existing script runner:** a script runs once, synchronously, on
  the Function executor thread. An applet lives for hours, needs timers and
  events, and must draw on GTK, which is main-thread only. Boa's `Context` is
  not `Send`, so it cannot move to the GTK thread either.
- **Network must not block the UI:** for applets the HTTP call is asynchronous
  (promise or callback); a synchronous variant is fine for Lister scripts, which
  already block by design.
- **Canvas:** a Canvas 2D style API (the one JavaScript developers already
  know: paths, fills, text, images, transforms) implemented over cairo in a
  `DrawingArea`. Redraw on request and on a timer, not continuously.
- **GTK from JavaScript:** a chosen subset of widgets (box, label, button,
  list, image, entry, switch, drawing area), not the whole of GTK.
- **Permissions:** the manifest declares `net`, `fs`, `exec`, but they are not
  enforced and nothing prompts, because for now the owner writes every applet.
  The field exists so enforcement can be added without changing the format.
- **Manifest fields** (draft): `name`, `version`, `description`, `entry`
  (default `main.js`); window: default size, minimum size, resizable,
  transparent background; `settings`: list of `{ key, type, label, default }`
  with types such as string, number, bool, choice; `permissions`.
- **Errors:** a manifest that does not parse, a script exception or a failed
  request is logged with `err_log!` and shown inside the applet's window.
- **Who runs the applet process (my call, a lifetime change):** the hub starts
  it at startup, like the other startup apps, and logs with `err_log!` when it
  exits. A bounded number of restarts (for example 3 in a minute), then it
  stays down and the log says so. The hub does not own the applets, only the
  process. The commit that adds this must answer, in its body, "who owns this
  object for the whole run, and what happens when it is gone?".
- **Enabled or disabled (my call):** an `enabled` flag in the applet's own
  state file `~/.local/state/opusdm/applets/<id>.json`. A new folder with no
  state file is disabled until switched on in the Applet Browser.
- **Live changes (my call):** a settings change is pushed to the running applet
  (an `opus.applet.onSettings` callback). A change to `main.js` is picked up by
  a Reload action in the Applet Browser, with no file watch in v1. Enable and
  disable work without restarting the process; timers of a stopped applet are
  cancelled.
- **A frozen applet (my call):** in v1 a looping applet may freeze the other
  applets (one process, one GTK loop) but never the hub. Callbacks run under
  the same VM cycle budget technique `scripting.rs` already uses
  (`evaluate_async_with_budget`), so a runaway callback is cut and logged where
  Boa allows it. One process per applet is the fallback if this proves not
  enough.
- **Weather source (my call):** `weather-clock` uses Open-Meteo (HTTPS, JSON,
  no API key). The HTTP client crate and the TLS choice for `opus.http` are the
  builder's, with HTTPS required and the dependency kept small. Settings of
  string type are stored in plain text in the state file, so a future applet
  that needs a key stores it there; this is accepted for now.
- **Talking to the applet process (my call, to settle in phase 2):** the
  Applet Browser, `opusdm-settings` and the hub reach the applet process
  through a small local channel of its own (the hub hands out the flag and the
  enable/disable commands), not through the Listers' protocol. Whether that is
  a new message in `opusdm-ipc` or a separate socket is the builder's choice
  after reading `opusdm-ipc`.
- **Phases**, each testable on the running session before the next:
  1. `opus.http` and shared core changes, usable from Lister scripts.
  2. The applet process: manifest loading, Boa per applet, timers, widgets,
     canvas, state files.
  3. Desktop integration: window placement, Alt-drag, edit mode (dashed border,
     handle, gear), per-applet settings UI.
  4. `opusdm-applet-browser`, the right-click entry, the Settings button.
  5. Later, after the two sample applets work: the bridge to the hub
     (`opus.command`, `opus.listers`, `opus.runFunction`), a risk-managed
     phase of its own under `CLAUDE.md` rule 4.
- **Stacking on X11 (my call, to try):** applets sit above the wallpaper and
  above the icon field, so they stay visible and clickable. Clicks on a
  transparent area should reach the desktop below if X11 allows it (input
  shape); if not, the transparent applets accept clicks on their whole window
  in v1. The owner did not choose; change it if it looks wrong.

## Constraints

- All of `CLAUDE.md` applies: behaviour first, ownership changes explained in
  the commit body, no silent early return (log with `err_log!`), a regression
  test for each wiring or lifetime change, one risky change per commit, no push.
- After touching hub, ipc, ui or the panel/dock, run the real thing and test:
  Lister (Enter in the path bar, double click on a folder, Backspace / parent,
  Alt+Left/Right, F-keys), global hotkeys (Ctrl+backslash, Ctrl+Shift+space,
  Ctrl+Print, Alt+1 / Alt+2), hub (second Lister, SOURCE/DEST, a Copy job, a
  function run). What cannot be checked is written in `CHANGES.md`.
- Follow the pattern of the other standalone tools (`opusdm-bluetooth-manager`,
  `opusdm-network-manager`, `opusdm-printer-manager`) and of how the desktop
  menu starts `opusdm-settings` (binary next to the hub first, then `PATH`).
- Text and code in English, `-` instead of the long dash.

## Risks and things to verify early

- **Alt + drag vs the window manager.** `openbox-rc.xml` binds `A-Left` +
  drag to `Move` in the `Frame` context. It is unknown whether applet windows
  get a frame. Check on the real session (Openbox) and in the sandbox; Wayland
  compositors (labwc, wayfire) are configured differently.
- **Applet windows vs the desktop windows.** The hub builds one background
  window per monitor (pinned geometry on X11) and the icon field on the
  primary one. How the applet windows are typed and stacked on X11 and how a
  click goes through an applet's transparent areas (see the stacking call
  above) must be tried on the real session.
- **Multi-monitor and resolution changes:** the hub re-pins its desktop windows
  on a resolution change; applets need the same.
- **The IPC for `opus.*` from the applet process:** how much of the current
  script operations (`command`, `listers`, `runFunction`, `fs.*`) can go through
  the existing `opusdm-ipc` channel with no hub changes beyond a new message.
- **Hot reload:** whether editing `main.js` reloads the applet, and whether an
  applet can be enabled and disabled from the browser without restarting the
  process.

## Left to the builder

- Name and packaging of the applet process and of `opusdm-applet-browser`
  (crates, binaries, `.deb`, `scripts/live-update.sh`).
- The exact JavaScript API (`opus.applet`, widgets, canvas, `opus.http`).
- How the edit-mode flag travels from the browser/settings to the applet
  process, and who owns it.
- The settings UI layout, the applet browser layout, the visual style of the
  edit mode (follow the mockup, variant A).
- Sample applets: `site-news` and `weather-clock` as the first two, also to
  prove the API.
