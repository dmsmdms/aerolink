#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ANDROID_DIR="$PROJECT_ROOT/android"

echo "🧹 Cleaning Android build artifacts via Gradle..."
if [ -d "$ANDROID_DIR" ]; then
    cd "$ANDROID_DIR"
    gradle clean --warning-mode all
else
    echo "⚠️ Android directory not found: $ANDROID_DIR"
fi

echo "✅ Android cleanup completed."
