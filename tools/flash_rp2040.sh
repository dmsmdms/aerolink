#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UF2="$PROJECT_ROOT/build/aerolink-rp2040.uf2"

if [ ! -f "$UF2" ]; then
    echo "❌ Error: $UF2 not found, run 'make rp2040' first."
    exit 1
fi

if ! command -v picotool > /dev/null; then
    echo "❌ Error: picotool not found (Debian package: picotool)."
    exit 1
fi

# The board must be in BOOTSEL (hold the button while plugging it in): our firmware's USB port is the AOA host,
# so -f cannot reboot it from here. The UF2 holds the loader, an erased boot record and the application
# (docs/09-deployment.md OPS-033); -x starts it.
echo "⚡ Flashing $UF2..."
picotool load -f -x -v "$UF2"

echo "✅ Firmware flashed."
