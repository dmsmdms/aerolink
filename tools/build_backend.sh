#!/bin/bash
set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$PROJECT_ROOT/build"
BACKEND_DIR="$PROJECT_ROOT/backend"
PACKAGE_TEMPLATE="$BACKEND_DIR/package"
WWW_DIR="$OUT_DIR/www"

if [ ! -f "$WWW_DIR/index.html" ]; then
    echo "❌ Error: $WWW_DIR not found, run 'make frontend' first (the .deb ships the console)."
    exit 1
fi

mkdir -p "$OUT_DIR"
cd "$BACKEND_DIR"

echo "🦀 Building Rust backend (Release)..."
cargo build --release

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

# Assemble the deb package in a temporary directory
BUILD_ROOT=$(mktemp -d)
trap 'rm -rf "$BUILD_ROOT"' EXIT
PKG_DIR="$BUILD_ROOT/aerolink"

mkdir -p "$PKG_DIR"
cp -r "$PACKAGE_TEMPLATE"/. "$PKG_DIR/"
mkdir -p "$PKG_DIR/usr/bin" "$PKG_DIR/usr/share/aerolink" "$PKG_DIR/usr/share/doc/aerolink"

cp target/release/aerolink "$PKG_DIR/usr/bin/aerolink"
chmod 755 "$PKG_DIR/usr/bin/aerolink"
cp -r "$WWW_DIR" "$PKG_DIR/usr/share/aerolink/www"
cp "$PROJECT_ROOT/docs/08-backend.md" "$PROJECT_ROOT/docs/09-deployment.md" "$PKG_DIR/usr/share/doc/aerolink/"
sed -i "s/^Version: .*/Version: $VERSION/" "$PKG_DIR/DEBIAN/control"
echo "Installed-Size: $(du -sk --exclude=DEBIAN "$PKG_DIR" | cut -f1)" >> "$PKG_DIR/DEBIAN/control"

# Ownership/modes: everything root-owned and world-readable, except the secrets file (fixed in postinst).
find "$PKG_DIR" -type d -exec chmod 755 {} +
find "$PKG_DIR" -type f ! -path "*/DEBIAN/*" ! -path "*/usr/bin/*" -exec chmod 644 {} +
chmod 755 "$PKG_DIR"/DEBIAN/postinst "$PKG_DIR"/DEBIAN/prerm "$PKG_DIR"/DEBIAN/postrm
chmod 644 "$PKG_DIR"/DEBIAN/control "$PKG_DIR"/DEBIAN/conffiles

echo "📦 Packaging into .deb..."
rm -f "$OUT_DIR/aerolink.deb"
dpkg-deb --root-owner-group -Zxz --build "$PKG_DIR" "$OUT_DIR/aerolink.deb"

echo "✅ Backend deb package successfully placed to build/aerolink.deb"
