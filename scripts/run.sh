#!/usr/bin/env bash
# Install Yappr to /Applications and launch it from there, so Spotlight, Dock,
# and `open` all resolve to ONE canonical app with ONE stable permission grant.
#   ./run.sh           install current build to /Applications + launch
#   ./run.sh --build   rebuild first, then install + launch
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

DEST="/Applications/Yappr.app"

if [ "${1:-}" = "--build" ]; then
  ./scripts/build_app.sh
fi

[ -d dist/Yappr.app ] || { echo "dist/Yappr.app not found — run with --build first."; exit 1; }

# stop any prior app + its managed engine (any location)
pkill -f "Yappr.app/Contents/MacOS/Yappr" 2>/dev/null || true
pkill -f "engine/bin/llama-server" 2>/dev/null || true
sleep 1
rm -f "$HOME/.yappr.lock"

# install to the canonical location (replace in place to keep the same path)
rm -rf "$DEST"
cp -R dist/Yappr.app "$DEST"
# refresh LaunchServices so Spotlight resolves the new copy at this path
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
  -f "$DEST" 2>/dev/null || true

open "$DEST"
echo "Yappr installed to $DEST and launched."
echo "Logs: tail -f ~/.yappr/yappr.log /tmp/yappr-llama-server.log"
echo "First time only: grant Yappr in System Settings > Privacy & Security >"
echo "  Input Monitoring + Accessibility. The grant sticks (stable signing)."
