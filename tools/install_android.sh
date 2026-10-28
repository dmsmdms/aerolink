#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APK="$PROJECT_ROOT/build/aerolink.apk"

if [ ! -f "$APK" ]; then
    echo "❌ Error: $APK not found, run 'make android' first."
    exit 1
fi

echo "📲 Installing $APK..."
adb install -r "$APK"

echo "✅ APK installed."
