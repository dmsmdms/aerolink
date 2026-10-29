#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "🧹 Cleaning frontend build output..."
rm -rf "$PROJECT_ROOT/build/www" "$PROJECT_ROOT/frontend/node_modules/.vite"

echo "✅ Frontend cleanup completed."
