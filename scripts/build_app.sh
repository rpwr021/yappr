#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release

APP="dist/Yappr.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp resources/Info.plist "$APP/Contents/Info.plist"
cp resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cp target/release/yappr "$APP/Contents/MacOS/Yappr"

SIGN_IDENTITY="${YAPPR_CODESIGN_IDENTITY:-}"
if [ -z "$SIGN_IDENTITY" ]; then
  SIGN_IDENTITY="$(security find-certificate -c "Yappr Self-Signed" -Z \
    ~/Library/Keychains/login.keychain-db 2>/dev/null | awk '/SHA-1/{print $3}' || true)"
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
