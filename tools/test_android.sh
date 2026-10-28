#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ANDROID_DIR="$PROJECT_ROOT/android"

cd "$ANDROID_DIR"
if [ ! -f local.properties ]; then
    echo "sdk.dir=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-/opt/android-sdk}}" > local.properties
fi

echo "🧪 Running Android unit tests..."
gradle testDebugUnitTest --warning-mode summary

echo "✅ Android unit tests passed."
