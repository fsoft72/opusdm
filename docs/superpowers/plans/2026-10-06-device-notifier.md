# Device Notifier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A panel icon with a drop-down of removable devices (mount, unmount, eject, open) plus a popup when a new device is plugged in.

**Architecture:** A GTK-free library crate `opusdm-devices` talks to udisks2 over the system bus with `zbus` (blocking) and exposes a pure model, filter, parser, diff and a `Backend` trait with a fake for tests. The panel owns one `DeviceCenter` (shared device list, worker thread, listeners) so a monitor-hotplug rebuild never loses the state; the primary bar gets a `gtk::MenuButton` from it, placed before `sysmon`. A popup window shows new devices.

**Tech Stack:** Rust 2021, `zbus` 5.19 (blocking), `gtk4` 0.10, `async-channel`, udisks2 (`org.freedesktop.UDisks2`).

**Spec:** `docs/superpowers/specs/2026-10-06-device-notifier-design.md`

## Global Constraints

- Library crate `crates/devices`, name `opusdm-devices`, no GTK, no new dependency (`zbus` is already in the lockfile).
- Icon on the right side of the panel, before `widgets::sysmon::build()`, primary bar only.
- Mounts go through udisks2 (`/media/<user>/<label>`): never `mount`, never `sudo`.
- No em dash anywhere; English code and comments; every function has a doc comment; internal exit conditions on top as one liners; constants in UPPER_CASE; text files end with an empty line.
- Never block the GTK main loop on D-Bus: operations run on a worker thread and report through `async_channel`.
- Commit after every task, no AI attribution lines, never `--no-verify`.
- Encrypted (LUKS) volumes are not unlocked by this feature (out of scope); an already unlocked `dm-*` volume flagged `HintSystem` is not listed.

## Review Focus

- A drive with no label and no model must still get a readable name (size based).
- The system disks (root, `/boot`, swap, loop devices, NVMe internal) must never show up, even though some are on a "removable looking" bus.
- Several partitions on one stick: each is a row, and **Eject** is offered once per drive, not per partition.
- Unplug while mounted: the row disappears and no error pops up for the call that fails afterwards.
- udisks2 not running at start: no crash, the button stays hidden, and it appears when the service shows up.

---

### Task 1: Crate skeleton, model, filter, parser

**Files:**
- Create: `crates/devices/Cargo.toml`, `crates/devices/src/lib.rs`, `crates/devices/src/model.rs`, `crates/devices/src/filter.rs`, `crates/devices/src/parse.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces:
  - `model::Kind { Usb, Disk, Card }`
  - `model::Device { path: String, drive_path: String, label: String, size: u64, dev_node: String, fs_type: String, mount_point: Option<PathBuf>, kind: Kind, can_eject: bool, can_power_off: bool, drive_name: String }`
  - `model::human_size(u64) -> String` ("32 GB", "1.5 TB")
  - `filter::RawBlock { path, dev_node, id_label, id_type, size, hint_system, hint_ignore, mount_points: Option<Vec<String>>, drive: Option<RawDrive> }`, `filter::RawDrive { path, vendor, model, bus, removable, ejectable, can_power_off, media }`
  - `filter::is_listed(&RawBlock) -> bool`
  - `parse::ManagedObjects` = `HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>`, `parse::devices(&ManagedObjects) -> Vec<Device>` (sorted by `dev_node`)

- [ ] **Step 1: Write the failing tests** in `filter.rs` (`#[cfg(test)]`): a USB stick with a filesystem is listed; `hint_system` is not; `hint_ignore` is not; no `Filesystem` (`mount_points == None`) is not; a block with no drive is not; an internal NVMe (`bus == ""`, `removable == false`) is not; a removable SD card (`removable == true`, bus `""`) is listed. In `model.rs`: `human_size(32_000_000_000) == "32 GB"`, `human_size(1_500_000_000_000) == "1.5 TB"`, `human_size(0) == "0 B"`. In `parse.rs`: a hand built `ManagedObjects` with one USB partition (label `SEAGATE`, mounted at `/media/fabio/SEAGATE` as NUL terminated bytes) and one drive gives one `Device` with `label == "SEAGATE"`, `mount_point == Some("/media/fabio/SEAGATE")`, `kind == Kind::Usb`, `can_power_off == true`; a partition with empty `IdLabel` gets label `"Seagate Expansion HDD"` (vendor + model); with empty vendor and model it gets `"16 TB Volume"`.
- [ ] **Step 2:** `cargo test -p opusdm-devices` fails (crate missing).
- [ ] **Step 3: Implement.** `Cargo.toml`: `zbus = { version = "5.19.0", features = ["blocking"] }` plus `async-channel` not needed here. `parse::devices` reads, per object path containing `/block_devices/`, the interfaces `org.freedesktop.UDisks2.Block` (`Device` is `ay` NUL terminated, `IdLabel`, `IdType`, `Size`, `HintSystem`, `HintIgnore`, `Drive` as object path, `"/"` means none) and `org.freedesktop.UDisks2.Filesystem` (`MountPoints` is `aay`), then looks the drive up in the same map (`org.freedesktop.UDisks2.Drive`: `Vendor`, `Model`, `ConnectionBus`, `Removable`, `Ejectable`, `CanPowerOff`, `Media`). Kind: bus `usb` is `Usb`; `Media` starting `flash_sd` or `flash_mmc` is `Card`; otherwise `Disk`. Label: `IdLabel`, else `"{vendor} {model}"` trimmed, else `"{human_size} Volume"`. A helper `bytes_to_string(&[u8])` cuts at the first NUL.
- [ ] **Step 4:** `cargo test -p opusdm-devices` passes.
- [ ] **Step 5: Commit** `feat(devices): model, filter and udisks2 object parser`.

### Task 2: Diff, errors, backend trait, ops

**Files:**
- Create: `crates/devices/src/diff.rs`, `crates/devices/src/error.rs`, `crates/devices/src/backend.rs`, `crates/devices/src/ops.rs`
- Modify: `crates/devices/src/lib.rs`

**Interfaces:**
- Consumes: `model::Device`.
- Produces:
  - `diff::DeviceEvent { Added(Device), Removed(String), Changed(Device) }`, `diff::diff(old: &[Device], new: &[Device]) -> Vec<DeviceEvent>` (`Changed` only when mount point, label or size differ)
  - `error::DeviceError { name: String, message: String }`, `DeviceError::from_dbus(name: &str, message: &str)`, `DeviceError::text(&self) -> String` (readable one line: `DeviceBusy` becomes "Device is busy", `NotAuthorized*` becomes "Not authorized", `AlreadyMounted` is not an error to show, otherwise the udisks2 message)
  - `backend::Backend` trait: `list(&self) -> Result<Vec<Device>, DeviceError>`, `mount(&self, path: &str) -> Result<PathBuf, DeviceError>`, `unmount(&self, path: &str) -> Result<(), DeviceError>`, `eject_drive(&self, drive_path: &str, power_off: bool) -> Result<(), DeviceError>`; `Backend: Send + Sync`
  - `backend::FakeBackend` (records calls as `Vec<String>`, scripted results)
  - `ops::eject(&dyn Backend, &Device, siblings: &[Device]) -> Result<(), DeviceError>`: unmounts every mounted partition of the same drive (`siblings` filtered by `drive_path`), then `eject_drive(drive_path, can_power_off)`

- [ ] **Step 1: Write the failing tests.** `diff`: empty to one device gives `[Added]`; one to empty gives `[Removed(path)]`; same mount point gives nothing; mount point `None` to `Some` gives `[Changed]`. `error`: `from_dbus("org.freedesktop.UDisks2.Error.DeviceBusy", "x").text() == "Device is busy"`; `NotAuthorizedCanObtain` text is "Not authorized"; unknown name returns the message. `ops` with `FakeBackend`: ejecting a stick with two mounted partitions calls `unmount a`, `unmount b`, then `eject drive true` in that order; an unmounted sibling is not unmounted; an unmount failure stops the sequence and returns the error without calling eject.
- [ ] **Step 2:** `cargo test -p opusdm-devices` fails.
- [ ] **Step 3: Implement** as described. `FakeBackend` keeps `calls: Mutex<Vec<String>>` and `fail_unmount: Mutex<Option<DeviceError>>`.
- [ ] **Step 4:** tests pass.
- [ ] **Step 5: Commit** `feat(devices): diff, error mapping, backend trait and eject sequence`.

### Task 3: Udisks backend and watcher over D-Bus

**Files:**
- Create: `crates/devices/src/udisks.rs`, `crates/devices/src/watch.rs`, `crates/devices/examples/list.rs`
- Modify: `crates/devices/src/lib.rs`

**Interfaces:**
- Consumes: `parse::devices`, `Backend`, `diff::diff`.
- Produces:
  - `udisks::UdisksBackend::connect() -> Result<UdisksBackend, DeviceError>` (system bus), implements `Backend`
  - `watch::spawn(backend: Arc<dyn Backend>) -> async_channel::Receiver<WatchMsg>`; `WatchMsg { Snapshot(Vec<Device>), Event(DeviceEvent), Unavailable(String) }`. The worker connects, sends `Snapshot`, then on every udisks2 signal coalesces for `COALESCE_MS = 300`, re-lists, diffs and sends events. If udisks2 is missing it sends `Unavailable` once and retries every `RETRY_SECS = 30`.

- [ ] **Step 1: Write the failing test** in `watch.rs` for the pure coalescing helper `drain_burst(rx: &Receiver<()>, quiet: Duration)` (three ticks 10 ms apart return after one quiet period, one call) and for `error::DeviceError::from_zbus(&zbus::Error)` mapping a `MethodError` name and message.
- [ ] **Step 2:** run, expect failure.
- [ ] **Step 3: Implement.** `Mount`: `proxy.call("Mount", &(options,))` on interface `org.freedesktop.UDisks2.Filesystem` with `options: HashMap<&str, Value>` empty, result `String`. `Unmount` the same with `Unmount`. `Eject`: `org.freedesktop.UDisks2.Drive.Eject` then, if `power_off`, `PowerOff`; both with empty options. `list`: `ObjectManagerProxy::new(&conn, "org.freedesktop.UDisks2", "/org/freedesktop/UDisks2")` then `get_managed_objects()` into `parse::devices`. The watcher uses `MessageIterator::for_match_rule` on signals from `org.freedesktop.UDisks2` in a reader thread that sends `()` over `std::sync::mpsc`. The example prints `list()` as one line per device.
- [ ] **Step 4:** `cargo test -p opusdm-devices`, then `cargo run -p opusdm-devices --example list` on this machine (USB drives are plugged in): the Seagate, SanDisk and Samsung volumes appear, `nvme0n1p2` and `dm-*` do not.
- [ ] **Step 5: Commit** `feat(devices): udisks2 backend and change watcher`.

### Task 4: Panel row model and DeviceCenter

**Files:**
- Create: `crates/panel/src/devices_model.rs`, `crates/panel/src/devices_center.rs`
- Modify: `crates/panel/Cargo.toml` (add `opusdm-devices = { path = "../devices" }`), `crates/panel/src/lib.rs` (`mod` lines only)

**Interfaces:**
- Consumes: `opusdm_devices::{Device, DeviceEvent, Backend, WatchMsg, ops}`.
- Produces:
  - `devices_model::Model` (pure): `apply(&mut self, WatchMsg)`, `devices() -> &[Device]`, `rows() -> Vec<Row>`, `is_empty()`, `set_busy(path, bool)`, `is_busy(path)`, `take_new(&mut self) -> Vec<Device>` (devices added after the first snapshot and not yet announced, never already mounted)
  - `devices_model::Row { device: Device, actions: Vec<Action>, busy: bool }`, `Action { Mount, Open, Unmount, Eject }`; rule: unmounted gives `[Mount]`; mounted gives `[Open, Unmount]` plus `Eject` only on the first partition of a drive that `can_eject || can_power_off`
  - `devices_center::DeviceCenter::new(on_open: Rc<dyn Fn(&Path)>) -> Rc<DeviceCenter>`, `button(self: &Rc<Self>) -> gtk::Widget`, `run(&self, Op)` with `Op { Mount(path), MountAndOpen(path), Unmount(path), Eject(path) }`

- [ ] **Step 1: Write the failing tests** for `Model`: the first `Snapshot` fills the list and `take_new()` is empty (start-up devices never popup); a later `Added` of an unmounted device appears once in `take_new()` and not twice; an `Added` already mounted does not; `Removed` drops it and clears busy; rows for a stick with two mounted partitions give `Eject` only on the first; an unmounted device has only `Mount`; a `Snapshot` after `Unavailable` replaces the list; `is_empty()` is true for no devices.
- [ ] **Step 2:** `cargo test -p opusdm-panel devices_model` fails.
- [ ] **Step 3: Implement.** `DeviceCenter` holds `Model` in a `RefCell`, a `Vec<Weak<DeviceButton>>` of listeners, and the `Arc<dyn Backend>`. `new` spawns `watch::spawn`, drains it with `glib::spawn_future_local`, feeds the model, refreshes every live button and calls `device_popup::show` (Task 5) for each `take_new()` device. `run(op)` marks the device busy, runs the backend call on `std::thread::spawn`, sends the result back over an `async_channel`, clears busy and reports an error line to the buttons (5 s). `MountAndOpen` calls `on_open(mount_point)` on success. Unplug races: if the result is an error and the device is no longer in the model, the error is dropped.
- [ ] **Step 4:** tests pass, `cargo build -p opusdm-panel`.
- [ ] **Step 5: Commit** `feat(panel): device model and center`.

### Task 5: Drop-down widget and popup

**Files:**
- Create: `crates/panel/src/widgets/devices.rs`, `crates/panel/src/device_popup.rs`
- Modify: `crates/panel/src/widgets/mod.rs`, `crates/panel/src/lib.rs` (CSS for the new classes in `install_css`)

**Interfaces:**
- Consumes: `DeviceCenter`, `Model::rows`.
- Produces: `widgets::devices::DeviceButton` (a `gtk::MenuButton` with a cairo drive icon, hidden when the model is empty, a popover with one row per device and an error label), `device_popup::show(center: &Rc<DeviceCenter>, device: &Device)`.

- [ ] **Step 1: Write the failing tests** for the pure helpers: `row_title(&Device) -> String` (`"SEAGATE (16 TB)"`), `row_subtitle(&Device) -> String` (the mount point, or `"Not mounted"`), `popup_timeout_secs() == 10`, and `popup_position(mon: (x, y, w, h), panel_top: bool, size: (w, h)) -> (x, y)` placing the window in the corner next to the panel with an 8 px margin.
- [ ] **Step 2:** run, expect failure.
- [ ] **Step 3: Implement.** Icon drawn with cairo (a rounded rectangle with a small LED), colour from the widget's foreground like the other panel icons. Rows: title, subtitle, buttons from `Row::actions` (`Mount`, `Open`, `Unmount`, `Eject`), a `gtk::Spinner` instead of buttons while busy. The popup is an undecorated `gtk::Window` (title `OpusDM Device`) with the label, size and the buttons **Mount**, **Mount and open**, **Ignore**, closed by `glib::timeout_add_seconds_local_once(10)`, by Ignore, and when the device leaves the model or becomes mounted. One popup at a time: a new one closes the old.
- [ ] **Step 4:** tests pass; `cargo build -p opusdm-panel`.
- [ ] **Step 5: Commit** `feat(panel): device drop-down and new device popup`.

### Task 6: Wire into the panel and the hub, docs

**Files:**
- Modify: `crates/panel/src/lib.rs` (`attach` signature and `build_bar_window`), `crates/hub/src/toolbar.rs:236`, `docs/usage.md`, `CHANGES.md`, `scripts/build-release.sh` only if the lib needs nothing (no new binary: leave untouched)

**Interfaces:**
- Consumes: `DeviceCenter::new`, `DeviceCenter::button`.
- Produces: `opusdm_panel::attach(app, position, on_open_folder: Rc<dyn Fn(&Path)>)`; in the right box the order is tray, separator, **device button**, sysmon, separator, clock (primary bar only; the separator after it only when the button exists).

- [ ] **Step 1:** Update every `attach` caller (grep `opusdm_panel::attach`). The hub passes a closure that calls `children::spawn_lister(&sock_path, Some(path))` and logs an error with `opusdm_ipc::err_log!`.
- [ ] **Step 2:** In `attach`, create the `DeviceCenter` once and pass it to `build_bar_window`; the primary bar appends `center.button()` before `widgets::sysmon::build()`.
- [ ] **Step 3:** `cargo build --workspace` and `cargo test --workspace`; fix fallout.
- [ ] **Step 4: Run it.** Start the session under Xephyr with `run-x11.sh`, check the panel shows the drive icon left of the CPU graph (devices are plugged in on this machine), open the drop-down, Mount a volume, Open (a Lister opens on `/media/fabio/...`), Unmount. State in `CHANGES.md` exactly what was and was not checked (plugging a new stick for the popup needs the real session).
- [ ] **Step 5: Docs.** New section in `docs/usage.md` (Device notifier: icon, drop-down actions, popup, `/media/<user>`, encrypted volumes not supported). Entry in `CHANGES.md` for 2026-10-06 in the style of the others.
- [ ] **Step 6: Commit** `feat(panel): device notifier in the panel, mount and eject removable drives`.
