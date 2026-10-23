# Bluetooth support - design

Date: 2026-10-23

## Intent

The owner uses OpusDM as the daily desktop and connects Bluetooth headphones,
keyboard, mouse, phone and other devices. Today there is no way to do this
from OpusDM: the owner wants what KDE Plasma 6 offers, in two places:

- a **Bluetooth icon in the tray** of the panel (the panel runs inside
  `opusdm-hub`), to see the devices, connect / disconnect them, and turn
  Bluetooth on and off;
- a **Bluetooth page in `opusdm-settings`** (Settings > Advanced, next to
  Network and Printers), to pair new devices and manage the paired ones.

## Goals

1. Turn the adapter on / off from the tray and from Settings.
2. List the paired devices with their state (connected or not, battery level
   when the device reports it) and connect / disconnect them with one click
   from the tray.
3. Pair a new device from Settings: scan, show the devices found, pair,
   trust, connect. Handle every pairing method BlueZ can ask for: PIN code
   entry, passkey entry, passkey display (keyboard: "type 123456 on the
   keyboard and press Enter"), numeric confirmation (phone: "does the code
   123456 match?"), authorize, authorize service.
4. Accept a pairing started by the other side (phone pairing to the PC) at
   any time, also when Settings is closed.
5. Manage a paired device in Settings: details (name, alias, address, type,
   icon, paired, trusted, blocked, connected, battery), rename (alias), trust
   / untrust, block / unblock, connect / disconnect, remove (forget).
6. Adapter options in Settings: alias (the name other devices see),
   discoverable on / off (with timeout as BlueZ allows), power on / off.
7. Desktop notifications: a device connected, a device disconnected, a device
   battery is low, a pairing failed.

## Non-goals (for now)

- Audio profile switching for headphones (A2DP vs headset / hands-free). The
  owner said to leave headphones-specific work out. Headphones still pair and
  connect like any other device; the audio routing stays to PipeWire /
  PulseAudio as today.
- File transfer with the phone (OBEX send / receive). Explicitly postponed.
- Network tethering (PAN / DUN), MIDI, LE Audio specifics.
- Supporting more than one adapter in the UI beyond "use the first one /
  the default one" (a second adapter may be listed but is not a goal).

## Anti-goals

- Any regression in the hub, the panel or the global hotkeys (see the
  project `CLAUDE.md`). The Bluetooth code must never block the GTK main loop
  and must never take the panel down when BlueZ is missing, stopped,
  restarted, or the adapter is unplugged / rfkill-blocked.
- Silent failures: every failed D-Bus call, failed `Weak::upgrade()`, closed
  channel or ended watcher logs with `err_log!` (rule 3 of `CLAUDE.md`).
- Shelling out to `bluetoothctl`. Talk to BlueZ over D-Bus.
- Leaving a pairing request unanswered: if no dialog can be shown, the agent
  rejects the request (and logs) instead of letting BlueZ time out.

## Constraints

- Linux, BlueZ 5 over the system D-Bus (`org.bluez`). BlueZ is active on the
  owner's machine with one adapter `hci0`.
- Follow the structure already used by the network support:
  - a library crate `crates/bluetooth` (`opusdm-bluetooth`), the
    counterpart of `crates/network`: BlueZ model, watcher thread
    (`ObjectManager` `InterfacesAdded` / `InterfacesRemoved` and
    `PropertiesChanged`), requests, pairing agent, errors. It uses `zbus`
    (already a dependency of `crates/network`), `async-channel`, no GTK.
  - a config crate `crates/bluetooth-config` (`opusdm-bluetooth-config`), the
    counterpart of `crates/network-config`: a GTK tool with an `embedded()`
    entry point returning `Rc<dyn EmbeddedTool>`, plus a standalone binary
    `opusdm-bluetooth-manager` like `opusdm-network-manager`.
  - in `crates/panel`: `bluetooth_center.rs` + `bluetooth_model.rs` +
    `widgets/bluetooth.rs`, owned by `attach()` and kept in the rebuild
    closure exactly like `NetworkCenter` and `PrinterCenter`.
  - in `crates/theme-config/src/advanced.rs`: a "Bluetooth" entry.
  - build / live-update scripts updated for the new binary, as done for
    Printers in commit `110adf9`.
- Pure logic (model, diffing, icon state, notification decisions, agent
  request routing) is kept out of GTK and unit tested, as in
  `network_model.rs`.
- One risky change per commit (rule 6): library, then Settings page, then
  panel icon, then agent, each with the app run in between.

## Decided parts of the how

- **Tray icon**: hidden when there is no adapter; a "Bluetooth off" icon when
  the adapter is powered off; a "Bluetooth on" icon when on; a "connected"
  variant when at least one device is connected. Click opens a popup in the
  Plasma 6 style:
  - top: the on / off switch;
  - the paired devices, connected first, each with icon, name, battery (if
    known) and a Connect / Disconnect action;
  - bottom: "Add device..." and "Configure..." which start the standalone
    `opusdm-bluetooth-manager` (the same tool as the Settings page), with
    `--pair` for "Add device..." so it opens straight into the pairing flow.
    *Builder's call, after the review:* the network and printer icons start
    their standalone programs too, and `opusdm-settings` cannot be opened on
    a chosen Advanced page today; changing its startup is not worth the risk.
  - Tooltip: on / off and the names of the connected devices.
- **Settings page**: left the list of paired devices, right the details of
  the selected one with its actions; a header row with the adapter switch,
  the adapter name and "Discoverable"; an "Add device..." button that opens
  the pairing flow (scan list updating live, pick one, pair, then trust and
  connect automatically). Scanning stops when the flow closes or the page is
  left.
- **Pairing agent ownership**:
  - the panel (inside the hub, alive for the whole session) registers an
    `org.bluez.Agent1` with capability `KeyboardDisplay` and asks to be the
    **default agent**, so pairings started by the other side (the phone) are
    always answered. Its dialogs are shown by the panel.
  - Settings registers its own agent for the pairings it starts (BlueZ sends
    the requests of a `Pair()` call to the agent of the caller's
    connection), so the PIN / passkey dialog appears in the Settings window.
  - The agent runs on a worker thread; requests are forwarded to the GTK
    thread through a channel and answered when the user replies or the dialog
    closes (closing = reject).
- **Who owns what for the whole run** (to be repeated in the commit bodies,
  rule 2): `attach()` owns the `BluetoothCenter`; the watcher loop and the
  agent loop hold it strongly as `NetworkCenter`'s loops do; buttons are
  strong and pruned when their widget leaves the bar. When BlueZ goes away
  the watcher logs, the icon hides, and the watcher reconnects when
  `org.bluez` appears again on the bus (`NameOwnerChanged`) and the agent
  registers again.
- **Pairable** (*builder's call, after the review*): the adapter is never
  left pairable for good. "Discoverable" in Settings turns on `Discoverable`
  and `Pairable` together for 3 minutes (`DiscoverableTimeout` and
  `PairableTimeout` = 180 s), so a phone can pair to the PC in that window;
  the pairing flow sets `Pairable` while it is open. When the adapter is not
  pairable BlueZ refuses incoming pairings before asking the agent, so the
  panel agent only shows dialogs the user invited.
- **rfkill**: if the adapter is soft-blocked, turning Bluetooth on clears
  the soft block first, by writing `/dev/rfkill` (writable by the owner's
  user, checked on 2026-10-23) (BlueZ `Powered=true` fails with `org.bluez.Error.Blocked`
  otherwise). A hard block is shown as "Blocked by hardware switch" and the
  switch is disabled.

## Left to the builder

- Exact module split inside the crates, and the names of types.
- If writing `/dev/rfkill` fails, show the error and log it.
- Icons: pick from the icon theme already used by the panel
  (`bluetooth-active`, `bluetooth-disabled`, `bluetooth-paired` or what the
  theme provides), with a fallback.
- Battery low threshold for the notification (suggested 20 %, notify once
  per device per crossing).
- Whether the standalone `opusdm-bluetooth-manager` is also reachable from
  the Run dialog / app menu.
- Tests: unit tests for model and agent routing; a regression test or a
  startup self-check that the `BluetoothCenter` is still alive after the
  first update (rule 5). Verification in the sandbox (`run-x11.sh`) and in
  the live session, with real devices, before writing "done"; anything not
  exercised goes into `CHANGES.md` as not verified.
