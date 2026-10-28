#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$PROJECT_ROOT/build"
FW_DIR="$PROJECT_ROOT/rp2040"
FW_BUILD="$FW_DIR/build"

export PICO_SDK_PATH="${PICO_SDK_PATH:-/opt/pico-sdk}"
PICO_BOARD="${PICO_BOARD:-pico}"
BUILD_TYPE="${BUILD_TYPE:-Release}"

if [ ! -f "$PICO_SDK_PATH/pico_sdk_init.cmake" ]; then
    echo "❌ Error: Pico SDK not found at $PICO_SDK_PATH (set PICO_SDK_PATH)."
    exit 1
fi

mkdir -p "$OUT_DIR"

echo "🔧 Building RP2040 firmware (board: $PICO_BOARD, $BUILD_TYPE, SDK: $PICO_SDK_PATH)..."
cmake -S "$FW_DIR" -B "$FW_BUILD" -G Ninja \
    -DPICO_BOARD="$PICO_BOARD" \
    -DCMAKE_BUILD_TYPE="$BUILD_TYPE"
cmake --build "$FW_BUILD"

UF2_SOURCE="$FW_BUILD/aerolink.uf2"
if [ -f "$UF2_SOURCE" ]; then
    cp "$UF2_SOURCE" "$OUT_DIR/aerolink-rp2040.uf2"
    echo "✅ Firmware successfully placed to build/aerolink-rp2040.uf2"
else
    echo "❌ Error: UF2 file not found!"
    exit 1
fi
