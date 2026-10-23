# Printer Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A CUPS printer manager (`opusdm-printer-manager`, also the "Printers" entry of Settings / Advanced) with the print queue, and a printer icon in the panel tray that shows the queue and notifies errors.

**Architecture:** A new non-GTK crate `opusdm-printing` talks to CUPS: reads go through a small IPP client over the CUPS domain socket (locale independent, cheap enough to poll), changes go through the CUPS command line tools run with `LC_ALL=C` (they authenticate through the socket's peer credentials, checked: no password for the `lpadmin` group). A new GTK crate `opusdm-printer-config` builds the window like `opusdm-network-config` (worker thread, `EmbeddedTool`). The panel gets a `printer_center` like `network_center`, fed by a polling watcher of `opusdm-printing`.

**Tech Stack:** Rust 2021, gtk4 0.10, async-channel, std `UnixStream` (no new external dependency), zbus (already in the panel) for notifications.

**Spec:** `docs/superpowers/specs/2026-10-23-printer-manager-design.md`

## Global Constraints

- `CLAUDE.md` rules apply: no behaviour change outside the new feature, no silent failure (every failure path logs with `opusdm_ipc::err_log!` or is shown), live checks before "done", `CHANGES.md` updated with what was not verified, one risky change per commit, commit only own files, never push.
- English in code and comments, never the em dash character, constants in UPPER_CASE, early returns as one-liners at the top, doc comment on every function.
- No new external crate: IPP and HTTP are written by hand on `std::os::unix::net::UnixStream`.
- Every CUPS command runs with `LC_ALL=C` and `LANG=C` (outputs are localized: Italian dates were seen).
- CUPS socket: `/run/cups/cups.sock`, fallback `/var/run/cups/cups.sock`. Timeouts: 5 s read/write on the socket; `lpinfo -v` with `--timeout 10`.
- The existing `C364Series` queue is never modified by tests or by live checks; live checks use temporary queues named `opusdm_probe*` with device `socket://127.0.0.1:9`, removed afterwards.
- Defaults are system wide: options with `lpadmin -p NAME -o KEY=VALUE`, default printer with `lpadmin -d NAME`.
- Tray error rule (spec, verbatim meaning): red + notification for a state reason ending in `-error`, a stopped job (`job-state` 6) or a printer stopped by CUPS (`printer-state` 5) while jobs wait; a printer disabled by the owner (stopped, no jobs, or reason `paused`) is grey, no notification; `-warning`/`-report` reasons only in the popup; a printer that does not answer while jobs wait shows the normal badge and notifies once after `OFFLINE_NOTIFY_SECS = 120`.

## Review Focus

1. A printer with a large vendor PPD (the Konica: 38 options, long choice lists) must make a usable Options tab, not a 2000 px tall form: common options first, the rest in an expander, scrolled. Test: `options_are_split_common_first` (Task 3).
2. CUPS stopped or the socket missing: the manager shows "CUPS is not available: ..." and the panel icon stays hidden, nothing panics, the log has one line per state change (not one per poll). Tests: `transport_reports_a_missing_socket` (Task 4), `watcher_logs_unavailable_once` (Task 5).
3. A job id that disappears between the list and the click (printed meanwhile): the action error is shown in the message line, the queue refreshes. Test: `a_failed_queue_action_refreshes` (Task 6).
4. The same error must not notify on every poll, and must notify again after it was cleared and comes back. Test: `notifies_once_per_error_episode` (Task 5).
5. Queue names typed in the wizard with spaces, slashes, `#` or longer than 127 bytes are refused with a reason; the suggested name from "KONICA MINOLTA bizhub C224e" is valid. Test: `queue_names` (Task 3).

---

## File structure

`crates/printing` (`opusdm-printing`, lib `opusdm_printing`, no GTK):
- `src/lib.rs` - module list, re-exports.
- `src/ipp.rs` - IPP request encoder and response decoder (pure).
- `src/transport.rs` - one HTTP POST of an IPP body over the CUPS socket.
- `src/model.rs` - `Printer`, `PrinterState`, `Job`, `JobState`, `Severity`, `Device`, `Driver`, `PrinterOption`, `NewPrinter`, `DriverChoice`, `PrintError`; mapping from IPP attributes.
- `src/parse.rs` - parsers of `lpinfo -l -v`, `lpinfo -m`, `lpoptions -l`; `validate_queue_name`, `suggest_queue_name`, `is_driverless_uri`, `split_options`.
- `src/backend.rs` - `trait PrintBackend` and `FakeBackend`.
- `src/cups.rs` - `CupsBackend` (IPP reads, command writes).
- `src/watch.rs` - polling thread sending `Snapshot`s.
- `src/tray.rs` - `TrayView` and `Notifier` (pure, time injected).

`crates/printer-config` (`opusdm-printer-config`, lib `opusdm_printer_config`, bin `opusdm-printer-manager`):
- `src/lib.rs`, `src/main.rs`
- `src/worker.rs` - `Job`/`Done`, `run_job`, `spawn`.
- `src/session.rs` - the draft of the General fields and the Options.
- `src/wizard.rs` - wizard state (pure).
- `src/wizard_ui.rs` - the wizard dialog.
- `src/pages.rs` - General, Options and Queue tab widgets.
- `src/window.rs` - the window, `PrinterTool: EmbeddedTool`, `embedded`, `open`.
- `src/gtk_test.rs` - copy of `crates/desktop-config/src/gtk_test.rs`.

Modified:
- `Cargo.toml` (workspace members), `crates/theme-config/Cargo.toml`, `crates/theme-config/src/advanced.rs` (one entry).
- `scripts/build-release.sh` (`BINARIES`, desktop entry pairs), `scripts/live-update.sh` (`PROCESSES`).
- `crates/panel/Cargo.toml`, `crates/panel/src/lib.rs` (create center, add button to the trays box), new `crates/panel/src/printer_center.rs`, `crates/panel/src/widgets/printer.rs`, `crates/panel/src/widgets/mod.rs`.
- `CHANGES.md`.

---

### Task 1: `opusdm-printing` crate and IPP codec

**Files:**
- Create: `crates/printing/Cargo.toml`, `crates/printing/src/lib.rs`, `crates/printing/src/ipp.rs`
- Modify: `Cargo.toml` (add `"crates/printing"` to `members`)

**Interfaces:**
- Produces:
  - `pub enum Value { Integer(i32), Boolean(bool), Enum(i32), Text(String), Other(u8, Vec<u8>) }` with `as_int(&self) -> Option<i32>`, `as_bool`, `as_str(&self) -> Option<&str>`.
  - `pub struct Group { pub tag: u8, pub attrs: Vec<(String, Vec<Value>)> }` with `get(&self, name) -> Option<&[Value]>`, `str(&self, name) -> Option<&str>` (first value), `int(&self, name) -> Option<i32>`, `strs(&self, name) -> Vec<String>`.
  - `pub struct Response { pub status: u16, pub groups: Vec<Group> }` with `groups_of(&self, tag: u8) -> impl Iterator<Item=&Group>`, `status_message(&self) -> Option<&str>`, `is_ok(&self) -> bool` (status < 0x0100).
  - `pub struct Request { op: u16, id: u32, attrs: Vec<(u8, String, Vec<Vec<u8>>)> }`: `Request::new(op: u16) -> Self` (adds charset `utf-8`, language `en`), `.uri(name, &str)`, `.name(name, &str)` (tag 0x42), `.keyword(name, &str)`, `.keywords(name, &[&str])` (1setOf), `.boolean(name, bool)`, `.integer(name, i32)`, `encode(&self) -> Vec<u8>`.
  - `pub fn decode(bytes: &[u8]) -> Result<Response, String>`.
  - Constants: `OP_GET_JOBS = 0x000A`, `OP_CUPS_GET_DEFAULT = 0x4001`, `OP_CUPS_GET_PRINTERS = 0x4002`, `TAG_OPERATION = 0x01`, `TAG_JOB = 0x02`, `TAG_END = 0x03`, `TAG_PRINTER = 0x04`, `STATUS_NOT_FOUND = 0x0406`.

Encoding (IPP/2.0, RFC 8010): header `version 2.0, op (u16 BE), request id (u32 BE)`, then `0x01`, then per attribute `tag, name len u16, name, value len u16, value`; additional values of a 1setOf repeat the tag with an empty name; end with `0x03`. Integer and enum (0x21, 0x23) are 4-byte BE, boolean (0x22) 1 byte. Decoding: a tag below 0x10 starts a group (0x03 ends); an empty name appends to the previous attribute; text tags 0x30..=0x4A become `Text` (lossy UTF-8).

- [ ] **Step 1: Write the failing tests** in `ipp.rs`:
  - `encodes_a_get_printers_request`: `Request::new(OP_CUPS_GET_PRINTERS).keywords("requested-attributes", &["printer-name", "printer-state"]).encode()` starts with `[2, 0, 0x40, 0x02, 0, 0, 0, 1, 0x01, 0x47]`, contains the bytes of `attributes-charset` / `utf-8`, contains a second `0x44` attribute with name length 0 and value `printer-state`, ends with `0x03`.
  - `decodes_printers_and_values`: a hand-built response (status 0, group 1 with charset, group 4 with `printer-name`=`C364Series` tag 0x42, `printer-state` enum 3, `printer-is-accepting-jobs` true, `printer-state-reasons` two values `none`... use `media-empty-error` and `toner-low-warning`, then group 4 with `printer-name`=`opusdm_probe`, end) decodes to two printer groups; `int("printer-state") == Some(3)`, `strs("printer-state-reasons") == ["media-empty-error","toner-low-warning"]`.
  - `decode_rejects_truncated_input`: `decode(&[2,0,0])` is `Err`, and a response cut inside a value is `Err`.
  - `status_message_is_read`: status 0x0406 with `status-message` "No default printer." gives `status == STATUS_NOT_FOUND`, `!is_ok()`, `status_message() == Some("No default printer.")`.
- [ ] **Step 2:** `cargo test -p opusdm-printing` fails (missing items).
- [ ] **Step 3:** Implement `ipp.rs`, `lib.rs` (`pub mod ipp;`), `Cargo.toml` (deps: `opusdm-ipc` path, `async-channel.workspace`; dev: none).
- [ ] **Step 4:** `cargo test -p opusdm-printing` passes; `cargo clippy -p opusdm-printing` has no warning.
- [ ] **Step 5:** Commit `printing: IPP request encoder and response decoder`.

### Task 2: model and mapping from IPP

**Files:** Create `crates/printing/src/model.rs`; modify `lib.rs`.

**Interfaces:**
- Consumes: `ipp::Group`.
- Produces:
  - `pub enum PrinterState { Idle, Processing, Stopped }` (3, 4, 5; anything else `Idle`).
  - `pub enum JobState { Pending, Held, Processing, Stopped, Canceled, Aborted, Completed }` (3..=9), `label(&self) -> &'static str` ("Waiting", "Held", "Printing", "Stopped", "Canceled", "Aborted", "Completed"), `is_active(&self) -> bool` (Pending, Held, Processing, Stopped).
  - `pub enum Severity { None, Report, Warning, Error }`, `pub fn reason_severity(reason: &str) -> Severity` (suffix `-error`, `-warning`, `-report`; `none` and others `None`).
  - `#[derive(Clone, Debug, PartialEq)] pub struct Printer { pub name, pub info, pub location, pub make_model, pub device_uri, pub state_message: String, pub state: PrinterState, pub reasons: Vec<String>, pub accepting: bool, pub is_default: bool }` with `from_group(&Group) -> Option<Printer>` (None without `printer-name`), `is_paused(&self) -> bool` (state Stopped or reason `paused`), `worst(&self) -> Severity`.
  - `#[derive(Clone, Debug, PartialEq)] pub struct Job { pub id: i32, pub printer: String, pub name: String, pub user: String, pub state: JobState, pub reasons: Vec<String>, pub created: i64 }` with `from_group(&Group) -> Option<Job>`; `printer` is the last path segment of `job-printer-uri` (`ipp://localhost:631/printers/opusdm_probe` gives `opusdm_probe`).
  - `pub struct Device { pub uri, pub info, pub make_model, pub device_id: String, pub class: String }`.
  - `pub struct Driver { pub name: String, pub description: String }` (`name` is what `lpadmin -m` takes).
  - `pub struct PrinterOption { pub key: String, pub label: String, pub choices: Vec<String>, pub default: String }`.
  - `pub enum DriverChoice { Driverless, Model(String), PpdFile(std::path::PathBuf) }`.
  - `pub struct NewPrinter { pub name, pub uri, pub info, pub location: String, pub driver: DriverChoice, pub make_default: bool }`.
  - `pub struct PrinterEdit { pub info: String, pub location: String, pub options: Vec<(String, String)> }` (only the changed options).
  - `#[derive(Clone, Debug, PartialEq)] pub struct PrintError(pub String)` with `Display`, `PrintError::new(impl Into<String>)`.
  - `pub const JOB_ATTRS: &[&str]` and `PRINTER_ATTRS: &[&str]` (the attributes asked: for printers `printer-name, printer-info, printer-location, printer-make-and-model, device-uri, printer-state, printer-state-reasons, printer-state-message, printer-is-accepting-jobs`; for jobs `job-id, job-name, job-state, job-state-reasons, job-printer-uri, job-originating-user-name, time-at-creation`).

- [ ] **Step 1: Failing tests:** `printer_from_group` (values of the Task 1 fixture; missing name gives None), `job_from_group` (job 32, `t.txt`, `fabio`, state 5 = Processing, printer `opusdm_probe`), `reason_severity_by_suffix` (`media-jam-error` Error, `toner-low-warning` Warning, `cups-insecure-filter-warning` Warning, `other-report` Report, `none` None, `paused` None), `paused_printer` (state 5 is paused; reason `paused` with state 3 is paused), `job_state_labels_and_activity`.
- [ ] **Step 2:** run, fail. **Step 3:** implement. **Step 4:** pass, clippy clean.
- [ ] **Step 5:** Commit `printing: printer and job model`.

### Task 3: command output parsers and name rules

**Files:** Create `crates/printing/src/parse.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
  - `pub fn parse_devices(out: &str) -> Vec<Device>` for `lpinfo -l -v` (blocks starting `Device: uri = X`, then `key = value` lines: `class`, `info`, `make-and-model`, `device-id`). Only devices whose uri contains `://` are kept (the bare scheme entries `ipp`, `socket`, ... are the manual kinds).
  - `pub fn parse_drivers(out: &str) -> Vec<Driver>` for `lpinfo -m` (first space splits name and description).
  - `pub fn filter_drivers<'a>(drivers: &'a [Driver], query: &str, limit: usize) -> Vec<&'a Driver>` (every whitespace word of the query, case insensitive, is in the description; empty query gives the first `limit`).
  - `pub fn parse_options(out: &str) -> Vec<PrinterOption>` for `lpoptions -p X -l` (`Key/Label: c1 *c2 c3`; the starred choice is the default without the star; no star: first choice).
  - `pub fn split_options(options: &[PrinterOption]) -> (Vec<&PrinterOption>, Vec<&PrinterOption>)`: common first, in the order of `COMMON_OPTION_KEYS = ["PageSize", "Duplex", "KMDuplex", "ColorModel", "SelectColor", "print-color-mode", "cupsPrintQuality", "print-quality", "Resolution", "InputSlot", "MediaType"]`, then the rest in file order.
  - `pub fn validate_queue_name(name: &str) -> Result<(), String>`: not empty, at most 127 bytes, only printable ASCII without space, `/`, `#`, `\`, `'`, `"`; error texts "The name is empty", "The name is longer than 127 characters", "The name cannot contain spaces, /, #, \\ or quotes".
  - `pub fn suggest_queue_name(make_model: &str, taken: &[String]) -> String`: the words of `make_model` with every character not allowed replaced by `_`, runs of `_` merged, trimmed of `_`, the repeated manufacturer word dropped (`KONICA MINOLTA KONICA MINOLTA bizhub C224e` gives `KONICA_MINOLTA_bizhub_C224e`), `Printer` when empty, and `_2`, `_3`, ... when taken.
  - `pub fn is_driverless_uri(uri: &str) -> bool`: `ipp://`, `ipps://`, or a `dnssd://` uri containing `._ipp._tcp` or `._ipps._tcp`. (The bizhub offers only `._pdl-datastream._tcp`: not driverless.)

- [ ] **Step 1: Failing tests** with fixtures copied from this machine (see the plan header facts): `devices_from_lpinfo` (the dnssd bizhub block and the bare `ipp` block: one device kept, its make-and-model and device-id read), `drivers_and_filter` (three lines incl. `drv:///cupsfilters.drv/pwgrast.ppd Generic IPP Everywhere Printer`; query `konica c250` finds only `KOC250UX`), `options_from_lpoptions` (lines `PageSize/Paper Size: A3 *A4 A5`, `KMDuplex/Duplex: *False True`, `Resolution/Resolution: *600dpi`; defaults `A4`, `False`, `600dpi`), `options_are_split_common_first` (input order `Staple, KMDuplex, PageSize, Finisher` gives common `[PageSize, KMDuplex]`, rest `[Staple, Finisher]`), `queue_names` (Review Focus 5), `suggested_names` (bizhub case; taken gives `_2`), `driverless_uris`.
- [ ] **Step 2-4:** fail, implement, pass + clippy.
- [ ] **Step 5:** Commit `printing: parsers of the CUPS tools and queue name rules`.

### Task 4: backend trait, fake, CUPS backend

**Files:** Create `crates/printing/src/transport.rs`, `backend.rs`, `cups.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
  - `transport::post(socket: &Path, body: &[u8]) -> Result<Vec<u8>, PrintError>`: `POST / HTTP/1.1`, `Host: localhost`, `Content-Type: application/ipp`, `Content-Length`, `Connection: close`; reads to EOF; requires status line `200`; returns the body after `\r\n\r\n` (truncated to `Content-Length` when present). Errors name the socket: "CUPS is not available (/run/cups/cups.sock): ...".
  - `transport::socket_path() -> PathBuf` (first existing of the two constants, else the first).
  - `pub trait PrintBackend: Send + Sync`:
    - `fn printers(&self) -> Result<Vec<Printer>, PrintError>` (with `is_default` set)
    - `fn jobs(&self) -> Result<Vec<Job>, PrintError>` (all printers, not completed)
    - `fn devices(&self) -> Result<Vec<Device>, PrintError>`
    - `fn drivers(&self) -> Result<Vec<Driver>, PrintError>`
    - `fn options(&self, printer: &str) -> Result<Vec<PrinterOption>, PrintError>`
    - `fn add(&self, new: &NewPrinter) -> Result<(), PrintError>`
    - `fn modify(&self, printer: &str, edit: &PrinterEdit) -> Result<(), PrintError>`
    - `fn remove(&self, printer: &str) -> Result<(), PrintError>`
    - `fn set_default(&self, printer: &str) -> Result<(), PrintError>`
    - `fn set_enabled(&self, printer: &str, on: bool) -> Result<(), PrintError>`
    - `fn test_page(&self, printer: &str) -> Result<(), PrintError>`
    - `fn cancel(&self, job: i32) -> Result<(), PrintError>`
    - `fn cancel_all(&self, printer: &str) -> Result<(), PrintError>`
    - `fn hold(&self, job: i32) -> Result<(), PrintError>`
    - `fn release(&self, job: i32) -> Result<(), PrintError>`
    - `fn retry(&self, printer: &str, job: Option<i32>) -> Result<(), PrintError>` (enable the printer; for a job also restart it)
  - `FakeBackend` (`#[derive(Default)]`, public fields `printers: Mutex<Vec<Printer>>`, `jobs: Mutex<Vec<Job>>`, `options: Mutex<Vec<PrinterOption>>`, `fail: Mutex<Option<PrintError>>`, `calls: Mutex<Vec<String>>`, `calls()` getter). Every method records a call like `"cancel 41"`, `"add NAME"`, `"modify NAME"`, then returns `fail` if set. `add`/`remove`/`cancel` change the lists so the UI tests see them.
  - `CupsBackend::new() -> CupsBackend`. Commands (all `LC_ALL=C LANG=C`, stderr in the error, error text `"<tool>: <first stderr line>"`):
    - add: `lpadmin -p NAME -E -v URI -D INFO -L LOCATION` + `-m everywhere` | `-m DRIVER` | `-P FILE`, then `lpadmin -d NAME` when `make_default`.
    - modify: `lpadmin -p NAME -D INFO -L LOCATION` and one `-o KEY=VALUE` per option.
    - remove `lpadmin -x NAME`; default `lpadmin -d NAME`; enable `cupsenable NAME` + `cupsaccept NAME`, disable `cupsdisable NAME` (accepting kept); test page `lp -d NAME -t "Test page" /usr/share/cups/data/testprint`; cancel `cancel ID`; cancel all `cancel -a NAME`; hold `lp -i ID -H hold`; release `lp -i ID -H resume`; retry `cupsenable NAME` then for a job `lp -i ID -H restart`.
    - devices `lpinfo -l -v --timeout 10`; drivers `lpinfo -m`; options `lpoptions -p NAME -l`.
    - printers: `CUPS-Get-Printers` with `PRINTER_ATTRS` and `CUPS-Get-Default` (`STATUS_NOT_FOUND` means no default, not an error); jobs: `Get-Jobs` on `ipp://localhost/` with `which-jobs=not-completed`, `my-jobs=false`, `requesting-user-name` = `$USER`, `JOB_ATTRS`. `CUPS-Get-Printers` answering not-found means no printers.

- [ ] **Step 1: Failing tests:** `transport_reports_a_missing_socket` (`post(Path::new("/nonexistent/cups.sock"), b"")` is `Err` whose text contains `CUPS is not available` and the path); `transport_reads_the_body` (a `UnixListener` in a tempdir answers `HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc`; result `b"abc"`); `transport_refuses_http_errors` (answer `HTTP/1.1 403 Forbidden`: `Err` containing `403`); `fake_records_calls_and_fails` (add then cancel; `fail` set makes `printers()` return it). In `cups.rs`: `#[test] #[ignore] live_probe_queue` that adds `opusdm_probe_test` on `socket://127.0.0.1:9` with `DriverChoice::Model("drv:///cupsfilters.drv/pwgrast.ppd")`... if that driver is missing use no `-m`; then lists printers (found, not default), sends the test page, finds its job in `jobs()`, holds, releases, cancels, removes, and checks it is gone. Run by hand with `--ignored`.
- [ ] **Step 2-4:** fail, implement (`tempfile` as dev-dependency), pass, then `cargo test -p opusdm-printing -- --ignored live_probe_queue` passes on this machine and `lpstat -v` shows only `C364Series` afterwards.
- [ ] **Step 5:** Commit `printing: CUPS backend (IPP reads, command line changes) and fake`.

### Task 5: watcher and tray logic

**Files:** Create `crates/printing/src/watch.rs`, `crates/printing/src/tray.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq, Default)] pub struct Snapshot { pub printers: Vec<Printer>, pub jobs: Vec<Job> }`.
  - `pub enum WatchMsg { Snapshot(Snapshot), Unavailable(String) }`.
  - `pub fn watch::spawn(backend: Arc<dyn PrintBackend>) -> (async_channel::Receiver<WatchMsg>, std::sync::mpsc::Sender<()>)`: a named thread `opusdm-printing-watch` polls `printers()` + `jobs()`; sends only when the result differs from the last one sent; waits `ACTIVE_POLL_MS = 2000` while any job is active, else `IDLE_POLL_MS = 5000`; a `()` on the returned sender polls at once (after an action). Ends when the receiver is dropped. Logs `Unavailable` once per change of state (`err_log!("opusdm-printing", ...)`), not per poll.
  - `pub fn watch::poll_once(backend: &dyn PrintBackend, last: &mut Option<Result<Snapshot, String>>) -> Option<WatchMsg>` (the testable step).
  - `pub enum Look { Hidden, Normal, Paused, Error }`.
  - `pub struct TrayView { pub look: Look, pub jobs: usize, pub tooltip: String }`, `pub fn tray::view(s: &Snapshot) -> TrayView`: `jobs` = active jobs; `Error` when a printer has an `-error` reason, a job is Stopped, or a printer is Stopped (not owner-paused) with active jobs; `Paused` when a printer with active jobs is paused by the owner (no error); `Hidden` when no active job and no error; else `Normal`. Tooltip: `"2 print jobs"`, `"1 print job"`, with the first error text appended (`"C364Series: media-jam-error"` or the state message).
  - "Owner-paused" vs "stopped by CUPS": a Stopped printer whose `state_message` is empty or whose reasons contain `paused` is owner-paused; a Stopped printer with a state message (CUPS writes why it stopped it) is an error.
  - `pub struct Note { pub summary: String, pub body: String }`.
  - `pub struct Notifier` (`Default`) with `fn update(&mut self, s: &Snapshot, now_secs: u64) -> Vec<Note>`: a note when an error key appears (key = printer + reason, or `job:<id>` for a stopped job); keys that are gone are forgotten, so a later return notifies again. Offline: a printer in `Processing` whose `state_message` is not empty and that has active jobs is "not answering" since the first snapshot that showed it; one note when `now - since >= OFFLINE_NOTIFY_SECS` (120), once per episode. Summary `"Printer C364Series"`; bodies `"Job \"fattura.pdf\" stopped: <state message or reason>"`, `"<reason as words>"` (`media-jam-error` gives `media jam`), `"Not answering: <state message>"`.

- [ ] **Step 1: Failing tests:** `poll_sends_only_changes` (fake: first call Some, second None, after a job is added Some); `watcher_logs_unavailable_once` (fake with `fail`: first `Unavailable`, second None; after `fail` cleared a `Snapshot`); `hidden_without_jobs_or_errors`; `badge_counts_active_jobs` (2 jobs, one Completed is not in the list anyway, one Held counts); `error_look_for_error_reason_and_stopped_job`; `owner_paused_is_grey_not_red` (Stopped, empty message, 1 job: `Paused`; no jobs: `Hidden`); `warning_does_not_make_error` (`toner-low-warning`, 1 job: `Normal`); `notifies_once_per_error_episode` (Review Focus 4: error at t=0 -> 1 note, t=2 -> none, cleared t=4 -> none, back t=6 -> 1 note); `offline_notifies_after_two_minutes` (t=0 none, t=119 none, t=120 one, t=200 none, recovered then offline again from t=300: one note at t=420).
- [ ] **Step 2-4:** fail, implement, pass + clippy.
- [ ] **Step 5:** Commit `printing: queue watcher and tray state`.

### Task 6: `opusdm-printer-config` worker and edit session

**Files:** Create `crates/printer-config/Cargo.toml`, `src/lib.rs`, `src/worker.rs`, `src/session.rs`; modify workspace `Cargo.toml`.

**Interfaces:**
- Consumes: Task 2 model, Task 4 `PrintBackend`, `FakeBackend`.
- Produces:
  - `pub enum Job { Refresh, LoadOptions(String), Devices, Drivers, Add(Box<NewPrinter>), Save { printer: String, edit: PrinterEdit }, Remove(String), SetDefault(String), SetEnabled(String, bool), TestPage(String), Cancel(i32), CancelAll(String), Hold(i32), Release(i32), Retry(String, Option<i32>) }`.
  - `pub enum Done { Refreshed(Snapshot), Options(String, Vec<PrinterOption>), Devices(Vec<Device>), Drivers(Vec<Driver>), Added(String), Saved(String), Acted(String), Failed { what: String, error: PrintError, refresh: bool } }`. `Acted` carries the info line (`"Job 41 canceled"`, `"C364Series is the default printer"`, ...). Every action that succeeds or fails on a job or printer is followed by an automatic `Refreshed` from the worker (`refresh: true` on failure: Review Focus 3).
  - `pub const SAVING: &str = "Saving"` and `Done::is_failed_save(&self) -> bool`, as in network-config.
  - `pub fn run_job(backend: &dyn PrintBackend, job: Job) -> Vec<Done>` and `pub fn spawn(backend: Arc<dyn PrintBackend>, done: async_channel::Sender<Done>) -> mpsc::Sender<Job>` (thread `opusdm-printer-worker`, ends when either side is dropped).
  - `session.rs`: `#[derive(Clone, Debug, PartialEq)] pub struct EditSession { pub printer: String, saved_info, saved_location: String, saved_options: Vec<(String, String)>, pub info: String, pub location: String, pub options: Vec<(String, String)> }`; `EditSession::new(&Printer) -> Self`; `set_options(&mut self, &[PrinterOption])` (sets saved and current from the defaults, keeps the user's current changes to info/location); `set_option(&mut self, key, value)`; `is_dirty(&self) -> bool`; `edit(&self) -> PrinterEdit` (only changed options); `revert(&mut self)`; `mark_saved(&mut self)`.

- [ ] **Step 1: Failing tests:** worker: `an_action_reports_and_refreshes` (Cancel(41) on a fake with job 41 gives `[Acted(..), Refreshed(s)]` with the job gone); `a_failed_queue_action_refreshes` (Review Focus 3: fail set to "client-error-not-found" on Cancel gives `Failed { refresh: true, .. }` then `Refreshed` ... the fake fails every call while `fail` is set, so the test clears `fail` with a fake flag `fail_once: Mutex<bool>`; add that field to `FakeBackend` in Task 4 if missing); `only_a_failed_save_is_a_failed_save`; `the_worker_runs_jobs_in_order`. Session: `new_session_is_clean`, `changed_option_makes_dirty_and_edit_lists_only_it`, `revert_and_mark_saved`, `options_loaded_after_info_edit_keep_the_edit`.
- [ ] **Step 2-4:** fail, implement, pass + clippy.
- [ ] **Step 5:** Commit `printer-config: worker and edit session`.

### Task 7: wizard state

**Files:** Create `crates/printer-config/src/wizard.rs`.

**Interfaces:**
- Produces:
  - `pub enum Step { Device, Driver, Name, Done }`.
  - `pub struct Wizard { pub step: Step, pub devices: Vec<Device>, pub device: Option<Device>, pub manual_uri: String, pub driver: Option<DriverChoice>, pub driverless: bool, pub name: String, pub info: String, pub location: String, pub make_default: bool, taken: Vec<String> }`.
  - `Wizard::new(taken: Vec<String>, configured_uris: Vec<String>) -> Self`; `is_configured(&self, uri) -> bool`.
  - `choose_device(&mut self, Device)` (sets `driverless = is_driverless_uri`, `driver = Some(Driverless)` when driverless else None, `name = suggest_queue_name(make_model or info)`, `info = device.info`); `choose_manual(&mut self, uri: &str)`.
  - `uri(&self) -> Option<String>` (device uri, or the trimmed manual uri when it contains `://`).
  - `can_next(&self) -> Result<(), String>`: Device needs `uri()` ("Choose a printer or type its address"); Driver needs `driver` ("Choose a driver"); Name needs `validate_queue_name` and the name not taken ("A printer with this name exists").
  - `next(&mut self)`, `back(&mut self)`; `request(&self) -> Option<NewPrinter>` (Name step valid).
- [ ] **Step 1: Failing tests:** `bizhub_is_not_driverless_and_gets_a_name`, `ipp_device_preselects_driverless`, `manual_uri_needs_a_scheme`, `next_is_blocked_with_a_reason`, `taken_names_are_refused`, `request_from_a_full_wizard`.
- [ ] **Step 2-4:** fail, implement, pass + clippy.
- [ ] **Step 5:** Commit `printer-config: add printer wizard state`.

### Task 8: the window, the tabs and the standalone program

**Files:** Create `src/pages.rs`, `src/window.rs`, `src/main.rs`, `src/gtk_test.rs`.

**Interfaces:**
- Consumes: Tasks 4-7.
- Produces:
  - `pub fn window::open(app: &gtk::Application)`: window title "Printers", 920x640, application id `org.opusdm.PrinterManager`.
  - `pub fn window::embedded(window: &gtk::Window) -> Rc<PrinterTool>`, `PrinterTool: EmbeddedTool` (title "Printers"; `save` sends `Job::Save` and answers `done` when `Saved` or the failed save comes back; `revert` reverts the session and redraws).
  - `fn build_with(window: &gtk::Window, backend: Arc<dyn PrintBackend>) -> (Rc<App>, gtk::Box)` used by both and by the test with `FakeBackend`.

Ownership (write it in the commit body): the program window owns `Rc<App>` through `keep_until_closed` (same as network-config); in Settings, `PrinterTool` owns it and the Advanced tab owns the tool for the life of the window. Every callback and the `glib::spawn_future_local` loops hold `Weak<App>`; a failed upgrade in a loop logs `err_log!("opusdm-printer-manager", "the printer window is gone: <loop> stops")` once and ends. The watcher thread (`watch::spawn`) ends when its receiver (owned by the loop) is dropped; the worker ends when `App` (its sender) is dropped.

Layout (option A, approved): `gtk::Paned`; left: `ListBox` of printers (name with a star for the default, a dim line with state and `"N jobs"`), buttons `+` "Add a printer", `-` "Remove the printer"; right: `gtk::Notebook` with "General", "Options", "Queue (N)"; message line at the bottom (`show_info` / `show_error` like network-config, `error` CSS class, `APP_CSS` copied from network-config).
- General: labeled rows Name (read only), Description (entry), Location (entry), Model, Address, State (state + message + reasons with severity); buttons "Set as default" (insensitive when it is), "Disable"/"Enable", "Print test page"; Save / Revert buttons (sensitive when dirty). A note "Your own options in ~/.cups/lpoptions override these defaults" when that file exists.
- Options: `ScrolledWindow`; common options as rows with a `gtk::DropDown` of the choices; the rest in a `gtk::Expander` "More options (N)"; loaded with `Job::LoadOptions` when a printer is selected ("Loading options..." meanwhile); edits go to the session.
- Queue: a `ListBox` of the printer's active jobs (id, document, user, state label, reasons); buttons "Cancel job", "Hold", "Resume", "Cancel all" (with a confirmation), "Retry" when the printer or the job is stopped. Tab label "Queue (N)".
- The list and the queue follow `watch::spawn` snapshots (the same watcher as the panel), so a job printed elsewhere appears; after every action the worker refreshes too.
- Selecting another printer or closing with a dirty session asks Save / Discard / Cancel (like network-config `ask_before_leaving`); removing asks "Remove PRINTER?" with detail "Its waiting jobs are canceled.".

- [ ] **Step 1: Failing GTK test** in `window.rs` (runs through `gtk_test::run`, skipped without a display): `the_window_stays_alive_and_follows_the_fake` builds `build_with` on a `gtk::Window` with a fake holding two printers and one job, iterates the main context until the list has 2 rows (at most 2 s), then adds a third printer to the fake, sends a refresh and iterates until 3 rows: the loops are still alive after many iterations (CLAUDE.md rule 5). Second test `cancel_from_the_queue_reaches_the_backend`: selects the printer with the job, calls the queue cancel action, iterates, checks `fake.calls()` contains `cancel 41`.
- [ ] **Step 2:** run, fail. **Step 3:** implement `pages.rs`, `window.rs`, `main.rs` (as network-config `main.rs`: second launch presents the window).
- [ ] **Step 4:** `cargo test -p opusdm-printer-config` passes (with `DISPLAY` set), clippy clean; `cargo run -p opusdm-printer-config --bin opusdm-printer-manager` shows `C364Series` with its state, its options (the Konica ones, common first), and an empty queue.
- [ ] **Step 5:** Commit `printer-config: printers window with General, Options and Queue` (body: ownership answer above).

### Task 9: the wizard dialog

**Files:** Create `src/wizard_ui.rs`; modify `window.rs` (`+` opens it).

**Interfaces:**
- Consumes: `Wizard` (Task 7), worker jobs `Devices`, `Drivers`, `Add`.
- Produces: `pub fn wizard_ui::open(parent: &gtk::Window, jobs: <the App's sender helper>, ...)` - the builder picks the exact wiring; the dialog owns its `Rc<WizardUi>` through its own close handler (same pattern as `keep_until_closed`), and the App routes `Done::Devices`, `Done::Drivers`, `Done::Added` and failures to the open wizard through a `RefCell<Option<Weak<WizardUi>>>` (a failed upgrade logs and is ignored: the wizard was closed).

Pages in a `gtk::Stack`, buttons Back / Next (Finish on Name) / Cancel; the reason of `can_next()` under the buttons.
- Device: "Searching the network..." then the list of devices (info, make-and-model, uri dim; "already added" for configured uris), "Search again", and "Address" entry for a manual uri with a hint (`ipp://host/ipp/print`, `socket://host:9100`, `lpd://host/queue`).
- Driver: radio "Driverless (IPP Everywhere)" (only when `driverless`), radio "Driver from the list" with a search entry prefilled with the device make-and-model words and a list of at most 200 matches (`filter_drivers`), radio "PPD file..." with a file picker (`opusdm_ui::pick_file_or_error`, filter `*.ppd`, `*.ppd.gz`). The driver list is asked once per wizard ("Loading drivers..."; about 3.5 s here).
- Name: name (prefilled), description, location, "Set as default".
- Done: "PRINTER was added." and "Print test page" (sends `Job::TestPage`), Close.

- [ ] **Step 1:** GTK test `wizard_adds_a_printer_through_the_backend`: opens the wizard on the fake (one ipp device), simulates choose device, Next, Next (driverless preselected), Finish; iterates; `fake.calls()` contains `add <suggested name>` and the window list shows it.
- [ ] **Step 2-4:** fail, implement, pass, clippy clean.
- [ ] **Step 5:** Commit `printer-config: add printer wizard`.

### Task 10: Settings entry, packaging, first live check

**Files:** Modify `crates/theme-config/Cargo.toml` (dependency `opusdm-printer-config`), `crates/theme-config/src/advanced.rs` (`ENTRIES` becomes 5 with `Entry { label: "Printers", build: build_printers }` last, `fn build_printers(window) -> Rc<dyn EmbeddedTool> { opusdm_printer_config::window::embedded(window) }`), `scripts/build-release.sh` (`BINARIES` and the desktop entry pairs get `opusdm-printer-manager` / `PrinterManager:printer-manager`, comment line 7), `scripts/live-update.sh` (`PROCESSES`), `CHANGES.md`.

- [ ] **Step 1:** `cargo build --workspace` and `cargo test --workspace` pass; `cargo clippy --workspace` gives no new warning.
- [ ] **Step 2: Live check (debug build, real session, nothing installed):** `target/debug/opusdm-settings` -> Advanced -> Printers shows `C364Series`; edit Description of a probe queue, switch entry: the unsaved question appears; Save works (`lpstat -l -p opusdm_probe` shows the new description). Wizard: add a manual `socket://127.0.0.1:9` queue `opusdm_probe` with a driver from the list; print test page; the job shows in Queue; Hold/Resume/Cancel work; Remove removes it. `lpstat -v` shows only `C364Series` at the end. Also the other Advanced entries still open (Hotkeys, File Types, Input, Network).
- [ ] **Step 3:** CHANGES.md entry (what was checked, what was not: the wizard on the real bizhub was not used, to leave its queue as it is).
- [ ] **Step 4:** Commit `settings: Printers entry in Advanced` (small, revertable).

### Task 11: panel printer center (high risk, own commit)

**Files:** Create `crates/panel/src/printer_center.rs`, `crates/panel/src/widgets/printer.rs`; modify `crates/panel/src/widgets/mod.rs`, `crates/panel/src/lib.rs` (field `printer: Rc<printer_center::PrinterCenter>` in `BarServices`, created in `attach` next to `network`, cloned in `rebuild`, `trays.append(&printer.button())` right after `network.button()`), `crates/panel/Cargo.toml` (`opusdm-printing`).

**Interfaces:**
- Consumes: `watch::spawn`, `tray::{view, Notifier, TrayView, Look}`, `CupsBackend`, `PrintBackend` actions.
- Produces: `PrinterCenter::new() -> Rc<Self>`, `button(self: &Rc<Self>) -> gtk::Widget`, `snapshot(&self) -> Snapshot`, `view(&self) -> TrayView`, `send(self: &Rc<Self>, action: Action)` with `pub enum Action { Cancel(i32), Hold(i32), Release(i32), Retry(String, Option<i32>) }` run on a worker thread like `network_center::spawn_worker`; failures go to a `TransientMessage`.

Ownership (write in the commit body): `attach` owns `Rc<PrinterCenter>` for the whole panel run (like `network`); the center holds strong `Rc<PrinterButton>`s pruned when detached (like `NetworkCenter::buttons`); the watcher loop holds the center strongly, exactly as `NetworkCenter::new` does, so it lives as long as the panel; if the watcher channel closes the loop logs `err_log!("opusdm-panel", "printer tray: the CUPS watcher stopped")`.

Button: `gtk::MenuButton` (flat) with a 20 px printer icon (`printer-symbolic` from the theme; red tint with the `error` look, dim with `Paused`) and a badge label with the job count; hidden for `Look::Hidden`. Popover (300 px): per printer with jobs a heading with its state message, rows `id name state` with buttons Cancel / Hold or Resume / Retry (on stopped), the transient error line, and "Manage printers..." which starts `opusdm-printer-manager` like `widgets/network.rs::open_manager`. Notifications: every `Note` from `Notifier::update` is sent with `org.freedesktop.Notifications.Notify` on a short thread (same code shape as `agenda_center::notify`, urgency normal = 1, app name `OpusDM`, icon `printer`), a failure logs.

- [ ] **Step 1:** Unit test in `printer_center.rs` for the action worker on `FakeBackend` (`run_action` reports a failure as text; success is silent), like the network center tests.
- [ ] **Step 2-4:** fail, implement, pass; `cargo build --workspace`, `cargo test --workspace`, clippy no new warning.
- [ ] **Step 5: Live check in the sandbox first** (`./run-x11.sh` or Xephyr script): the panel shows no printer icon with an empty queue; `lpadmin` a probe queue on `socket://127.0.0.1:9`, `lp` a file: the icon appears with badge 1, the popover lists it, Cancel removes it and the icon hides. `cupsdisable -r "test" opusdm_probe` + a job: red icon + one notification. The other tray icons (devices, audio, battery, network) and the clock still work. Then the 2 minute offline note (a job on the probe queue, wait 2 min). Remove the probe queue.
- [ ] **Step 6:** Commit `panel: printer icon with the print queue and error notifications` (body: ownership answer).
- [ ] **Step 7: Live check in the running session** after `scripts/live-update.sh --all` (needs sudo: ask the owner) with the CLAUDE.md rule 4 checklist (Lister keys, global hotkeys, hub second Lister / Copy job). If not possible, say so in CHANGES.md and in the final message.

### Task 12: wrap up

- [ ] CHANGES.md complete for parts 1 and 2 with what was not verified; `cargo test --workspace` and clippy once more; final commit of CHANGES.md if separate.
