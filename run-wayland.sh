#!/usr/bin/env bash
#
# Launch a nested labwc (or Wayfire, --wayfire) and run opusdm-hub inside it.
# The Wayland sibling of run-x11.sh: a sandbox for testing OpusDM on Wayland
# without touching the real session. labwc opens as a window of the current
# desktop, X11 or Wayland alike.
#
# Usage:
#   ./run-wayland.sh [--wayfire] [--debug] [--grid-debug] [--headless] [resolution]
#
# Examples:
#   ./run-wayland.sh              # 1920x1080
#   ./run-wayland.sh 1280x800
#   ./run-wayland.sh --debug      # debug log in /ramdisk/opusdm.log
#   ./run-wayland.sh --headless   # no window: for scripted tests (grim, wlrctl)
#   ./run-wayland.sh --wayfire    # Wayfire with wayfire.ini instead of labwc

set -euo pipefail

DEBUG=0
GRID_DEBUG=0
HEADLESS=0
WAYFIRE=0
ARGS=()
for a in "$@"; do
    case "$a" in
        --debug) DEBUG=1 ;;
        --grid-debug) GRID_DEBUG=1 ;;
        --headless) HEADLESS=1 ;;
        --wayfire) WAYFIRE=1 ;;
        *) ARGS+=("$a") ;;
    esac
done
set -- ${ARGS[@]+"${ARGS[@]}"}

# Configuration (overridable via args or environment)
RESOLUTION="${1:-${WAYLAND_RES:-1920x1080}}"
LABWC_CONFIG="${WAYLAND_LABWC_CONFIG:-$(pwd)/labwc-rc.xml}"   # rc.xml for labwc
WAYFIRE_CONFIG="${WAYLAND_WAYFIRE_CONFIG:-$(pwd)/wayfire.ini}" # wayfire.ini for Wayfire

# Exit conditions up front
if [ "$WAYFIRE" = "1" ]; then
    command -v wayfire >/dev/null 2>&1 || { echo "wayfire not found (package: wayfire)"; exit 1; }
    [ -f "$WAYFIRE_CONFIG" ] || { echo "Wayfire config not found: $WAYFIRE_CONFIG"; exit 1; }
else
    command -v labwc >/dev/null 2>&1 || { echo "labwc not found (package: labwc)"; exit 1; }
    [ -f "$LABWC_CONFIG" ] || { echo "labwc config not found: $LABWC_CONFIG"; exit 1; }
fi
command -v wlr-randr >/dev/null 2>&1 || { echo "wlr-randr not found (package: wlr-randr)"; exit 1; }

# Build the workspace before entering the sandbox
echo "Building opusdm (release)"
cargo build --release --workspace

HUB_BIN="$(pwd)/target/release/opusdm-hub"
[ -x "$HUB_BIN" ] || { echo "opusdm-hub not built at $HUB_BIN"; exit 1; }

# A config directory of its own: rc.xml plus an autostart that sizes the
# nested output and starts the hub.
CONFIG_DIR="$(mktemp -d /tmp/opusdm-labwc-XXXXXX)"
# The hub gets its own runtime dir so a stale socket never blocks the
# sandbox; the compositor keeps the real one (its socket lives there).
HUB_RUNTIME_DIR="$(mktemp -d /tmp/opusdm-wayland-XXXXXX)"
trap 'rm -rf "$CONFIG_DIR"' EXIT

cp "$LABWC_CONFIG" "$CONFIG_DIR/rc.xml"

HUB_OUT="/dev/stderr"
[ "$DEBUG" = "1" ] && HUB_OUT="/ramdisk/opusdm.log" && echo "Debug log: $HUB_OUT"

{
    echo '# Size the nested output (its name depends on the host: WL-1, X11-1...)'
    echo 'OUTPUT="$(wlr-randr | awk '"'"'NR == 1 { print $1 }'"'"')"'
    echo "wlr-randr --output \"\$OUTPUT\" --custom-mode $RESOLUTION || true"
    echo '# WAYLAND_DISPLAY must be a full path once XDG_RUNTIME_DIR changes.'
    echo 'export WAYLAND_DISPLAY="$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"'
    echo "export XDG_RUNTIME_DIR=\"$HUB_RUNTIME_DIR\""
    echo 'unset GDK_BACKEND'
    # The hub writes its global hotkeys into the sandbox's own rc.xml.
    echo "export OPUSDM_LABWC_RC=\"$CONFIG_DIR/rc.xml\""
    # Hotkeys such as exec:opusdm-run find the OpusDM binaries by name
    echo "export PATH=\"$(pwd)/target/release:\$PATH\""
    [ "$GRID_DEBUG" = "1" ] && echo 'export GRID_DEBUG=1'
    [ "$DEBUG" = "1" ] && echo 'export OPUSDM_DEBUG=1'
    echo "\"$HUB_BIN\" >\"$HUB_OUT\" 2>&1 &"
} >"$CONFIG_DIR/autostart"

if [ "$WAYFIRE" = "1" ]; then
    # wayfire.ini starts $OPUSDM_HUB: here the same autostart script labwc runs.
    export OPUSDM_HUB="sh $CONFIG_DIR/autostart"
    echo "Starting Wayfire ($RESOLUTION), config: $WAYFIRE_CONFIG"
    if [ "$HEADLESS" = "1" ]; then
        WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WAYLAND_DISPLAY= wayfire -c "$WAYFIRE_CONFIG"
    else
        wayfire -c "$WAYFIRE_CONFIG"
    fi
    exit 0
fi

echo "Starting labwc ($RESOLUTION), config: $LABWC_CONFIG"
if [ "$HEADLESS" = "1" ]; then
    # Prints the socket to use, e.g. WAYLAND_DISPLAY=wayland-1 grim shot.png
    WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WAYLAND_DISPLAY= labwc --config-dir "$CONFIG_DIR"
else
    labwc --config-dir "$CONFIG_DIR"
fi
