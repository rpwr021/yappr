#!/usr/bin/env bash
# Manually run the llama-server engine for Yappr (dev/debug).
# Normally the app starts the engine itself ([server] manage = true); use this
# only to run/inspect the server by hand.
set -euo pipefail
PORT=8089

if curl -sf "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
  echo "A llama-server is already running on port ${PORT}."
  echo "To replace it:  lsof -ti:${PORT} | xargs kill"
  exit 0
fi

# Resolve the engine binary (installed by engine_install.sh, or on PATH).
BIN="$HOME/.yappr/bin/llama-server"
[ -x "$BIN" ] || BIN="$(command -v llama-server || true)"
[ -n "$BIN" ] || { echo "llama-server not found; run engine/install.sh first"; exit 1; }

# Resolve the model from the HF cache.
SNAP="$(find "$HOME/.cache/huggingface/hub/models--google--gemma-4-E4B-it-qat-q4_0-gguf/snapshots" \
  -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1)"
[ -n "$SNAP" ] || { echo "model not in cache; launch the app once to download it"; exit 1; }

exec "$BIN" \
  -m "$SNAP/gemma-4-E4B_q4_0-it.gguf" \
  --mmproj "$SNAP/gemma-4-E4B-it-mmproj.gguf" \
  -ngl 99 -fa on -c 8192 --jinja --port "$PORT"
