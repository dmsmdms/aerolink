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

# -f reboots a running board into BOOTSEL if possible, -x starts the new image.
echo "⚡ Flashing $UF2..."
picotool load -f -x -v "$UF2"

echo "✅ Firmware flashed."
