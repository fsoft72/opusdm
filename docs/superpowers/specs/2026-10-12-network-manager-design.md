# Network manager - design

Date: 2026-10-12.

## Goal

A program to configure network connections, like the "Connections" module of
KDE Plasma System Settings: `opusdm-network-manager`. It lists every
connection profile of NetworkManager and edits Wi-Fi, Ethernet, WireGuard and
OpenVPN profiles, with their IPv4 and IPv6 settings. WireGuard `.conf` and
OpenVPN `.ovpn` files can be imported. The panel network tray gets a VPN
section and a button that opens the program.

Out of scope: 802.1X (enterprise), bridge, bond, VLAN, hotspot, proxy, other
VPN plugins (OpenConnect, PPTP, ...). Profiles of those kinds are listed
read only and can be deleted.

## Layout

### `crates/network` (`opusdm-network`)

New modules, no GTK:

- `profile`: the typed model of one profile.
  - `Profile { uuid, id, kind, autoconnect, autoconnect_priority, ipv4, ipv6,
    vpn_on_connect (connection.secondaries), detail }`.
  - `ProfileKind`: `Wifi`, `Ethernet`, `WireGuard`, `OpenVpn`,
    `Other(type)`.
  - `IpConfig { method, addresses, gateway, dns, ignore_auto_dns,
    never_default }`; `IpMethod`: `Auto`, `Manual`, `LinkLocal`, `Disabled`
    (IPv6 `ignore` is shown as `Disabled`).
  - `WifiDetail`, `EthernetDetail`, `WireGuardDetail` (with `Peer`s) and
    `OpenVpnDetail`.
  - `Profile::from_settings` reads the `a{sa{sv}}` map;
    `Profile::to_settings(base)` writes the managed keys over `base`, the map
    that was read, so keys this program does not know are kept. The legacy
    `addresses` and `dns` keys are dropped when the new ones are written,
    because NetworkManager prefers `addresses` over `address-data`.
  - `Profile::validate` returns the errors by field.
  - `Profile::secrets` lists the secrets with their keyring keys.
- `ovpn`, `wgconf`: pure parsers of the import files. They return the
  profile, the directives that were ignored and, for `.ovpn`, the inline
  files (`<ca>`, `<cert>`, `<key>`, `<tls-auth>`, `<tls-crypt>`) to write.
- `wgkey`: WireGuard key generation (`x25519-dalek`, `getrandom`, `base64`).
- `profiles`: trait `ProfileBackend` (`list`, `load`, `add`, `update`,
  `delete`, `activate`, `deactivate`) and `FakeProfiles` for tests. The real
  one is `NmProfiles` in `nm_profiles`.
- `keystore`: keys are found by `(uuid, setting, key)`, with the attributes
  `nm-applet` uses (`connection-uuid`, `setting-name`, `setting-key`), so the
  Wi-Fi keys already kept stay readable. `delete` forgets every key of a
  profile.
- `agent`: answers for `802-11-wireless-security` (`psk`), `vpn` (the
  `secrets` dictionary of OpenVPN) and `wireguard` (`private-key` and the
  `preshared-key` of each peer). A one-shot store in memory is asked first:
  it holds the password of a VPN that asks every time, for one activation.
  A VPN key that NetworkManager refuses is not forgotten (it is edited in the
  program).

### `crates/network-config`, binary `opusdm-network-manager`

GTK4, the layout of `input-config`. A `gtk::Application` with id
`org.opusdm.NetworkManager`, so a second launch raises the window.

- Left: a search entry and the profiles grouped by kind (Ethernet, Wi-Fi,
  VPN, Other); a mark shows the active ones. Below: `+` (asks the kind),
  `-` (asks to confirm; forgets the keys and the imported files too) and
  `Import...`.
- Right: the name and the tabs General, the kind tab (Wi-Fi, Ethernet,
  WireGuard or OpenVPN), IPv4, IPv6. OpenVPN rare options are in an
  "Advanced..." dialog. Buttons Connect / Disconnect, Revert and Save. Save is
  enabled only with changes and no validation error; a field with an error
  shows it in its tooltip. Leaving a changed profile asks Save, Discard or
  Cancel.
- An import opens the editor on a profile that is not saved yet, with a note
  that lists the ignored directives.
- A worker thread runs the backend calls; the window never waits for the bus.
  The `watch` worker refreshes the list when profiles or active connections
  change. The program registers its own secret agent (identifier
  `org.opusdm.network.manager`) so a connection started from it works without
  the panel.

## Secrets

Secrets are agent owned (flag 1) and kept in the `login` keyring. The program
writes them to the keyring itself and sends the profile to NetworkManager
without them. An OpenVPN password can be "ask every time" (flag 2): the tray
or the program asks for it before connecting and gives it to its agent's
one-shot store; it is never kept.

Imported certificates and keys go to `~/.local/share/opusdm/vpn/<uuid>/`,
folder 0700 and files 0600; deleting the profile removes the folder.

Secrets are never logged, never passed as process arguments and never shown
by `Debug`.

## Panel

- The popover gets a VPN section when VPN profiles exist: one row per
  profile with a switch. A VPN that asks every time opens the password dialog
  first.
- The icon gets a small lock when a VPN is active; the tooltip adds
  "VPN: <name>".
- A "Configure connections..." button at the bottom launches
  `opusdm-network-manager`.

## Errors

Every failed call shows the NetworkManager text in the message line of the
program or of the popover. An import that fails says the line and the
directive.

## Tests

Profile round trips (unknown keys kept, legacy keys dropped), validation,
the `.ovpn` and `.conf` parsers on sample files, key generation against a
known vector, keyring attributes, agent answers for VPN and WireGuard and the
one-shot store, the VPN rows and the icon logic of the panel. By hand under
Xephyr with this machine's NetworkManager.

## Docs

`CHANGES.md`, a section in `docs/usage.md`.
