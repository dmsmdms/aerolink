#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FRONTEND_DIR="$PROJECT_ROOT/frontend"

cd "$FRONTEND_DIR"
if ! command -v npm >/dev/null; then
    echo "❌ Error: npm not found (Debian packages: nodejs npm)."
    exit 1
fi
# Reinstall only when the lockfile changed since the last install.
if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then
    echo "📦 Installing frontend dependencies (npm ci)..."
    npm ci --no-audit --no-fund
fi

echo "🔎 Type-checking frontend..."
npm run --silent check

echo "🌐 Building frontend (Vite) and pre-compressing (.br/.gz)..."
npm run --silent build

echo "✅ Frontend placed to build/www"
