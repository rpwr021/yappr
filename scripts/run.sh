#!/usr/bin/env bash
# Launch Yappr cleanly: stop any running instance, then start the bundle via
# `open` (so it persists under launchd, not tied to this shell).
#   ./run.sh           launch current build
#   ./run.sh --build   rebuild first, then launch
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

if [ "${1:-}" = "--build" ]; then
  ./scripts/build_app.sh
fi

# stop any prior app + its managed engine
pkill -f "MacOS/Yappr" 2>/dev/null || true
pkill -f "yappr.py" 2>/dev/null || true
pkill -f "engine/bin/llama-server" 2>/dev/null || true
sleep 1
rm -f "$HOME/.yappr.lock"

open dist/Yappr.app
echo "Yappr launched. Logs: tail -f /tmp/yappr*.log"
echo "(menu shows Starting… then Ready once the engine is up)"
