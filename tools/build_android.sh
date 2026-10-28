#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$PROJECT_ROOT/build"
ANDROID_DIR="$PROJECT_ROOT/android"

mkdir -p "$OUT_DIR"
cd "$ANDROID_DIR"

# local.properties is machine-specific and not in git
if [ ! -f local.properties ]; then
    SDK_DIR="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-/opt/android-sdk}}"
    echo "sdk.dir=$SDK_DIR" > local.properties
    echo "ℹ️ Created android/local.properties (sdk.dir=$SDK_DIR)"
fi

echo "🤖 Building Android APK..."
gradle assembleRelease --warning-mode all

APK_SOURCE="$ANDROID_DIR/app/build/outputs/apk/release/app-release.apk"
if [ -f "$APK_SOURCE" ]; then
    cp "$APK_SOURCE" "$OUT_DIR/aerolink.apk"
    echo "✅ Android APK successfully placed to build/aerolink.apk"
else
    echo "❌ Error: Release APK file not found!"
    exit 1
fi
