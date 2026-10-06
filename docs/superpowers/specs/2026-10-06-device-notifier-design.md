# Device notifier (removable drives in the panel)

Date: 2026-10-06

## Goal

Something like the KDE Device Notifier. When an external disk or a USB
stick is plugged in, OpusDM shows a popup that offers to mount it. A
panel icon lists the removable devices at any time, with mount, unmount
and eject. Mounted devices appear under `/media/<user>/<label>`, with no
password and no `sudo`.

Scope decided with the user:

- Notification: a **panel icon with a drop-down** (KDE tray style) **plus a
  popup** when a new device arrives.
- Position: the **right side of the panel, before the CPU / network
  monitor** (`widgets::sysmon`).
- Actions: mount, mount and open in the Lister, unmount, eject (power off
  when the drive supports it).
- Out of scope: automount, formatting, SMART data, optical disc burning,
  MTP phones (they are not udisks2 block devices), Wayland specifics.

## Why not `gio::VolumeMonitor`

The first idea was `gio::VolumeMonitor` in a separate binary. GVfs is not
installed on the target system (only `udisks2`), and without the GVfs
volume monitor GIO only sees `/proc/mounts`, not a disk that was just
plugged in. The panel already depends on `zbus` (`sni.rs`, `dbusmenu.rs`),
so the client talks to udisks2 directly and adds no new dependency.

## Structure

- New crate `crates/devices` (`opusdm-devices`), **library only, no GTK**.
- New panel widget `crates/panel/src/widgets/devices.rs`, and a popup
  window in the same panel crate (`crates/panel/src/device_popup.rs`).
- No new binary: the panel lives inside the hub process.

### `opusdm-devices` modules

- `model`: `Device { object_path, label, size, dev_node, fs_type, mount_point: Option<PathBuf>, kind (Usb | Disk | Card), can_eject, can_power_off, drive_name }` and `DeviceEvent { Added(Device), Removed(path), Changed(Device) }`. Pure data, serde not needed.
- `filter`: `is_listed(&RawBlock) -> bool`, pure. A block device is listed when udisks2 has a `Filesystem` interface, `HintIgnore` is false, `HintSystem` is false, it is not a loop device, and it is removable or on a USB-like bus (`Drive.Removable`, `Drive.ConnectionBus == "usb"`, or `Drive.CanPowerOff`). The root, `/boot` and the swap are never listed.
- `parse`: converts the udisks2 `GetManagedObjects` dictionary (`a{oa{sa{sv}}}`) into `RawBlock` and then `Device`. Label falls back to the model name, then to the size (`"32 GB Volume"`), as KDE does. Mount points arrive as NUL terminated byte arrays.
- `client`: `Client` over `zbus::blocking`, with `list()`, `mount(path)`, `unmount(path)`, `eject(path)` and `watch(sender)`. `watch` runs `InterfacesAdded`, `InterfacesRemoved` and `PropertiesChanged` handlers on a thread and sends `DeviceEvent`s through a `std::sync::mpsc` channel (the same pattern as `sni.rs`). The system bus is a separate connection from the session bus used by `sni`.
- `ops`: the three actions as pure request builders plus the call. `Mount` uses `Filesystem.Mount` with empty options (udisks2 chooses `/media/<user>/<label>`), `Unmount` uses `Filesystem.Unmount`, `Eject` unmounts first, then `Drive.Eject` and, if `CanPowerOff`, `Drive.PowerOff`.
- `backend`: trait `Backend` (`list`, `mount`, `unmount`, `eject`) implemented by `UdisksBackend` and a `FakeBackend` for tests, so the widget logic never needs a bus.
- `error`: `DeviceError` with the udisks2 error name and message (for example `NotAuthorized`, `DeviceBusy`), mapped to a short readable text.

### Panel widget

`widgets::devices::build(on_open)` returns a `gtk::MenuButton` with a
drive icon (cairo, like the other panel icons, so it does not depend on
the icon theme) and a `gtk::Popover` holding one row per device:

- Icon by kind, label, size, and the mount point when mounted.
- Not mounted: a **Mount** button.
- Mounted: **Open** (calls `on_open(mount_point)`), **Unmount**, and **Eject** when `can_eject`.
- Busy rows (an operation is running) are disabled and show a spinner.

The button is **hidden while the list is empty**. A badge with the count
is not needed.

Placement in `lib.rs`: in `right`, between the tray block and
`widgets::sysmon::build()`, with a separator after it. Only the primary
bar gets it, as the tray does (a device must not appear twice).

Operations run on a worker thread and report back through
`async_channel`, so the GTK main loop never blocks on D-Bus.

### Popup

`device_popup::show(device, actions)` opens a small undecorated window at
the corner of the primary monitor (the corner nearest the panel) with the
device label, size, and the buttons **Mount**, **Mount and open** and
**Ignore**. It closes by itself after 10 seconds, and when the device is
removed or mounted from the drop-down. A second device while one popup is
open replaces it (the drop-down still lists both). A device already
mounted when the popup would open (an old session, or another tool
mounted it first) shows no popup.

Devices that are present at start-up do not trigger a popup, only the
ones that arrive afterwards (`Added` after the initial `list()`).

### Opening in the Lister

The panel does not know how to start a Lister (that lives in
`hub/children.rs`). `opusdm_panel::attach` gains an `on_open_folder:
Rc<dyn Fn(&Path)>` argument that the hub supplies
(`children::spawn_lister(sock, Some(path))`). The widget and the popup
call it for **Open** and **Mount and open**.

## Error handling

- No system bus or udisks2 not running: the widget stays hidden and one
  line is logged (`eprintln!` as the rest of the panel does). It retries
  every 30 seconds, so starting `udisks2` later works.
- A failed action (busy, not authorized, device gone) is shown as a
  one line message at the top of the drop-down for 5 seconds. Nothing is
  swallowed silently and nothing panics.
- Unmount of a busy device reports the udisks2 message and leaves the
  device listed and mounted.
- A device that disappears mid-operation (unplugged) is removed from the
  list by the `Removed` event, and the failed call is ignored.

## Testing

- `parse`: real `GetManagedObjects` dumps (a USB stick with a label, one
  without a label, a card, an internal NVMe, a loop device) converted
  to `Device`s.
- `filter`: every listing rule, including the root disk never listed.
- `ops`: mount, unmount and eject request building, and the eject
  sequence (unmount, eject, power off) with the `FakeBackend`.
- `error`: mapping of udisks2 error names to messages.
- Panel: the row model (which buttons a device gets, the empty state,
  event handling: added, removed, changed, busy) is a pure struct tested
  without GTK, as `pager.rs` and `sysmon.rs` do.
- By hand under the real session (Xephyr cannot plug in a disk): plug a
  USB stick, popup, Mount, Open, Unmount, Eject, and unplug while
  mounted. Whatever is not checked by hand will be stated in
  `CHANGES.md`.

## Files touched

- `Cargo.toml`: add `crates/devices` to the workspace.
- `crates/devices/**`: new library.
- `crates/panel/Cargo.toml`: depend on `opusdm-devices`.
- `crates/panel/src/widgets/{mod,devices}.rs`, `crates/panel/src/device_popup.rs`, `crates/panel/src/lib.rs`.
- `crates/hub/src/toolbar.rs`: pass the open folder callback to `attach`.
- `docs/usage.md`: new section; `CHANGES.md`.
