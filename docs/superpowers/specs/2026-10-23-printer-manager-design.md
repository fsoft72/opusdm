# Printer manager - design

Date: 2026-10-23.

## Goal

Manage printers from OpusDM, like the "Printers" module of KDE Plasma System
Settings, so the owner no longer opens the KDE settings to add a printer (the
last time was to add the Konica Minolta bizhub C224e). Two parts, built and
checked live one at a time, in this order:

1. **Printer manager** (`opusdm-printer-manager`): list, add, edit and remove
   CUPS printers, with the print queue of each printer.
2. **Printer center in the panel tray**: an icon while jobs are queued or a
   printer is in error, a popup with the queue, a notification on errors.

Deferred to a later design (the owner asked to keep the bizhub scanner out of
this work): scan to folder from the printer (SMB share, Samba setup, bizhub
authentication and the PC address) and scanning started from the PC. Today
SANE does not see the bizhub (`airscan-discover` and `scanimage -L` find only
the webcam). Both will share one "scan folder" setting.

Out of scope: sharing printers to other PCs, printer classes, editing the
bizhub address book (done in its web page).

## Facts about the machine (checked 2026-10-23)

- CUPS is active; `lpstat`, `lpadmin`, `lpinfo` and `libcups.so.2` exist.
- The owner is in the `lpadmin` group: `lpinfo -v` runs without a password,
  so CUPS admin calls should not need sudo or polkit. The builder confirms
  this for `lpadmin -p ... -E` before relying on it.
- One printer: `C364Series`, device
  `dnssd://KONICA%20MINOLTA%20bizhub%20C224e(98%3A1F%3A7C)._pdl-datastream._tcp.local/`,
  vendor PPD, no system default destination.

## Precedent to follow

`crates/network-config` (`opusdm-network-manager`) is the model: a crate with
a lib and a standalone binary, a worker thread for every system call, and an
`EmbeddedTool` (`crates/ui/src/embedded_tool.rs`) shown in the Advanced tab of
`opusdm-settings` (`crates/theme-config/src/advanced.rs`, the `ENTRIES`
array). The panel network tray (`crates/panel/src/network_center.rs`,
`widgets/network.rs`, `MANAGER_BIN`) is the model for part 2. See
`docs/superpowers/specs/2026-10-12-network-manager-design.md`.

## Part 1 - Printer manager

### Where it lives

- New crate `crates/printer-config` (`opusdm-printer-config`), lib
  `opusdm_printer_config` and bin `opusdm-printer-manager`.
- New entry "Printers" at the end of `ENTRIES` in the Advanced tab, built on
  first use like the others. Nothing else in `opusdm-settings` changes.
- CUPS access goes through a backend trait (list printers, jobs, add, modify,
  remove, set default, enable/disable, job cancel/hold/release, discover
  devices, list drivers) with a fake for tests, like `ProfileBackend` and
  `FakeProfiles` in `crates/network`. Builder's call: whether the real
  backend uses the CUPS command line tools (`lpstat`, `lpadmin`, `lpinfo`,
  `lpoptions`, `cancel`, `lp -H`) or libcups/IPP. It may live in a small
  non-GTK crate (for example `crates/printing`) so part 2 can share it.
- All CUPS calls run off the GTK thread (worker, as in `network-config`), and
  every failure is shown in the message line and logged with `err_log!`.

### Layout (option A of the mockups, approved)

- Left: the list of CUPS printers. Each row shows the name, the state and the
  number of queued jobs; the default printer has a star. Under the list, `+`
  (add) and `-` (remove, with a confirmation).
- Right: three tabs for the selected printer.
  - **General**: name, description, location, model, device address, state
    and state message. Buttons: Set default, Enable/Disable, Print test page.
  - **Options**: the default job options. Which ones are shown is decided
    from what the driver declares (at least paper size, duplex, color mode,
    quality when the driver has them). Builder's call how to map PPD/IPP
    options to widgets.
  - **Queue (n)**: the jobs of this printer (id, document, user, state, and
    the printer's state message). Buttons: Cancel job, Hold, Resume, Cancel
    all. Refreshed while the tab is visible (builder's call: polling interval
    or CUPS notifications).
- A message line at the bottom, as in Network.

### Save and immediate actions

- Field edits (name/description/location, Options) are a draft: `is_dirty`,
  Save and Cancel, `confirm_leave` when leaving the entry, as the other tools.
- Actions take effect at once and are not part of the draft: add, remove, set
  default, enable/disable, test page, every queue action.
- Renaming a CUPS queue is not supported by CUPS directly; builder's call
  (likely: name is read only, description is editable).

### Add printer wizard (the `+` button, approved as in the mockup)

One page at a time, Back / Next / Cancel:

1. **Device**: devices found on the network (`lpinfo -v` / dnssd), with
   Search again, plus "Enter address by hand" (IPP, IPPS, socket, LPD URI).
   Devices already configured are marked.
2. **Driver**: Driverless (IPP Everywhere) when the device supports it,
   preselected; or make/model from the driver list; or a PPD file.
3. **Name**: queue name (valid CUPS name, suggested from the model),
   description, location, "Set as default".
4. **Done**: the printer is added and enabled; button Print test page.

## Part 2 - Printer center in the panel tray

- A `printer_center` in `crates/panel`, built like `network_center`: one
  shared state owned by `attach`, so a monitor hotplug rebuild keeps it.
- The icon is shown only while at least one job is queued or a printer is in
  error. Badge with the number of jobs; red icon and "!" on errors (job
  stopped, printer stopped, state reasons such as paper jam).
- Click: a popup with the jobs grouped by printer, the printer state message,
  buttons Cancel / Hold / Resume (Retry on a stopped job or printer), and
  "Manage printers..." which launches `opusdm-printer-manager`.
- Notification through the OpusDM notification server when a job stops or a
  printer goes into error, once per event (not on every refresh).
- Watching CUPS must never block the GTK thread and must not cost CPU when
  nothing is printed. Builder's call: CUPS notifications (`dbus` notifier or
  IPP subscriptions) or a light poll that slows down when the queue is empty.
- **High risk** (the panel runs in the daily session, see `CLAUDE.md`):
  separate commit, ownership of the center written in the commit body, and a
  live check of the panel (other tray icons still work, monitor hotplug, the
  icon appears and goes away with a real print job).

## Decisions made after the builder check (designer's calls, owner to confirm)

- **Defaults are system wide**, as in KDE: Options use `lpadmin -o`, Set
  default uses `lpadmin -d`. `~/.cups/lpoptions` does not exist today; if it
  appears, the General tab says that per-user options override the server
  ones.
- **The existing `C364Series` queue is left untouched.** The wizard is checked
  live by adding a temporary queue (driverless when the bizhub offers it) and
  removing it afterwards. The Options tab must cope with the large Konica
  vendor PPD of `C364Series`.
- **If CUPS admin calls need authentication**, they are run through `pkexec`
  (the `opusdm-polkit` agent asks for the password). `crates/polkit` is only
  the agent, so this means `pkexec` plus a small helper or policy, not a
  library call. The failure is shown, never silent. Checked first, before
  building the backend.
- **What the tray treats as an error**: a CUPS state reason ending in
  `-error`, a stopped job, or a printer stopped by CUPS while jobs wait. A
  printer the owner disabled on purpose is grey, not red, and does not
  notify. `-warning` / `-report` reasons (toner low) are shown in the popup,
  no notification. A printer that does not respond while jobs wait (the
  bizhub switched off) shows the normal badge and notifies once after it has
  not answered for 2 minutes.

## Constraints

- `CLAUDE.md` rules: behaviour of existing code unchanged, no silent failure
  (`err_log!`), regression tests for wiring, live checks before saying done,
  one risky change per commit, `CHANGES.md` updated including what was not
  verified.
- English in code and comments; no em dash.

## Left to the builder

The CUPS access method, the crate split, option-to-widget mapping and the
queue refresh strategy, as marked above.
