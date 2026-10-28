#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKEND_DIR="$PROJECT_ROOT/backend"

echo "🧹 Cleaning Backend (Rust) build artifacts..."
if [ -d "$BACKEND_DIR" ]; then
    cd "$BACKEND_DIR"
    cargo clean
else
    echo "⚠️ Backend directory not found: $BACKEND_DIR"
fi

echo "✅ Backend cleanup completed."
