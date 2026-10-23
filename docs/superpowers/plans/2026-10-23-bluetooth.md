# Bluetooth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bluetooth like KDE Plasma 6: a tray icon in the panel and a Bluetooth tool (Settings > Advanced and `opusdm-bluetooth-manager`) to power the adapter, pair, connect and manage devices.

**Architecture:** a GTK-free crate `opusdm-bluetooth` talks to BlueZ over the system bus with zbus (model, parser, backend trait + fake, watcher, pairing agent, rfkill). `opusdm-bluetooth-config` is the GTK tool (embedded in Settings, standalone binary) and holds the agent dialogs. The panel gets `BluetoothCenter` + widget, owned by `attach()` like `PrinterCenter`.

**Tech Stack:** Rust 2021, gtk4 0.10, zbus 5.19 (blocking + async interface for the agent), async-channel.

**Spec:** `docs/superpowers/specs/2026-10-23-bluetooth-design.md`

## Global Constraints

- No em dash anywhere; English in code and comments; constants UPPERCASE.
- Every failure path in a handler, loop or worker logs with `opusdm_ipc::err_log!`; a failed `Weak::upgrade()` logs.
- No `bluetoothctl`; BlueZ only over D-Bus (`org.bluez`).
- Never block the GTK thread: every BlueZ call runs on a worker thread.
- Adapter never left pairable for good: `Discoverable` + `Pairable` together with both timeouts = 180 s; pairing flow sets `Pairable` while open.
- A pairing request with no live UI is rejected (`org.bluez.Error.Rejected`) and logged.
- One risky change per commit; commit bodies of lifetime changes answer "who owns this object for the whole run, and what happens when it is gone?".
- No AI attribution lines in commits; never push.

## Review Focus

1. BlueZ stopped / restarted while the panel runs: icon hides, comes back, agent registered again (watcher test with fake session failure + manual `systemctl restart bluetooth` check).
2. Adapter missing or rfkill soft-blocked: icon hidden / off; switching on clears the soft block (rfkill encode test).
3. A pairing request arriving when the dialog side is gone: rejected, not left hanging (agent test with the receiver dropped).
4. Agent `Cancel` arriving while a dialog is open: dialog closes (agent forwards `Cancel`, test).
5. Discovery left running after the pairing flow closes or the Settings page is left (StopDiscovery on close; fake backend call log test).

---

## File structure

- `crates/bluetooth/` (new, `opusdm_bluetooth`)
  - `model.rs`: `BtState`, `Adapter`, `Device`, `DeviceKind`, sorting.
  - `parse.rs`: managed objects -> `BtState`.
  - `backend.rs`: `Backend` trait, `Request`, `run_request`, `FakeBackend`.
  - `bluez.rs`: `BluezBackend` (zbus blocking).
  - `rfkill.rs`: read `/sys/class/rfkill`, write `/dev/rfkill`.
  - `agent.rs`: `org.bluez.Agent1` object, `AgentRequest`, `AgentReply`, register.
  - `watch.rs`: `spawn() -> Receiver<WatchMsg>`.
  - `error.rs`: `BtError = opusdm_watch::DbusError`.
- `crates/bluetooth-config/` (new, `opusdm_bluetooth_config`, bin `opusdm-bluetooth-manager`)
  - `agent_dialog.rs`: GTK dialogs answering an `AgentRequest` (shared with the panel).
  - `window.rs`: list + details + adapter header; `embedded()`, `open()`.
  - `pair_dialog.rs`: scan list and pairing flow.
  - `worker.rs`: request worker thread for the window.
  - `main.rs`: `--pair` flag.
- `crates/panel/src/bluetooth_model.rs`, `bluetooth_center.rs`, `widgets/bluetooth.rs`.
- `crates/theme-config/src/advanced.rs`: "Bluetooth" entry.
- `scripts/build-release.sh`, `scripts/live-update.sh`, `Cargo.toml` members, `CHANGES.md`.

### Task 1: model, parser, backend trait, rfkill

**Interfaces - Produces:**
- `pub struct Adapter { pub path: String, pub address: String, pub alias: String, pub powered: bool, pub discoverable: bool, pub pairable: bool, pub discovering: bool }`
- `pub enum DeviceKind { Headset, Audio, Keyboard, Mouse, Phone, Computer, Gamepad, Other }` + `fn from_icon(icon: &str) -> Self` + `fn icon_name(&self) -> &'static str`
- `pub struct Device { pub path: String, pub address: String, pub name: String /* Alias, else Name, else address */, pub kind: DeviceKind, pub paired: bool, pub trusted: bool, pub blocked: bool, pub connected: bool, pub battery: Option<u8> }` (no RSSI: it would make every scan tick a change)
- `pub struct BtState { pub adapter: Option<Adapter>, pub devices: Vec<Device>, pub rfkill: Rfkill }`, `fn paired(&self) -> impl Iterator<Item=&Device>` (connected first, then name), `fn found(&self)` (not paired), `fn device(&self, path) -> Option<&Device>`
- `pub fn parse::state(objects: &ManagedObjects) -> BtState` where `ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>`; first adapter by path; only devices of that adapter.
- `pub enum Request { SetPowered(bool), SetDiscoverable(bool), SetAdapterAlias(String), StartDiscovery, StopDiscovery, SetPairable(bool), Connect(String), Disconnect(String), Pair(String), CancelPairing(String), SetTrusted(String, bool), SetBlocked(String, bool), SetAlias(String, String), Remove(String) }`
- `pub trait Backend: Send + Sync { fn run(&self, r: &Request) -> Result<(), BtError>; fn state(&self) -> Result<BtState, BtError>; }`, `FakeBackend { state: Mutex<BtState>, calls: Mutex<Vec<String>>, fail: Mutex<Option<BtError>> }`
- `pub struct Rfkill { pub soft: bool, pub hard: bool }`, `rfkill::read() -> Rfkill`, `rfkill::encode_unblock() -> [u8; 8]` (idx 0, type 2 BLUETOOTH, op 3 CHANGE_ALL, soft 0, hard 0), `rfkill::unblock() -> Result<(), BtError>`
- `SetPowered(true)` in `BluezBackend` calls `rfkill::unblock()` first when soft-blocked; `SetDiscoverable(on)` writes `DiscoverableTimeout`/`PairableTimeout` = 180 then `Pairable` and `Discoverable`.

- [ ] Tests: `parse_reads_adapter_devices_and_battery`, `device_name_falls_back_to_name_then_address`, `devices_of_other_adapters_are_skipped`, `paired_lists_connected_first`, `kind_from_bluez_icon` (`audio-headset` -> Headset, `input-keyboard` -> Keyboard, `input-mouse` -> Mouse, `phone` -> Phone, `x` -> Other), `rfkill_unblock_event_bytes` (`[0,0,0,0,2,3,0,0]`), `fake_records_calls`.
- [ ] Implement, `cargo test -p opusdm-bluetooth`, commit.

### Task 2: watcher and agent

**Interfaces - Produces:**
- `pub enum WatchMsg { Backend(Arc<dyn Backend>), State(BtState), Unavailable(String) }`, `watch::spawn(agent: Option<AgentSetup>) -> Receiver<WatchMsg>`; `run_with_retry` session: connect, register agent, `GetManagedObjects`, then re-read on each burst of `org.bluez` signals; a `NameOwnerChanged` for `org.bluez` (match rule with `arg0`) ends the session with `Err` so it reconnects and registers the agent again. `rfkill` re-read on each burst and every 5 s poll tick is not needed (BlueZ signals `Powered`).
- `pub enum AgentKind { PinCode, Passkey, DisplayPinCode(String), DisplayPasskey(u32, u16), Confirm(u32), Authorize, AuthorizeService(String) }`
- `pub enum AgentReply { Pin(String), Passkey(u32), Accept, Reject }`
- `pub enum AgentEvent { Request { device: String, kind: AgentKind, reply: async_channel::Sender<AgentReply> }, Cancel }`
- `pub struct AgentSetup { pub path: &'static str, pub default: bool, pub events: async_channel::Sender<AgentEvent> }`; paths `PANEL_AGENT_PATH = "/org/opusdm/bluetooth/panel"`, `MANAGER_AGENT_PATH = "/org/opusdm/bluetooth/manager"`, capability `KeyboardDisplay`.
- `fn answer(kind: &AgentKind, reply: Option<AgentReply>) -> Result<Answer, AgentError>` pure mapping; `None` (receiver gone / dialog closed) -> `Rejected`; wrong reply type -> `Rejected`. Display kinds answer at once.

- [ ] Tests: `display_kinds_answer_at_once`, `pin_reply_answers_pin_request`, `missing_reply_rejects`, `wrong_reply_rejects`, `a_dropped_receiver_rejects` (send to a closed channel inside the async method body helper `ask(events, device, kind)`), `cancel_is_forwarded`.
- [ ] Implement, test, commit.

### Task 3: Bluetooth tool (manager + Settings)

**Interfaces - Consumes:** Tasks 1-2. **Produces:** `opusdm_bluetooth_config::window::{open, embedded, APP_CSS}`, `BluetoothTool: EmbeddedTool` (never dirty; save/revert call `done` at once), `agent_dialog::show(parent: &gtk::Window, state: &BtState, device: &str, kind: AgentKind, reply: Sender<AgentReply>) -> gtk::Window` (returned so `Cancel` can close it), `open_pairing(app)` for `--pair`.

- Ownership as in `printer-config`: `build_with(window, backend)` returns `Rc<App>`; standalone keeps it with `keep_until_closed`; embedded keeps it in `BluetoothTool`; loops hold it weakly with logging `listen`.
- Pairing flow (`pair_dialog.rs`): `SetPairable(true)`, `StartDiscovery`; list of found devices updates live; Pair -> on success `SetTrusted(true)` then `Connect`; on close `StopDiscovery`, `SetPairable(false)`. Its own agent (`MANAGER_AGENT_PATH`, not default).
- Settings: entry `Bluetooth` after `Printers` in `advanced.rs` (`ENTRIES: [Entry; 6]`).

- [ ] Tests (GTK, via `gtk_test.rs` copy): `the_program_window_owns_the_app` (200 main loop turns, list still follows the fake), `connect_button_reaches_backend`, `closing_pairing_stops_discovery` (fake calls end with `StopDiscovery`, `SetPairable(false)`).
- [ ] Implement, test, run the manager in Xephyr against the real BlueZ, commit; then the Settings entry + scripts, commit.

### Task 4: panel tray

**Interfaces - Consumes:** Tasks 1-3. **Produces:** `BluetoothCenter::new() -> Rc<Self>`, `button()`, `send(Request)`; `bluetooth_model::{icon_look(&BtState) -> Look { Hidden, Off, On, Connected }, tooltip(&BtState) -> String, Notifier::update(&BtState) -> Vec<Note>}`.
- Notes: device connected / disconnected (paired devices only, not on the first state), battery crossing below 20 % (once per crossing, again after going above), pair failure (from the request worker).
- Agent: center passes `AgentSetup { PANEL_AGENT_PATH, default: true }`; events shown with `agent_dialog::show` (no parent: a top-level window); `Cancel` closes the open one.
- Placed before the printer button in `attach`; strong ownership like `PrinterCenter`.

- [ ] Tests: `icon_hidden_without_adapter`, `icon_off_when_not_powered`, `icon_connected_with_a_connected_device`, `tooltip_names_connected_devices`, `first_state_sends_no_notes`, `connect_and_disconnect_notes`, `battery_low_once_per_crossing`.
- [ ] Implement, test, run sandbox: icon, popover, switch, connect; rule 4 checklist; commit.

### Task 5: CHANGES.md and final verification

- [ ] `cargo build --workspace`, `cargo test` for the touched crates, `cargo clippy` for them with no new warnings.
- [ ] CHANGES.md entry with what was checked and NOT checked; commit.
