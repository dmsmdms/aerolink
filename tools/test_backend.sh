#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT/backend"

echo "🧪 Running backend tests (hub, db, auth, HTTP/WebSocket end-to-end)..."
cargo test --quiet

echo "✅ Backend tests passed."
