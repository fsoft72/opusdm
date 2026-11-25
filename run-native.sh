#!/usr/bin/env bash
#
# Launch OpusDM's own Wayland compositor (opusdm-compositor) nested in a
# window of the current desktop and run opusdm-hub inside it. The sibling of
# run-wayland.sh (nested labwc) and run-x11.sh: a sandbox for testing the
# native compositor without touching the real session.
#
# Stage 1 of the compositor: windows, popups, layer shell, move / resize,
# maximize, fullscreen. No tasklist, pager, global hotkeys or minimize yet.
#
# Usage:
#   ./run-native.sh [--debug] [resolution]
#
# Examples:
#   ./run-native.sh               # 1920x1080
#   ./run-native.sh 1280x800
#   ./run-native.sh --debug       # debug log in /ramdisk/opusdm.log
#   NATIVE_LOG=/tmp/hub.log ./run-native.sh --debug   # debug log elsewhere

set -euo pipefail

DEBUG=0
ARGS=()
for a in "$@"; do
    case "$a" in
        --debug) DEBUG=1 ;;
        *) ARGS+=("$a") ;;
    esac
done
set -- ${ARGS[@]+"${ARGS[@]}"}

# Configuration (overridable via args or environment)
RESOLUTION="${1:-${NATIVE_RES:-1920x1080}}"
DEBUG_LOG="${NATIVE_LOG:-/ramdisk/opusdm.log}"   # hub output with --debug

# Exit conditions up front
command -v cargo >/dev/null 2>&1 || { echo "cargo not found (install Rust: https://rustup.rs)"; exit 1; }
[[ "$RESOLUTION" =~ ^[0-9]+x[0-9]+$ ]] || { echo "Bad resolution: $RESOLUTION (expected WxH, e.g. 1280x800)"; exit 1; }

# Build the workspace before entering the sandbox
echo "Building opusdm (release)"
cargo build --release --workspace

BIN_DIR="$(pwd)/target/release"
HUB_BIN="$BIN_DIR/opusdm-hub"
COMPOSITOR_BIN="$BIN_DIR/opusdm-compositor"
[ -x "$HUB_BIN" ] || { echo "opusdm-hub not built at $HUB_BIN"; exit 1; }
[ -x "$COMPOSITOR_BIN" ] || { echo "opusdm-compositor not built at $COMPOSITOR_BIN"; exit 1; }

# A config directory of its own holding the autostart script the compositor
# runs once its socket is up.
CONFIG_DIR="$(mktemp -d /tmp/opusdm-native-XXXXXX)"
# The hub gets its own runtime dir so a stale socket never blocks the
# sandbox; the compositor keeps the real one (its socket lives there).
HUB_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-native-run-XXXXXX)"
trap 'rm -rf "$CONFIG_DIR" "$HUB_RUNTIME_DIR"' EXIT

HUB_OUT="/dev/stderr"
[ "$DEBUG" = "1" ] && HUB_OUT="$DEBUG_LOG" && echo "Debug log: $HUB_OUT"

{
    echo '# WAYLAND_DISPLAY must be a full path once XDG_RUNTIME_DIR changes.'
    echo 'export WAYLAND_DISPLAY="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"'
    echo "export XDG_RUNTIME_DIR=\"$HUB_RUNTIME_DIR\""
    echo 'unset GDK_BACKEND'
    # Tools started by the hub (opusdm-lister, opusdm-run...) are found by name
    echo "export PATH=\"$BIN_DIR:\$PATH\""
    [ "$DEBUG" = "1" ] && echo 'export OPUSDM_DEBUG=1'
    # A sandbox never starts the user's Startup Apps a second time (as run-x11.sh)
    echo "\"$HUB_BIN\" --no-startup-apps >\"$HUB_OUT\" 2>&1 &"
} >"$CONFIG_DIR/autostart"

[ "$DEBUG" = "1" ] && export OPUSDM_DEBUG=1

echo "Starting opusdm-compositor ($RESOLUTION)"
# Not exec: the shell stays to run the EXIT trap that removes the temp dirs.
"$COMPOSITOR_BIN" --nested --size "$RESOLUTION" --startup "sh $CONFIG_DIR/autostart"
