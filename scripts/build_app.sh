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
# Select by SHA-1 hash, not by name. Duplicate certificates can share the common
# name ("Yappr Self-Signed" imported twice, only one with a private key), and
# codesign then refuses with "ambiguous (matches ...)". find-identity -p
# codesigning only lists identities that have a usable private key, so the first
# hash it reports is the one that can actually sign.
if [ -z "$SIGN_IDENTITY" ]; then
  SIGN_IDENTITY="$(security find-identity -v -p codesigning \
    ~/Library/Keychains/login.keychain-db 2>/dev/null \
    | awk '/Yappr Self-Signed/{print $2; exit}' || true)"
fi
if [ -z "$SIGN_IDENTITY" ]; then
  SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
    | awk '/^ *[0-9]+\)/{print $2; exit}' || true)"
fi

if [ -n "${SIGN_IDENTITY:-}" ]; then
  codesign --force --options runtime --entitlements resources/Entitlements.plist --sign "$SIGN_IDENTITY" "$APP"
  echo "signed with stable identity: $SIGN_IDENTITY"
else
  codesign --force --entitlements resources/Entitlements.plist --sign - "$APP"
  echo "WARN: no stable code-signing identity found; used ad-hoc signing."
  echo "      Ad-hoc signatures change every rebuild, so macOS will KEEP"
  echo "      dropping the Microphone / Input Monitoring grants. Fix once with:"
  echo "          ./scripts/make_signing_identity.sh"
fi

echo "Built $APP (staging copy)"
echo "Diagnostics: $APP/Contents/MacOS/Yappr --check"
echo "Not installed. Use ./scripts/run.sh to install /Applications/Yappr.app and launch it;"
echo "that also removes this staging copy, so only one bundle holds permissions."
