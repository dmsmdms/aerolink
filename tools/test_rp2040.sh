#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FW_DIR="$PROJECT_ROOT/rp2040"
TEST_BUILD="$FW_DIR/build/host-test"

mkdir -p "$TEST_BUILD"

echo "🧪 Building firmware host tests..."
gcc -std=c11 -O1 -g -Wall -Wextra -Werror \
    -fsanitize=address,undefined -fno-sanitize-recover=all \
    -DAEROLINK_FW_VERSION_MAJOR=0 -DAEROLINK_FW_VERSION_MINOR=1 -DAEROLINK_FW_VERSION_PATCH=0 \
    -I"$FW_DIR/src" -I"$FW_DIR/test" \
    "$FW_DIR/test/test_main.c" "$FW_DIR/test/test_fw.c" "$FW_DIR/test/fake_flash.c" \
    "$FW_DIR/src/proto.c" "$FW_DIR/src/control.c" "$FW_DIR/src/link.c" \
    "$FW_DIR/src/crc32.c" "$FW_DIR/src/fwstore.c" "$FW_DIR/src/fwupdate.c" \
    -o "$TEST_BUILD/test_main"

"$TEST_BUILD/test_main" "$PROJECT_ROOT/protocol/vectors.txt"
