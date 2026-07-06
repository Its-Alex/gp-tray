#!/usr/bin/env bash
# Install gp-tray for the current user (no root needed).
set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
CONFIG_DIR="$HOME/.config/gp-tray"
AUTOSTART_DIR="$HOME/.config/autostart"
APPS_DIR="$HOME/.local/share/applications"

echo "Installing gp-tray…"
mkdir -p "$BIN_DIR" "$CONFIG_DIR" "$AUTOSTART_DIR" "$APPS_DIR"

install -m755 "$SRC_DIR/gp-tray.py" "$BIN_DIR/gp-tray"
echo "  • $BIN_DIR/gp-tray"

if [ ! -f "$CONFIG_DIR/portals.conf" ]; then
    cp "$SRC_DIR/portals.conf.example" "$CONFIG_DIR/portals.conf"
    echo "  • $CONFIG_DIR/portals.conf  (created — EDIT THIS with your portals)"
else
    echo "  • $CONFIG_DIR/portals.conf  (kept existing)"
fi

# Desktop entries with an absolute Exec so autostart works regardless of PATH.
gen_desktop() {
    sed "s|^Exec=gp-tray$|Exec=$BIN_DIR/gp-tray|" "$SRC_DIR/gp-tray.desktop" > "$1"
    echo "  • $1"
}
gen_desktop "$AUTOSTART_DIR/gp-tray.desktop"
gen_desktop "$APPS_DIR/gp-tray.desktop"

echo
echo "Done. Edit $CONFIG_DIR/portals.conf, then start it with:  gp-tray &"
echo "It will also auto-start on your next login."
