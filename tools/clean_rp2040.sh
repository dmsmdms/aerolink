#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "🧹 Cleaning RP2040 firmware build..."
rm -rf "$PROJECT_ROOT/rp2040/build"

echo "✅ RP2040 cleanup completed."
