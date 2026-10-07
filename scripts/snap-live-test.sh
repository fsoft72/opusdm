#!/usr/bin/env bash
#
# Run the live snap test (crates/snap/tests/live.rs) on a nested X server:
# starts Xephyr, Openbox and, when it is installed, picom (without a
# compositor the zone preview is not checked), runs the ignored test and
# stops everything.
#
# Usage: scripts/snap-live-test.sh [display] [resolution]
#   display     nested X display, default :98
#   resolution  default 1280x800

set -euo pipefail

DISPLAY_NUM="${1:-${SNAP_TEST_DISPLAY:-:98}}"
RESOLUTION="${2:-1280x800}"
READY_TIMEOUT_TENTHS=50
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

command -v Xephyr > /dev/null 2>&1 || { echo "Xephyr not found (package: xserver-xephyr)"; exit 1; }
command -v openbox > /dev/null 2>&1 || { echo "openbox not found"; exit 1; }
command -v xdpyinfo > /dev/null 2>&1 || { echo "xdpyinfo not found (package: x11-utils)"; exit 1; }

CHILD_PIDS=()

_cleanup() {
	local pid
	for pid in "${CHILD_PIDS[@]:-}"; do
		[ -n "$pid" ] && kill "$pid" 2> /dev/null || true
	done
}
trap _cleanup EXIT INT TERM

# Wait until the nested server answers, or give up.
_wait_for_server() {
	local i
	for ((i = 0; i < READY_TIMEOUT_TENTHS; i++)); do
		DISPLAY="$DISPLAY_NUM" xdpyinfo > /dev/null 2>&1 && return 0
		sleep 0.1
	done
	echo "Xephyr did not start on $DISPLAY_NUM"
	return 1
}

Xephyr "$DISPLAY_NUM" -screen "$RESOLUTION" -ac > /dev/null 2>&1 &
CHILD_PIDS+=("$!")
_wait_for_server

DISPLAY="$DISPLAY_NUM" openbox --config-file "$ROOT_DIR/openbox-rc.xml" > /dev/null 2>&1 &
CHILD_PIDS+=("$!")

if command -v picom > /dev/null 2>&1; then
	DISPLAY="$DISPLAY_NUM" picom --config "$ROOT_DIR/picom.conf" > /dev/null 2>&1 &
	CHILD_PIDS+=("$!")
else
	echo "picom not found: the zone preview will not be checked"
fi
sleep 1

cd "$ROOT_DIR"
DISPLAY="$DISPLAY_NUM" cargo test -p opusdm-snap --test live -- --ignored --nocapture --test-threads=1
