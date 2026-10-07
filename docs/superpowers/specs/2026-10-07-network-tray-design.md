# Network tray - design

Date: 2026-10-07. Branch: `network`.

## Goal

A button in the panel tray (primary bar, next to the audio tray) that opens a
popover like the KDE network applet: switches to enable or disable Wi-Fi and
Ethernet, and the list of the Wi-Fi networks in range. A click on a network
connects to it, asking for the password when the network is protected and has
never been saved.

Out of scope: VPN, hotspot, IP configuration, hidden networks, 802.1X
(enterprise) networks.

## Backend: NetworkManager over D-Bus

New library crate `crates/network` (`opusdm-network`), no GTK, only `zbus`
(blocking) and `async-channel`, both already used by `opusdm-devices`. It
follows the layout of `devices` and `audio`.

Alternatives rejected: `nmcli` as a subprocess (polling, locale dependent
output, password visible in the process arguments) and a mix of both.

Modules:

- `model`: `NetState` (`wifi_present`, `wifi_enabled`, `eth_present`,
  `eth_available` = cable or carrier, `eth_on` = connected or connecting,
  `eth_connected`, `aps`) and `AccessPoint` (`ssid`, `signal` 0 to 100,
  `security`, `active`, `saved`). `Security` is `Open`, `Psk`, `Sae` or
  `Enterprise`.
- `parse`: pure functions. `RawAp` is read from the `GetAll` properties of an
  access point; `security(flags, wpa, rsn)`; `build_aps` drops hidden SSIDs,
  merges duplicates (the best signal wins, the active one always wins), marks
  saved networks and sorts: active first, then by signal.
- `error`: `NetError` with the D-Bus name and message and a one line `text()`.
- `backend`: trait `Backend` (`state`, `set_wifi_enabled`, `set_eth_enabled`,
  `scan`, `connect`, `disconnect_wifi`) and `FakeBackend` for tests.
- `nm`: the real backend.
- `watch`: a worker thread that coalesces NetworkManager signals for 300 ms,
  re-reads the state and sends it only when it changed. Without the system bus
  or NetworkManager it reports once and retries every 30 s.

### NetworkManager calls

- State: `GetDevices`, then `DeviceType` (1 Ethernet, 2 Wi-Fi) and `State`
  (20 unavailable, 30 disconnected, 40 to 90 activating, 100 activated).
  Access points from `GetAllAccessPoints` of the Wi-Fi device, the active one
  from `ActiveAccessPoint`. Saved networks from `Settings.ListConnections` and
  `GetSettings` (`802-11-wireless.ssid`).
- Wi-Fi switch: the `WirelessEnabled` property of the manager.
- Ethernet switch: off is `Device.Disconnect` (NetworkManager then does not
  autoconnect that device again until a manual activation); on is
  `ActivateConnection("/", device, "/")`, which lets NetworkManager pick the
  best saved connection.
- Connect to a saved network: `ActivateConnection(profile, device, ap)`.
  To a new open network: `AddAndActivateConnection` with an empty settings
  map (NetworkManager fills it from the access point). To a new protected
  network: the same call with `802-11-wireless-security` (`key-mgmt` `wpa-psk`
  or `sae`, and `psk`).
- The connect call waits (on the worker thread) for the active connection to
  reach `activated`, up to 30 s. When it ends `deactivated` the new profile is
  deleted, so a wrong password is not kept, and the error is "Could not
  connect, wrong password?".
- `RequestScan` is sent when the popover opens.

## Panel

- `network_model.rs`: pure logic: the request type and its coalescing (the
  last switch value wins), the icon state (`Ethernet`, `Wifi(level 0..3)`,
  `Disconnected`, `Off`) and the tooltip, the row texts.
- `network_center.rs`: like `AudioCenter`: shared state owned by `attach`, so a
  monitor-hotplug rebuild keeps it; a worker thread runs the requests so the
  GTK thread never waits; failures come back as a message line shown for 5 s.
- `widgets/network.rs`: the cairo icon (Wi-Fi arcs or a plug), the popover
  with the two switches and the list. A row has the name, a lock when the
  network is protected and the signal; the active row has a check. The button
  is hidden while NetworkManager is not reachable. The Wi-Fi section is hidden
  without a Wi-Fi device, the Ethernet one without an Ethernet device; the
  Ethernet switch is insensitive without a cable.
- Password dialog: a modal `gtk::Window` with a hidden entry and a show
  toggle. Connect sends the request; the dialog closes and an error appears in
  the popover message line.

## Security

The password lives only in the request, in the D-Bus message to
NetworkManager, and in the dialog entry. It is never logged and never passed
as a process argument. Rust strings are not wiped from memory on drop; this
is accepted, the same as every other GTK entry.

## Errors

Nothing is swallowed: every failed request shows its text in the popover. A
network that vanished while connecting is not an error line (it is already gone
from the list).

## Tests

Parser on built property maps (security flags, hidden SSID, duplicates,
active, saved, ordering), error texts, request coalescing, icon and tooltip
logic, the connect sequence on `FakeBackend`, burst coalescing in the watcher.
Checked by hand under Xephyr with this machine's NetworkManager: icon, popover,
list of networks, Wi-Fi switch, scan. Not checked: switching Ethernet off (it
would cut this machine's connection), a real password connection, enterprise
networks.

## Docs

`CHANGES.md`, a section in `docs/usage.md`.
