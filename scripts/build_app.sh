#!/usr/bin/env bash
# Build Yappr.app via py2app (alias mode) so Yappr gets its OWN macOS permission
# identity (its own signed binary), instead of inheriting Terminal's / uv's.
#
# Alias mode references this project + venv (fast, for personal use). For a
# distributable standalone app, drop the "-A".
set -euo pipefail

cd "$(dirname "$0")/.."   # repo root

# py2app + modern setuptools choke on pyproject's [project] table; hide it during build.
HID=0
if [ -f pyproject.toml ]; then mv pyproject.toml _pyproject.toml.bak; HID=1; fi
trap '[ "$HID" = 1 ] && mv _pyproject.toml.bak pyproject.toml' EXIT

rm -rf build dist
uv run python setup.py py2app -A

# Sign with the stable self-signed "Yappr Self-Signed" identity so the signature
# stays constant across rebuilds -> macOS keeps Input Monitoring / Accessibility
# grants instead of dropping them every build. Falls back to ad-hoc if missing.
CERT_HASH=$(security find-certificate -c "Yappr Self-Signed" -Z \
  ~/Library/Keychains/login.keychain-db 2>/dev/null | awk '/SHA-1/{print $3}')
if [ -n "${CERT_HASH:-}" ]; then
  codesign --force --deep --sign "$CERT_HASH" dist/Yappr.app
  echo "signed with stable identity (Yappr Self-Signed: $CERT_HASH)"
else
  codesign --force --deep --sign - dist/Yappr.app
  echo "WARN: 'Yappr Self-Signed' cert not found; used ad-hoc signing (grants will reset)."
fi

echo ""
echo "Built dist/Yappr.app"
echo "Launch:  open dist/Yappr.app"
echo "Logs:    dist/Yappr.app/Contents/MacOS/Yappr   (run in a terminal to see events)"
echo "First time only: grant Yappr in System Settings > Privacy & Security >"
echo "  Input Monitoring AND Accessibility. Stable signing keeps them after rebuilds."
