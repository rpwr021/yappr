#!/usr/bin/env bash
# Build Yappr.app via py2app (standalone) so the app embeds its OWN signed Python
# interpreter. Alias mode (-A) symlinks to the venv python, whose ad-hoc identity
# does NOT match the Yappr signature — that breaks TCC (re-prompts every launch,
# silent mic/hotkey failures). Standalone makes the running process truly
# com.rpwr021.yappr, so permission grants stick.
set -euo pipefail

cd "$(dirname "$0")/.."   # repo root

# py2app + modern setuptools choke on pyproject's [project] table; hide it during build.
HID=0
if [ -f pyproject.toml ]; then mv pyproject.toml _pyproject.toml.bak; HID=1; fi
trap '[ "$HID" = 1 ] && mv _pyproject.toml.bak pyproject.toml' EXIT

rm -rf build dist
# NOTE: standalone (no -A) is abandoned — py2app can't bundle conda/standalone
# Python cleanly (libffi / zlib failures). Alias mode runs for personal use; the
# real distributable is the planned Rust rewrite (see docs/RUST_REWRITE_HANDOFF.md).
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
