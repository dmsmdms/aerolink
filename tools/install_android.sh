#!/bin/bash
# Installs the APK matching the connected phone's preferred ABI (build/aerolink-<abi>.apk).
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

ABIS="$(adb shell getprop ro.product.cpu.abilist | tr -d '\r')"
if [ -z "$ABIS" ]; then
    echo "❌ Error: no phone found (adb devices)."
    exit 1
fi
for ABI in ${ABIS//,/ }; do
    APK="$PROJECT_ROOT/build/aerolink-$ABI.apk"
    if [ -f "$APK" ]; then
        echo "📲 Installing $APK..."
        adb install -r "$APK"
        echo "✅ APK installed."
        exit 0
    fi
done
echo "❌ Error: no build/aerolink-<abi>.apk for this phone ($ABIS), run 'make android' first."
exit 1
