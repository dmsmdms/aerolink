#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$PROJECT_ROOT/build"
BACKEND_DIR="$PROJECT_ROOT/backend"
PACKAGE_TEMPLATE="$BACKEND_DIR/package"

mkdir -p "$OUT_DIR"
cd "$BACKEND_DIR"

echo "🦀 Building Rust backend (Release)..."
cargo build --release

# Assemble the deb package in a temporary directory
BUILD_ROOT=$(mktemp -d)
trap 'rm -rf "$BUILD_ROOT"' EXIT
PKG_DIR="$BUILD_ROOT/aerolink"

mkdir -p "$PKG_DIR"
cp -r "$PACKAGE_TEMPLATE"/. "$PKG_DIR/"
mkdir -p "$PKG_DIR/usr/bin" "$PKG_DIR/DEBIAN"

cp target/release/aerolink "$PKG_DIR/usr/bin/aerolink"
chmod 755 "$PKG_DIR/usr/bin/aerolink"

echo "📦 Packaging into .deb..."
rm -f "$OUT_DIR/aerolink.deb"
dpkg-deb --root-owner-group --build "$PKG_DIR" "$OUT_DIR/aerolink.deb"

echo "✅ Backend deb package successfully placed to build/aerolink.deb"
