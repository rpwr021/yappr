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
ditto dist/Yappr.app "$DEST"
# refresh LaunchServices so Spotlight resolves the new copy at this path
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
  -f "$DEST" 2>/dev/null || true

# Remove the staging copy. It is byte-identical to the installed one, but macOS
# tracks Accessibility and Input Monitoring per bundle PATH, so leaving it behind
# gives you two Yappr entries in System Settings and a second app Spotlight can
# launch with its own (unGranted) permissions. Keeping one copy is the whole
# point of installing to a canonical location.
rm -rf dist/Yappr.app
# Drop the stale LaunchServices registration for the path we just deleted.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
  -u "$PWD/dist/Yappr.app" 2>/dev/null || true

open "$DEST"
echo "Yappr installed to $DEST and launched."
echo "Staging copy removed, so /Applications/Yappr.app is the only bundle."
echo "Logs: tail -f ~/.yappr/yappr.log /tmp/yappr-llama-server.log"
echo "First time only: grant Yappr in System Settings > Privacy & Security >"
echo "  Input Monitoring + Accessibility. The grant sticks (stable signing)."
