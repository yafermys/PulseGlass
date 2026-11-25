#!/usr/bin/env bash
set -euo pipefail

if [ -x "$HOME/.local/bin/pulseglass" ]; then
    "$HOME/.local/bin/pulseglass" --uninstall
else
    rm -f "$HOME/.local/bin/pulseglass" \
          "$HOME/.local/share/applications/pulseglass.desktop" \
          "$HOME/.local/share/icons/hicolor/256x256/apps/pulseglass.png" \
          "$HOME/.config/autostart/pulseglass.desktop"
    echo "PulseGlass removed."
fi
