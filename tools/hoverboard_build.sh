#!/bin/bash
# Builds the hoverboard mainboard firmware of one model (docs/11-hoverboard.md):
#   hoverboard_build.sh gen2x-tt-sd2.2
# Clones the pinned upstream sources (hoverboard/<model>/build.conf) into build/hoverboard/, puts the model's
# config.h in place and builds one image per SLAVE_ID into hoverboard/<model>/firmware-id<N>.bin + SHA256SUMS.
# The images are committed, so flashing a robot needs no build; rebuild only after changing config.h.
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODEL="$1"
MODEL_DIR="$PROJECT_ROOT/hoverboard/$MODEL"
if [ -z "$MODEL" ] || [ ! -f "$MODEL_DIR/build.conf" ]; then
    echo "usage: $0 MODEL   (one of: $(ls "$PROJECT_ROOT/hoverboard" | tr '\n' ' '))"
    exit 1
fi
for tool in git make arm-none-eabi-gcc; do
    if ! command -v $tool > /dev/null; then
        echo "❌ Error: $tool not found (Debian packages: git, make, gcc-arm-none-eabi, libnewlib-arm-none-eabi)."
        exit 1
    fi
done
# shellcheck source=/dev/null
. "$MODEL_DIR/build.conf"

# Clones repo $1 at commit $2 into $3 (reused if already there).
checkout() {
    if [ ! -d "$3/.git" ]; then
        git clone --quiet "$1" "$3"
    fi
    git -C "$3" cat-file -e "$2^{commit}" 2> /dev/null || git -C "$3" fetch --quiet origin
    git -C "$3" checkout --quiet --force "$2"
    git -C "$3" clean --quiet -fdx -e lib/framework
}

SRC="$PROJECT_ROOT/build/hoverboard/$MODEL"
FW="$SRC/$FIRMWARE_DIR"
echo "🔧 Fetching sources for $MODEL..."
checkout "$FIRMWARE_REPO" "$FIRMWARE_COMMIT" "$SRC"
checkout "$SPL_REPO" "$SPL_COMMIT" "$FW/lib/framework"

for id in $SLAVE_IDS; do
    echo "🔧 Building SLAVE_ID $id..."
    sed -E "s/^([[:space:]]*#define[[:space:]]+SLAVE_ID[[:space:]]+)[0-9]+/\1$id/" "$MODEL_DIR/config.h" > "$FW/Inc/config.h"
    grep -qE "^[[:space:]]*#define[[:space:]]+SLAVE_ID[[:space:]]+$id\b" "$FW/Inc/config.h" || { echo "❌ SLAVE_ID not found in config.h"; exit 1; }
    rm -rf "$FW/build"
    make -C "$FW" > "$SRC/make-id$id.log" 2>&1 || { tail -20 "$SRC/make-id$id.log"; echo "❌ Build failed, log: $SRC/make-id$id.log"; exit 1; }
    bin=$(ls "$FW"/build/*.bin)
    cp "$bin" "$MODEL_DIR/firmware-id$id.bin"
done

(cd "$MODEL_DIR" && sha256sum firmware-id*.bin > SHA256SUMS)
cat "$MODEL_DIR/SHA256SUMS"
echo "✅ Images in hoverboard/$MODEL/, flash with: tools/hoverboard_swd.sh flash hoverboard/$MODEL/firmware-id<N>.bin"
