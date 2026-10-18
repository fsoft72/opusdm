#!/usr/bin/env bash
#
# Launch a nested X server (Xephyr) on a chosen display and run opusdm-hub
# inside it. The hub spawns the first Lister window itself. Useful as a
# sandbox for testing OpusDM without touching the real session.
#
# Usage:
#   ./xephyr-sandbox.sh [--debug] [--grid-debug] [--config-dir DIR] [display] [resolution]
#
# Examples:
#   ./xephyr-sandbox.sh              # :1 at 1920x1080
#   ./xephyr-sandbox.sh :2 1920x1080
#   ./xephyr-sandbox.sh --debug      # lister debug log in /ramdisk/opusdm.log
#   ./xephyr-sandbox.sh --grid-debug # red box around every icon-view item
#   ./xephyr-sandbox.sh --config-dir ~/test-config
#                                    # use DIR as the OpusDM configuration
#                                    # (settings.json, hotkeys.json, ...) instead
#                                    # of ~/.config/opusdm; DIR must exist. The
#                                    # sandbox reads and writes only DIR (also
#                                    # picom.conf); XDG_CONFIG_HOME points to a
#                                    # private folder, so ~/.config/mimeapps.list
#                                    # is not touched either. Same as XEPHYR_CONFIG_DIR.

set -euo pipefail

DEBUG=0
GRID_DEBUG=0
CONFIG_DIR="${XEPHYR_CONFIG_DIR:-}"
ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --debug) DEBUG=1 ;;
        --grid-debug) GRID_DEBUG=1 ;;
        --config-dir)
            [ $# -ge 2 ] || { echo "--config-dir needs a folder"; exit 2; }
            CONFIG_DIR="$2"
            shift
            ;;
        *) ARGS+=("$1") ;;
    esac
    shift
done
set -- ${ARGS[@]+"${ARGS[@]}"}

# A relative --config-dir means relative to where the user started the script
if [ -n "$CONFIG_DIR" ]; then
    [ -d "$CONFIG_DIR" ] || { echo "Config folder '$CONFIG_DIR' is not a directory"; exit 1; }
    CONFIG_DIR="$(cd "$CONFIG_DIR" && pwd)"
fi

# Work from the repository root, wherever the script was started: the build,
# the config files and the binaries are all found from there.
cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")"

# Configuration (overridable via args or environment)
DISPLAY_NUM="${1:-${XEPHYR_DISPLAY:-:1}}"
RESOLUTION="${2:-${XEPHYR_RES:-1920x1080}}"
TERMINAL="${XEPHYR_TERM:-xterm}"
START_WM="${XEPHYR_WM:-openbox}"   # WM providing decorations; set empty to disable
OPENBOX_CONFIG="${XEPHYR_OB_CONFIG:-$(pwd)/openbox-rc.xml}"  # rc.xml for openbox
COMPOSITOR="${XEPHYR_COMPOSITOR:-picom}"   # compositor for shadows and transparency; set empty to disable
PICOM_CONFIG="${XEPHYR_PICOM_CONFIG:-$(pwd)/picom.conf}"  # picom.conf (GL backend)
READY_TIMEOUT_TENTHS=50            # 50 * 0.1s = 5s max wait for the server

# Exit conditions up front
command -v Xephyr >/dev/null 2>&1 || { echo "Xephyr not found (package: xserver-xephyr)"; exit 1; }
command -v "$TERMINAL" >/dev/null 2>&1 || { echo "Terminal '$TERMINAL' not found"; exit 1; }

# The sandbox is an X11 session even when started from a Wayland desktop:
# without this GTK (and OpusDM's session detection) would pick the host
# compositor instead of Xephyr.
unset WAYLAND_DISPLAY
export GDK_BACKEND=x11

# Use DIR as the OpusDM configuration: OpusDM reads $XDG_CONFIG_HOME/opusdm,
# so XDG_CONFIG_HOME becomes a private folder whose `opusdm` entry is DIR.
if [ -n "$CONFIG_DIR" ]; then
    XDG_CONFIG_HOME="$(mktemp -d /tmp/opusdm-xephyr-config-XXXXXX)"
    ln -s "$CONFIG_DIR" "$XDG_CONFIG_HOME/opusdm"
    export XDG_CONFIG_HOME
    echo "OpusDM configuration: $CONFIG_DIR (XDG_CONFIG_HOME=$XDG_CONFIG_HOME)"
fi

CHILD_PIDS=()

# Kill every process we spawned when the script exits
_cleanup() {
    local pid
    for pid in "${CHILD_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    # Only the symlink to the config folder lives here; rm never follows it
    [ -n "$CONFIG_DIR" ] && rm -rf "$XDG_CONFIG_HOME"
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
    -no-host-grab \
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
        DISPLAY="$DISPLAY_NUM" XDG_DATA_DIRS="$(pwd)/assets/openbox-share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}" "$START_WM" "${WM_ARGS[@]}" &
        CHILD_PIDS+=("$!")
    else
        echo "Window manager '$START_WM' not found, skipping"
    fi
fi

# Optional compositor (shadows, transparency, GL acceleration)
if [ -n "$COMPOSITOR" ]; then
    if command -v "$COMPOSITOR" >/dev/null 2>&1; then
        echo "Starting compositor: $COMPOSITOR"
        COMP_ARGS=()
        # Use the sandbox picom config when present
        if [ "$COMPOSITOR" = "picom" ] && [ -f "$PICOM_CONFIG" ]; then
            # Run on a per-user copy: the hub rewrites its opacity keys and
            # signals picom to reload it (Settings, Windows tab).
            PICOM_USER_CONF="${XDG_CONFIG_HOME:-$HOME/.config}/opusdm/picom.conf"
            mkdir -p "$(dirname "$PICOM_USER_CONF")"
            [ -f "$PICOM_USER_CONF" ] || cp "$PICOM_CONFIG" "$PICOM_USER_CONF"
            COMP_ARGS=(--config "$PICOM_USER_CONF")
            export OPUSDM_PICOM_BASE="$PICOM_CONFIG"
            echo "  using config: $PICOM_USER_CONF (base: $PICOM_CONFIG)"
        fi
        DISPLAY="$DISPLAY_NUM" "$COMPOSITOR" ${COMP_ARGS[@]+"${COMP_ARGS[@]}"} &
        CHILD_PIDS+=("$!")
    else
        echo "Compositor '$COMPOSITOR' not found (package: picom), skipping"
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
# Hotkeys such as exec:opusdm-run find the OpusDM binaries by name
export PATH="$(pwd)/target/release:$PATH"
# Draw a red box around every icon-view item (hub desktop and every Lister)
if [ "$GRID_DEBUG" = "1" ]; then
    echo "Grid debug overlay: on"
    export GRID_DEBUG=1
fi
# The sandbox never launches the Startup Apps: they belong to the real session
if [ "$DEBUG" = "1" ]; then
    LOG=/ramdisk/opusdm.log
    echo "Debug log: $LOG"
    OPUSDM_DEBUG=1 DISPLAY="$DISPLAY_NUM" "$HUB_BIN" --no-startup-apps >"$LOG" 2>&1 &
else
    DISPLAY="$DISPLAY_NUM" "$HUB_BIN" --no-startup-apps &
fi
CHILD_PIDS+=("$!")

# Optional extra terminal for poking around
if [ "${XEPHYR_TERM_EXTRA:-0}" = "1" ]; then
    DISPLAY="$DISPLAY_NUM" "$TERMINAL" &
    CHILD_PIDS+=("$!")
fi

# Keep running until Xephyr window is closed
wait "$XEPHYR_PID"
