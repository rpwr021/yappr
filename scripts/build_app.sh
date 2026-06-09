#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release

APP="dist/Yappr.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp resources/Info.plist "$APP/Contents/Info.plist"
cp resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cp engine/install.sh "$APP/Contents/Resources/engine-install.sh"
cp target/release/yappr "$APP/Contents/MacOS/Yappr"

if [ -n "${YAPPR_VERSION:-}" ]; then
  plutil -replace CFBundleShortVersionString -string "$YAPPR_VERSION" "$APP/Contents/Info.plist"
fi
if [ -n "${YAPPR_BUILD:-}" ]; then
  plutil -replace CFBundleVersion -string "$YAPPR_BUILD" "$APP/Contents/Info.plist"
fi

SIGN_IDENTITY="${YAPPR_CODESIGN_IDENTITY:-}"
if [ -z "$SIGN_IDENTITY" ]; then
  SIGN_IDENTITY="$(security find-identity -v -p codesigning \
    ~/Library/Keychains/login.keychain-db 2>/dev/null | awk -F '"' '/Yappr Self-Signed/{print $2; exit}' || true)"
fi
if [ -z "$SIGN_IDENTITY" ]; then
  SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | awk -F '"' '/\\)/{print $2; exit}' || true)"
fi

if [ -n "${SIGN_IDENTITY:-}" ]; then
  codesign --force --options runtime --entitlements resources/Entitlements.plist --sign "$SIGN_IDENTITY" "$APP"
  echo "signed with identity: $SIGN_IDENTITY"
else
  codesign --force --entitlements resources/Entitlements.plist --sign - "$APP"
  echo "WARN: 'Yappr Self-Signed' cert not found; used ad-hoc signing."
fi

echo "Built $APP"
echo "Diagnostics: $APP/Contents/MacOS/Yappr --check"
echo "Not installed. Use ./scripts/run.sh --build to install /Applications/Yappr.app and launch it."
