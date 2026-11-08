#!/bin/sh
# Capture the state of frozen OpusDM processes, for a post-mortem.
#
# Usage: scripts/freeze-report.sh [PID ...]
#   (no PID)  every opusdm-* process of the current user (hub, terminals,
#             Listers, dock, ...).
#   PID ...   only these processes (any program, e.g. a frozen terminal).
#
# Run it WHILE the freeze is happening, from a terminal that still works (the
# global hotkeys keep working when the hub's GTK thread is stuck, so
# Ctrl+backslash opens one). Nothing is killed or changed: gdb attaches,
# prints the stacks and detaches.
#
# Writes one folder ${XDG_STATE_HOME:-~/.local/state}/opusdm/freeze-reports/<time>/
# with:
#   summary.txt     process list, per-thread state (wchan, syscall) of each
#                   process, picom and Xorg state, recent session log and
#                   kernel journal.
#   <comm>-<pid>.txt  full stacks of every thread (gdb, else eu-stack).
#
# Needs gdb (or eu-stack from elfutils) and ptrace permission on own
# processes (/proc/sys/kernel/yama/ptrace_scope 0); without them only the
# summary is written, and the reason is logged in it.

set -u

GDB_TIMEOUT=60
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/opusdm/freeze-reports"
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$STATE_DIR/$STAMP"

# Print an error and exit.
die() {
	echo "freeze-report: $*" >&2
	exit 1
}

# The pids to inspect: the arguments, or every opusdm-* process of the user.
target_pids() {
	if [ "$#" -gt 0 ]; then
		echo "$@"
		return
	fi
	pgrep -u "$(id -u)" '^opusdm-' | tr '\n' ' '
}

# One line per thread of process $1: tid, name, wchan, syscall.
thread_states() {
	for task in /proc/"$1"/task/*; do
		[ -d "$task" ] || continue
		tid=${task##*/}
		comm=$(cat "$task/comm" 2>/dev/null)
		wchan=$(cat "$task/wchan" 2>/dev/null)
		sysc=$(cut -d' ' -f1-4 "$task/syscall" 2>/dev/null)
		printf '  %-8s %-18s %-34s %s\n' "$tid" "$comm" "$wchan" "$sysc"
	done
}

# Write the stacks of every thread of process $1 to file $2.
dump_stacks() {
	if command -v gdb >/dev/null 2>&1; then
		timeout "$GDB_TIMEOUT" gdb -p "$1" -batch -nx \
			-ex 'set pagination off' -ex 'info threads' -ex 'thread apply all bt 50' \
			>"$2" 2>&1 && return 0
		echo "gdb failed or timed out (exit $?), trying eu-stack" >>"$2"
	fi
	if command -v eu-stack >/dev/null 2>&1; then
		timeout "$GDB_TIMEOUT" eu-stack -p "$1" >>"$2" 2>&1 && return 0
		echo "eu-stack failed or timed out (exit $?)" >>"$2"
		return 1
	fi
	echo "no stack dumper found: install gdb or elfutils" >>"$2"
	return 1
}

mkdir -p "$OUT" || die "cannot create $OUT"
PIDS=$(target_pids "$@")
[ -n "$PIDS" ] || die "no process to inspect (no opusdm-* process running?)"

SUMMARY="$OUT/summary.txt"
{
	echo "freeze report $STAMP on $(hostname)"
	echo "ptrace_scope: $(cat /proc/sys/kernel/yama/ptrace_scope 2>/dev/null || echo unknown)"
	echo
	echo "== processes"
	# shellcheck disable=SC2086
	ps -o pid,ppid,lstart,stat,wchan:32,comm -p $(echo $PIDS | tr ' ' ',')
	echo
	for pid in $PIDS; do
		[ -d /proc/"$pid" ] || { echo "== $pid: gone"; continue; }
		echo "== $(cat /proc/"$pid"/comm) $pid threads"
		thread_states "$pid"
		echo
	done
	for name in picom Xorg openbox; do
		for pid in $(pgrep -x "$name"); do
			echo "== $name $pid main thread: $(cat /proc/"$pid"/wchan 2>/dev/null) $(cut -d' ' -f1-4 /proc/"$pid"/syscall 2>/dev/null)"
		done
	done
	echo
	echo "== last lines of ~/.xsession-errors"
	tail -n 60 "$HOME/.xsession-errors" 2>/dev/null
	echo
	echo "== journal, last 15 minutes"
	journalctl --since "-15min" --no-pager 2>&1 | tail -n 150
} >"$SUMMARY" 2>&1

for pid in $PIDS; do
	[ -d /proc/"$pid" ] || continue
	comm=$(cat /proc/"$pid"/comm 2>/dev/null)
	echo "freeze-report: stacks of $comm $pid"
	dump_stacks "$pid" "$OUT/$comm-$pid.txt" || echo "freeze-report: no stacks for $comm $pid, see $OUT/$comm-$pid.txt" >&2
done

echo "freeze-report: written $OUT"
