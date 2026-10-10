#!/bin/sh
# Build OpusDM in release mode and stage everything needed to deploy it in ./dist.
#
# Layout of dist/:
#   bin/      opusdm-hub, opusdm-lister, opusdm-settings, opusdm-dock, opusdm-run,
#             opusdm-shot, opusdm-terminal, opusdm-player, opusdm-viewer, opusdm-input-config, opusdm-screen, opusdm-apps-editor
#             (the hub finds opusdm-lister next to itself, keep them together)
#   config/   openbox-rc.xml, picom.conf, labwc-rc.xml
#   themes/   <slug>.zip theme packages
#   opusdm-session            X11 session script (Openbox + picom + opusdm-hub)
#   opusdm-session-wayland    Wayland session script (labwc + opusdm-hub)
#   opusdm.desktop            "OpusDM (X11)" xsession entry template
#   opusdm-wayland.desktop    "OpusDM (Wayland)" wayland-session entry template
#                             (Exec of both is set by install.sh)
#   install.sh                copies dist to a prefix and registers the
#                             session(s): install.sh --x11 | --wayland [prefix]
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
DIST="$REPO_ROOT/dist"
BINARIES="opusdm-hub opusdm-lister opusdm-settings opusdm-dock opusdm-run opusdm-shot opusdm-terminal opusdm-player opusdm-viewer opusdm-input-config opusdm-screen opusdm-apps-editor"

cd "$REPO_ROOT"

echo "Building workspace (release)"
cargo build --release --workspace

echo "Packaging themes"
cargo run --release -p xtask -- package-themes

# Start from a clean dist so removed files do not linger
rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/config" "$DIST/themes"

for b in $BINARIES; do
    src="$REPO_ROOT/target/release/$b"
    if [ ! -x "$src" ]; then
        echo "build-release: missing binary $src" >&2
        exit 1
    fi
    cp "$src" "$DIST/bin/$b"
done

# Folder opener behind the opusdm-lister.desktop entry (inode/directory)
cp "$REPO_ROOT/scripts/opusdm-open-folder" "$DIST/bin/opusdm-open-folder"
cp "$REPO_ROOT/assets/desktop/opusdm-lister.desktop" "$DIST/opusdm-lister.desktop"

cp "$REPO_ROOT/openbox-rc.xml" "$REPO_ROOT/picom.conf" "$REPO_ROOT/labwc-rc.xml" "$DIST/config/"
cp "$REPO_ROOT"/target/themes/*.zip "$DIST/themes/"

# X11 session script: resolves its own location, so dist can live anywhere
cat > "$DIST/opusdm-session" <<'SESSION'
#!/bin/sh
# OpusDM X11 session: compositor, hub and Openbox as the window manager.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# Hotkeys such as exec:opusdm-run find the OpusDM binaries by name.
export PATH="$HERE/bin:$PATH"

# picom runs on a per-user copy of its config: the hub rewrites the opacity
# keys of that copy (Settings, Windows tab) and signals picom to reload it.
export OPUSDM_PICOM_BASE="$HERE/config/picom.conf"
PICOM_USER_CONF="${XDG_CONFIG_HOME:-$HOME/.config}/opusdm/picom.conf"
if command -v picom >/dev/null 2>&1; then
    mkdir -p "$(dirname "$PICOM_USER_CONF")"
    [ -f "$PICOM_USER_CONF" ] || cp "$OPUSDM_PICOM_BASE" "$PICOM_USER_CONF"
    picom --config "$PICOM_USER_CONF" &
fi
"$HERE/bin/opusdm-hub" &
exec openbox --config-file "$HERE/config/openbox-rc.xml"
SESSION
chmod +x "$DIST/opusdm-session"

# Wayland session script: labwc as the compositor, the hub started by it.
cat > "$DIST/opusdm-session-wayland" <<'SESSION'
#!/bin/sh
# OpusDM Wayland session: labwc as the compositor, opusdm-hub started by it.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# Hotkeys such as exec:opusdm-run find the OpusDM binaries by name.
export PATH="$HERE/bin:$PATH"
export XDG_SESSION_TYPE=wayland
# GTK must not be pushed onto XWayland by a leftover setting.
unset GDK_BACKEND

# labwc reads ~/.config/labwc/rc.xml. The first time, start from the OpusDM
# one; the hub then keeps its own marked blocks in it (global hotkeys, input
# settings) and never touches the rest.
LABWC_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/labwc"
if [ ! -f "$LABWC_DIR/rc.xml" ]; then
    mkdir -p "$LABWC_DIR"
    cp "$HERE/config/labwc-rc.xml" "$LABWC_DIR/rc.xml"
fi

# -s (not -S): the session lives as long as labwc, so restarting the hub
# (scripts/live-update.sh) does not log the user out.
exec labwc -s "$HERE/bin/opusdm-hub"
SESSION
chmod +x "$DIST/opusdm-session-wayland"

cat > "$DIST/opusdm.desktop" <<'DESKTOP'
[Desktop Entry]
Name=OpusDM (X11)
Comment=OpusDM desktop on Openbox
Exec=@PREFIX@/opusdm-session
Type=Application
DESKTOP

cat > "$DIST/opusdm-wayland.desktop" <<'DESKTOP'
[Desktop Entry]
Name=OpusDM (Wayland)
Comment=OpusDM desktop on labwc
Exec=@PREFIX@/opusdm-session-wayland
Type=Application
DesktopNames=OpusDM;labwc;wlroots
DESKTOP

cat > "$DIST/install.sh" <<'INSTALL'
#!/bin/sh
# Copy this dist to a prefix and register the OpusDM session(s) for the login
# manager (SDDM, GDM, LightDM...).
# Usage: ./install.sh --x11|--wayland [--x11|--wayland] [prefix]
#   --x11       register "OpusDM (X11)" (Openbox + picom) in /usr/share/xsessions
#   --wayland   register "OpusDM (Wayland)" (labwc) in /usr/share/wayland-sessions
#   At least one of the two is required; both can be given.
#   prefix      default /opt/opusdm (needs sudo)
set -eu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PREFIX=/opt/opusdm
X11=0
WAYLAND=0

usage() {
    sed -n '4,8p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --x11) X11=1 ;;
        --wayland) WAYLAND=1 ;;
        -h|--help) usage ;;
        -*) echo "install: unknown option $1" >&2; usage ;;
        *) PREFIX="$1" ;;
    esac
    shift
done

# Exit conditions up front
[ "$X11" -eq 1 ] || [ "$WAYLAND" -eq 1 ] || { echo "install: choose the session: --x11 and/or --wayland" >&2; usage; }
if [ "$X11" -eq 1 ] && ! command -v openbox >/dev/null 2>&1; then
    echo "install: warning: openbox is not installed, the X11 session needs it (package: openbox)" >&2
fi
if [ "$WAYLAND" -eq 1 ]; then
    command -v labwc >/dev/null 2>&1 || echo "install: warning: labwc is not installed, the Wayland session needs it (package: labwc)" >&2
    command -v wlr-randr >/dev/null 2>&1 || echo "install: warning: wlr-randr is not installed, the screen settings need it (package: wlr-randr)" >&2
fi

if [ "$(id -u)" -eq 0 ]; then SUDO=""; else SUDO="sudo"; fi

$SUDO mkdir -p "$PREFIX"
$SUDO cp -r "$HERE/bin" "$HERE/config" "$HERE/themes" "$HERE/opusdm-session" "$HERE/opusdm-session-wayland" "$PREFIX/"
# The folder handler needs opusdm-open-folder on the PATH of the programs that call it
$SUDO mkdir -p /usr/local/bin
$SUDO ln -sf "$PREFIX/bin/opusdm-open-folder" /usr/local/bin/opusdm-open-folder
$SUDO mkdir -p /usr/share/applications
$SUDO cp "$HERE/opusdm-lister.desktop" /usr/share/applications/opusdm-lister.desktop
echo "Installed to $PREFIX"

if [ "$X11" -eq 1 ]; then
    $SUDO mkdir -p /usr/share/xsessions
    sed "s|@PREFIX@|$PREFIX|" "$HERE/opusdm.desktop" | $SUDO tee /usr/share/xsessions/opusdm.desktop >/dev/null
    echo "Session \"OpusDM (X11)\": /usr/share/xsessions/opusdm.desktop"
fi
if [ "$WAYLAND" -eq 1 ]; then
    $SUDO mkdir -p /usr/share/wayland-sessions
    sed "s|@PREFIX@|$PREFIX|" "$HERE/opusdm-wayland.desktop" | $SUDO tee /usr/share/wayland-sessions/opusdm-wayland.desktop >/dev/null
    echo "Session \"OpusDM (Wayland)\": /usr/share/wayland-sessions/opusdm-wayland.desktop"
fi
echo "Folders open in OpusDM by default: opusdm-hub sets opusdm-lister.desktop for inode/directory at every start"
INSTALL
chmod +x "$DIST/install.sh"

echo "Done: $DIST"
