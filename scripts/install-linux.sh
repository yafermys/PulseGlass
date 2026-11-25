#!/usr/bin/env bash
set -euo pipefail

BIN_SRC="${1:-./target/release/PulseGlass}"
if [ ! -f "$BIN_SRC" ] && [ -f "./pulseglass" ]; then
    BIN_SRC="./pulseglass"
fi
if [ ! -f "$BIN_SRC" ]; then
    echo "Binary not found at $BIN_SRC. Build first with: cargo build --release --bin PulseGlass"
    exit 1
fi

"$BIN_SRC" --install
echo "PulseGlass installed to ~/.local/bin/pulseglass and added to your desktop application menu."
