#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${YAPPR_VERSION:-$(sed -nE 's/^version = "([^"]+)"/\1/p' Cargo.toml | head -1)}"
BUILD="${YAPPR_BUILD:-$VERSION}"
TAG="${YAPPR_RELEASE_TAG:-v$VERSION}"
ARCH="$(uname -m)"
OUT_DIR="dist/release"
ZIP="$OUT_DIR/Yappr-macos-$ARCH.zip"

YAPPR_VERSION="$VERSION" YAPPR_BUILD="$BUILD" ./scripts/build_app.sh

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"
COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent dist/Yappr.app "$ZIP"
cp "$ZIP" "$OUT_DIR/Yappr-macos.zip"
shasum -a 256 "$ZIP" "$OUT_DIR/Yappr-macos.zip" > "$OUT_DIR/SHA256SUMS"
SHA="$(shasum -a 256 "$OUT_DIR/Yappr-macos.zip" | awk '{print $1}')"
sed \
  -e "s/__VERSION__/$VERSION/g" \
  -e "s/__SHA256__/$SHA/g" \
  packaging/homebrew/yappr.rb.template > "$OUT_DIR/yappr.rb"

echo "Packaged Yappr $VERSION for $ARCH:"
echo "  $ZIP"
echo "  $OUT_DIR/Yappr-macos.zip"
echo "  $OUT_DIR/SHA256SUMS"
echo "  $OUT_DIR/yappr.rb"
