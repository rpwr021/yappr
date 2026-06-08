#!/usr/bin/env bash
# Install the llama.cpp engine Yappr needs, the way tools like opencode/ollama do:
# prefer Homebrew if present, else download the official prebuilt macOS binary
# into ~/.yappr/bin. No sudo, no compiling, no dylib surgery.
#
#   curl -fsSL <url>/install.sh | bash      # or run locally: ./install.sh
set -euo pipefail

YAPPR_HOME="$HOME/.yappr"
YAPPR_BIN="$YAPPR_HOME/bin"
# Pinned to a release verified to handle Gemma audio without crashing.
LLAMA_TAG="${YAPPR_LLAMA_TAG:-b9544}"

log() { echo "[yappr-install] $*"; }

# already available?
if command -v llama-server >/dev/null 2>&1; then
  log "llama-server already on PATH: $(command -v llama-server)"; exit 0
fi
if [ -x "$YAPPR_BIN/llama-server" ]; then
  log "llama-server already installed at $YAPPR_BIN"; exit 0
fi

arch="$(uname -m)"  # arm64 or x86_64
case "$arch" in
  arm64)  asset="llama-${LLAMA_TAG}-bin-macos-arm64.tar.gz" ;;
  x86_64) asset="llama-${LLAMA_TAG}-bin-macos-x64.tar.gz" ;;
  *) log "unsupported arch: $arch"; exit 1 ;;
esac

# Path 1: Homebrew (handles deps + PATH cleanly).
if command -v brew >/dev/null 2>&1; then
  log "installing via Homebrew..."
  if brew install llama.cpp; then
    log "done (brew). llama-server: $(command -v llama-server)"; exit 0
  fi
  log "brew install failed; falling back to prebuilt download"
fi

# Path 2: prebuilt release -> ~/.yappr/bin (no brew, no sudo).
log "downloading prebuilt $asset..."
mkdir -p "$YAPPR_BIN"
tmp="$(mktemp -d)"
url="https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_TAG}/${asset}"
curl -fsSL -o "$tmp/llama.tar.gz" "$url"
tar xzf "$tmp/llama.tar.gz" -C "$tmp"
# the tarball extracts to llama-<tag>/ containing binary + dylibs (self-contained)
src="$(find "$tmp" -name llama-server -type f | head -1)"
[ -z "$src" ] && { log "llama-server not in archive"; exit 1; }
srcdir="$(dirname "$src")"
# copy binary + all sibling dylibs together (they resolve via @rpath/@loader_path)
cp "$srcdir"/llama-server "$srcdir"/*.dylib "$YAPPR_BIN"/ 2>/dev/null || cp "$srcdir"/* "$YAPPR_BIN"/
chmod +x "$YAPPR_BIN/llama-server"
rm -rf "$tmp"
log "done. installed to $YAPPR_BIN/llama-server"
