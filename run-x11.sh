#!/usr/bin/env bash
#
# Launch a nested X server (Xephyr) on a chosen display and run opusdm-hub
# inside it. The hub spawns the first Lister window itself. Useful as a
# sandbox for testing OpusDM without touching the real session.
#
# Usage:
#   ./xephyr-sandbox.sh [display] [resolution]
#
# Examples:
#   ./xephyr-sandbox.sh              # :1 at 1280x800
#   ./xephyr-sandbox.sh :2 1920x1080

set -euo pipefail

# Configuration (overridable via args or environment)
DISPLAY_NUM="${1:-${XEPHYR_DISPLAY:-:1}}"
RESOLUTION="${2:-${XEPHYR_RES:-1920x1080}}"
TERMINAL="${XEPHYR_TERM:-xterm}"
START_WM="${XEPHYR_WM:-openbox}"   # WM providing decorations; set empty to disable
OPENBOX_CONFIG="${XEPHYR_OB_CONFIG:-$(pwd)/openbox-rc.xml}"  # rc.xml for openbox
READY_TIMEOUT_TENTHS=50            # 50 * 0.1s = 5s max wait for the server

# Exit conditions up front
command -v Xephyr >/dev/null 2>&1 || { echo "Xephyr not found (package: xserver-xephyr)"; exit 1; }
command -v "$TERMINAL" >/dev/null 2>&1 || { echo "Terminal '$TERMINAL' not found"; exit 1; }

CHILD_PIDS=()

# Kill every process we spawned when the script exits
_cleanup() {
    local pid
    for pid in "${CHILD_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
}
trap _cleanup EXIT INT TERM

# Wait until the nested server answers, or give up
_wait_for_server() {
    local i
    for ((i = 0; i < READY_TIMEOUT_TENTHS; i++)); do
        if DISPLAY="$DISPLAY_NUM" xdpyinfo >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    echo "Xephyr on $DISPLAY_NUM did not become ready in time"
    return 1
}

# Start the nested X server
echo "Starting Xephyr on $DISPLAY_NUM ($RESOLUTION)"
Xephyr "$DISPLAY_NUM" \
    -ac \
    -screen "$RESOLUTION" \
    -glamor \
    -resizeable \
    -title "Xephyr $DISPLAY_NUM" &
XEPHYR_PID=$!
CHILD_PIDS+=("$XEPHYR_PID")

_wait_for_server

# Optional bare window manager so windows can be moved and focused
if [ -n "$START_WM" ]; then
    if command -v "$START_WM" >/dev/null 2>&1; then
        echo "Starting window manager: $START_WM"
        WM_ARGS=()
        # Use the sandbox openbox config (Amiga-Workbench theme) when present
        if [ "$START_WM" = "openbox" ] && [ -f "$OPENBOX_CONFIG" ]; then
            WM_ARGS=(--config-file "$OPENBOX_CONFIG")
            echo "  using config: $OPENBOX_CONFIG"
        fi
        DISPLAY="$DISPLAY_NUM" "$START_WM" "${WM_ARGS[@]}" &
        CHILD_PIDS+=("$!")
    else
        echo "Window manager '$START_WM' not found, skipping"
    fi
fi

# Build the workspace before entering the sandbox
echo "Building opusdm (release)"
cargo build --release --workspace

# Launch the hub inside the nested server; it spawns the first Lister itself
HUB_BIN="$(pwd)/target/release/opusdm-hub"
if [ ! -x "$HUB_BIN" ]; then
    echo "opusdm-hub not built at $HUB_BIN"
    exit 1
fi
echo "Starting opusdm-hub"
# Give the hub its own runtime dir so a stale socket never blocks the sandbox
export XDG_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-xephyr-XXXXXX)"
DISPLAY="$DISPLAY_NUM" "$HUB_BIN" &
CHILD_PIDS+=("$!")

# Optional extra terminal for poking around
if [ "${XEPHYR_TERM_EXTRA:-0}" = "1" ]; then
    DISPLAY="$DISPLAY_NUM" "$TERMINAL" &
    CHILD_PIDS+=("$!")
fi

# Keep running until Xephyr window is closed
wait "$XEPHYR_PID"
