#!/bin/bash
# End-to-end smoke test: backend + console build + fake robot in headless Chromium (tools/e2e_console.mjs),
# then the Android robot code (signaling, session control, simulated base) against the same backend.
# Needs: chromium (Debian package: chromium, or set CHROMIUM=/path), Node >= 22, `make frontend` done.
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$PROJECT_ROOT/backend/target/release/aerolink"
BACKEND_PORT="${E2E_BACKEND_PORT:-18080}"
CONSOLE_PORT="${E2E_CONSOLE_PORT:-14173}"

if ! command -v "${CHROMIUM:-chromium}" >/dev/null; then
    echo "❌ Error: chromium not found (Debian package: chromium), or set CHROMIUM=/path/to/browser."
    exit 1
fi
if [ "$(node -p 'process.versions.node.split(".")[0]')" -lt 22 ]; then
    echo "❌ Error: Node >= 22 required (global WebSocket), found $(node --version)."
    exit 1
fi
if [ ! -f "$PROJECT_ROOT/build/www/console/index.html" ]; then
    echo "❌ Error: build/www not found, run 'make frontend' first."
    exit 1
fi
(cd "$PROJECT_ROOT/backend" && cargo build --release --quiet)

WORK="$(mktemp -d)"
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

cat > "$WORK/env" <<CONF
AEROLINK_LISTEN=127.0.0.1:$BACKEND_PORT
AEROLINK_DATABASE=$WORK/aerolink.db
AEROLINK_ERROR_LOG=$WORK/error.log
AEROLINK_ORIGIN=http://localhost:$CONSOLE_PORT
AEROLINK_SECURE_COOKIES=false
AEROLINK_LOG=warn
CONF
export AEROLINK_CONFIG="$WORK/env"
echo password1234 | "$BIN" operator add op@x.org Op >/dev/null
TOKEN="$("$BIN" robot add r1 2>/dev/null | tail -1)"
PHONE_TOKEN="$("$BIN" robot add phone 2>/dev/null | tail -1)"

"$BIN" serve &
PIDS+=($!)
(cd "$PROJECT_ROOT/frontend" && AEROLINK_BACKEND="127.0.0.1:$BACKEND_PORT" \
    exec ./node_modules/.bin/vite preview --port "$CONSOLE_PORT" --strictPort --logLevel warn) &
PIDS+=($!)
sleep 1.5

echo "🧪 Console end-to-end test (screenshots: ${E2E_SCREENSHOTS:-not kept})..."
node "$PROJECT_ROOT/tools/e2e_console.mjs" "$TOKEN" "$WORK" "$CONSOLE_PORT" "$BACKEND_PORT"
echo "🧪 Robot app (signaling + session control, fake WebRTC peer) against the backend..."
(cd "$PROJECT_ROOT/android" && gradle testDebugUnitTest --quiet --rerun-tasks --tests '*BackendInteropTest' \
    -Paerolink.backend="http://127.0.0.1:$BACKEND_PORT" -Paerolink.robotToken="$PHONE_TOKEN" \
    -Paerolink.origin="http://localhost:$CONSOLE_PORT")
if [ -n "$E2E_SCREENSHOTS" ]; then
    mkdir -p "$E2E_SCREENSHOTS" && cp "$WORK"/*.png "$E2E_SCREENSHOTS"/
fi
if [ -s "$WORK/error.log" ]; then
    echo "❌ Backend logged errors:"; cat "$WORK/error.log"; exit 1
fi
echo "✅ Console end-to-end test passed."
