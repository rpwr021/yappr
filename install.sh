#!/usr/bin/env bash
# Yappr one-line installer.
#
#   curl -fsSL https://raw.githubusercontent.com/rpwr021/yappr/main/install.sh | bash
#
# Clones (or updates) Yappr, installs Python deps, builds the app, and launches
# it. The engine (llama.cpp) and the model are fetched by the app on first run.
set -euo pipefail

REPO="${YAPPR_REPO:-https://github.com/rpwr021/yappr.git}"
DEST="${YAPPR_DEST:-$HOME/.yappr/app}"

say() { printf "\033[1;36m[yappr]\033[0m %s\n" "$*"; }
die() { printf "\033[1;31m[yappr] %s\033[0m\n" "$*" >&2; exit 1; }

# --- preflight ---
[ "$(uname -s)" = "Darwin" ] || die "Yappr is macOS only."
[ "$(uname -m)" = "arm64" ] || say "warning: built for Apple Silicon; Intel is untested."
command -v git >/dev/null || die "git is required (install Xcode command line tools: xcode-select --install)."

# uv (Python toolchain) — install if missing, the same way uv itself recommends.
if ! command -v uv >/dev/null 2>&1; then
  say "installing uv (Python toolchain)…"
  curl -fsSL https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi
command -v uv >/dev/null || die "uv install failed; see https://docs.astral.sh/uv/"

# --- fetch / update source ---
if [ -d "$DEST/.git" ]; then
  say "updating existing checkout in $DEST…"
  git -C "$DEST" pull --ff-only
else
  say "cloning Yappr into $DEST…"
  mkdir -p "$(dirname "$DEST")"
  git clone --depth 1 "$REPO" "$DEST"
fi
cd "$DEST"

# --- build + launch ---
say "installing dependencies…"
uv sync
say "building Yappr.app…"
./scripts/build_app.sh
say "launching…"
./scripts/run.sh

cat <<'EOF'

[yappr] Done. A 🎙️ icon is in your menu bar.
  • First launch downloads the engine + model (~5.8 GB) — watch the menu status.
  • Grant Yappr under System Settings > Privacy & Security:
      Input Monitoring (hotkeys) and Accessibility (typing).
  • Hold Right Option to dictate; Cmd+Right Option to chat.
EOF
