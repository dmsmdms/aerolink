#!/bin/bash
# Bench server on this PC for a phone on the same LAN (docs/09-deployment.md "Bench test"):
# backend on 0.0.0.0:$BENCH_PORT (the robot app connects to it directly, plain ws), console at
# http://localhost:$BENCH_CONSOLE_PORT/console/ (vite preview, proxies /api and /ws to the backend).
# State (database, robot token) stays in build/bench/ between runs. Ctrl+C stops everything.
# The robot app must be a debug build (plain http allowed): make android ANDROID_VARIANT=debug install-apk
#
# BENCH_USB=1: the phone is not on the LAN (e.g. cellular only) and talks to this PC over the USB cable:
# `adb reverse` forwards the backend and a local coturn (TURN over TCP). Media still goes direct when the phone
# is on the LAN after all; open the console with `?relay` to force the relay path, as on cellular behind a
# strict NAT. Needs `coturn` (only the turnserver binary is used, the system service stays off).
#
# App updates (ROB-008): copy build/aerolink-*.apk and build/aerolink-release.json into build/bench/apk/, then
# `pkill -HUP -f "aerolink serve"` (or restart this script) and the phone updates itself between sessions.
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$PROJECT_ROOT/backend/target/release/aerolink"
STATE="$PROJECT_ROOT/build/bench"
PORT="${BENCH_PORT:-8080}"
CONSOLE_PORT="${BENCH_CONSOLE_PORT:-5173}"
OPERATOR="bench@aerolink.local"
PASSWORD="bench-password"

if [ ! -f "$PROJECT_ROOT/build/www/console/index.html" ]; then
    echo "❌ Error: build/www not found, run 'make frontend' first."
    exit 1
fi
(cd "$PROJECT_ROOT/backend" && cargo build --release --quiet)
mkdir -p "$STATE" "$STATE/apk"

TURN_PORT="${BENCH_TURN_PORT:-3478}"
TURN_ENV=""
if [ -n "$BENCH_USB" ]; then
    command -v turnserver >/dev/null || { echo "❌ Error: turnserver not found (Debian package: coturn)."; exit 1; }
    adb reverse "tcp:$PORT" "tcp:$PORT" >/dev/null
    adb reverse "tcp:$TURN_PORT" "tcp:$TURN_PORT" >/dev/null
    [ -s "$STATE/turn-secret" ] || head -c 24 /dev/urandom | base64 > "$STATE/turn-secret"
    # Clients reach coturn on 127.0.0.1 (adb reverse / this PC), relay addresses are on the LAN address:
    # WebRTC does not pair loopback candidates, so relays on 127.0.0.1 would never connect.
    RELAY_IP="$(hostname -I | awk '{print $1}')"
    cat > "$STATE/turnserver.conf" <<TURN
listening-ip=127.0.0.1
relay-ip=${RELAY_IP:-127.0.0.1}
listening-port=$TURN_PORT
no-tls
no-dtls
no-cli
fingerprint
use-auth-secret
static-auth-secret=$(cat "$STATE/turn-secret")
realm=bench
allow-loopback-peers
pidfile=$STATE/turnserver.pid
log-file=stdout
simple-log
TURN
    TURN_ENV="AEROLINK_TURN_URLS=turn:127.0.0.1:$TURN_PORT?transport=tcp
AEROLINK_TURN_SECRET=$(cat "$STATE/turn-secret")"
fi

cat > "$STATE/env" <<CONF
AEROLINK_LISTEN=0.0.0.0:$PORT
AEROLINK_DATABASE=$STATE/aerolink.db
AEROLINK_ERROR_LOG=$STATE/error.log
AEROLINK_ORIGIN=http://localhost:$CONSOLE_PORT
AEROLINK_SECURE_COOKIES=false
AEROLINK_LOG=info
AEROLINK_APK_DIR=$STATE/apk
$TURN_ENV
CONF
export AEROLINK_CONFIG="$STATE/env"

if ! "$BIN" operator list | grep -q "$OPERATOR"; then
    echo "$PASSWORD" | "$BIN" operator add "$OPERATOR" Bench >/dev/null
fi
if [ ! -s "$STATE/robot-token" ]; then
    "$BIN" robot add bench 2>/dev/null | tail -1 > "$STATE/robot-token"
fi

PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

"$BIN" serve &
PIDS+=($!)
if [ -n "$BENCH_USB" ]; then
    turnserver -c "$STATE/turnserver.conf" > "$STATE/turnserver.log" 2>&1 &
    PIDS+=($!)
fi
(cd "$PROJECT_ROOT/frontend" && AEROLINK_BACKEND="127.0.0.1:$PORT" \
    exec ./node_modules/.bin/vite preview --port "$CONSOLE_PORT" --strictPort --logLevel warn) &
PIDS+=($!)

SERVER_IP="$(hostname -I | awk '{print $1}')"
[ -n "$BENCH_USB" ] && SERVER_IP=127.0.0.1
cat <<INFO

  Robot app (debug build) → Server: http://$SERVER_IP:$PORT
                            Robot token: $(cat "$STATE/robot-token")
  Console → http://localhost:$CONSOLE_PORT/console/   login $OPERATOR / $PASSWORD
  Phone logs: adb logcat -s AeroLinkService AeroLinkTeleop AeroLinkRtc

INFO
wait
