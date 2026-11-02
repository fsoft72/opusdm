# Firewall tool (ufw) - design

Date: 2026-10-30

## Intent

The owner wants to see from `opusdm-settings` which ports are open in and out,
and to open a new one, without a terminal. OpusDM has no firewall tool today.

## Decisions of the owner

- A new entry "Firewall" in the list of Settings > Advanced (after Bluetooth).
- The password is asked as soon as the entry is picked, and again for changes.
  No polkit rule or root daemon to avoid it.
- One tab per direction (IN, OUT), full width; "Add rule..." opens a dialog.
- Changes are queued; one "Apply" runs them all with one password.

## Goals

1. Show the firewall state, the default policies and the rules per direction.
2. Queue: add a rule, remove a rule, turn the firewall on or off.
3. Apply the queue in one privileged run, then show the real state.

## Non-goals (first version)

Changing the default policies, editing a rule (remove and add), logs, rate
limit rules in the form, port ranges and application profiles in the form, a
standalone binary, a panel icon.

## Anti-goals

- Showing "no rules" when the rules could not be read. A failed or cancelled
  read says so, with a Retry.
- A shell command built from text: rule arguments reach `ufw` as separate
  arguments, and only a fixed script runs as root.
- Touching the firewall without Apply. Picking the entry only reads.

## How it works

New library crate `crates/firewall-config` (`opusdm_firewall_config`), no
binary. `advanced.rs` gets one more entry through `window::embedded`.

### Facts checked on the real ufw 0.36.2

Run against a private `--rootdir` in a user namespace, never the live firewall:

- `ufw status` shows no rules while the firewall is off and needs iptables.
- `ufw show added` lists every rule in both states, IPv4 and IPv6 merged, one
  command per line, for example `ufw allow from 192.168.1.0/24 to any port 22
  proto tcp`, `ufw deny out 25/tcp`, `ufw allow 8080/tcp comment 'my app'`,
  `ufw route allow in on wg0 out on eth0`.
- `ufw --force delete <those words>` removes the rule, with no number that can
  shift. A route rule needs `ufw --force route delete <words>`.
- A repeated add and a delete of a missing rule both exit 0 (with a text). The
  re-read after the run shows the truth, so they are not reported as errors.
- `/etc/default/ufw` is world readable and holds the default policies, also
  while the firewall is off.

### The privileged run

One call for reading and applying:
`pkexec /bin/sh -c SCRIPT sh UFW DEFAULTS n1 a1 .. n1args n2 ..`. `SCRIPT` is a
constant. For each operation it takes the count and that many positional
arguments and runs `"$ufw" "$@"` without `eval`. It stops at the first failure,
then always prints `ufw status`, the defaults file and `ufw show added`, each
under a marker. So reading is an empty list of operations, and an Apply does
not ask a second password to refresh. The output is `LC_ALL=C`.

`pkexec` exit 126 means the dialog was dismissed, 127 not authorized or no
agent (its stderr is shown). `ufw` is looked up in `/usr/sbin` then `/sbin` as
an absolute path; when missing, the tool says so and never calls `pkexec`.

### Model

- A rule keeps the exact words from `show added` (used for the delete) plus
  the fields shown: port, proto, action, peer (From on IN, To on OUT), notes.
  Words the parser does not understand go to the notes, so nothing is hidden
  and everything can be removed. Route rules appear under IN, marked `route`.
  A rule without `out` is IN.
- The form checks: port 1-65535, optional IPv4 or IPv6 address or CIDR, proto
  tcp, udp or both, action allow or deny.
- The queue order is removals, additions, then on or off. After a partial run
  the operations done leave the queue; the failed one and the later ones stay,
  and the output of `ufw` is shown. No rollback: undoing automatically on a
  firewall can do more harm than the error.

### Screen

Header: state ("active", "not active", "active, will be turned off") and an
Enable or Disable button that queues the change (again: cancels it). A notebook
IN / OUT with the rule count, the default policy and a table: Port, Proto,
Action, From or To, Notes, a remove button. A queued addition is marked "to
add", its button drops it. A queued removal is struck through, its button
restores it. Bottom row: "Add rule...", Reload (disabled with a queue), a
message line, "Discard changes" and "Apply" (enabled with a queue).

### Settings integration

`EmbeddedTool`: dirty means the queue is not empty, `save` applies, `revert`
empties the queue. Save while a run is going fails with a message. The tool
owner is `Advanced.tools`, kept for the whole life of Settings; the report loop
holds the state weakly and logs with `err_log!` when it is gone.

## Calls made without the owner

The owner was away when these came up; both can be changed later.

- The password is asked on the first pick of the entry in a Settings run. Coming
  back shows the last read; Reload reads again. Every pick would need a new
  method on `EmbeddedTool`, shared with the other six tools.
- New rules go last and ufw uses the first match, so a deny after a broader
  allow has no effect. The table keeps the order of ufw and the dialog says so.
  No insert at a position: it brings back the rule numbers avoided above, and
  shadow detection is not in this version.

## Left to the builder

Widget details, names, wording of messages (English, as the rest of Settings).

## Verification

Unit tests: rule parsing with the real lines above, form validation, operation
arguments and order, queue after a partial run, the script against a fake `ufw`
with hostile arguments (spaces, quotes, `;`). A GTK test on a fake backend:
the tool survives the first read and main loop iterations and Apply still works
(rule 5). By hand: real ufw in the private rootdir, Settings in a Xephyr
sandbox. The real `pkexec` run on the live firewall is for the owner, who must
enter the password.
