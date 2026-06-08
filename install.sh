#!/usr/bin/env bash
set -euo pipefail

REPO="${YAPPR_REPO:-https://github.com/rpwr021/yappr.git}"
DEST="${YAPPR_DEST:-$HOME/.yappr/app}"
APP_DEST="/Applications/Yappr.app"
ZIP_URL="${YAPPR_ZIP_URL:-https://github.com/rpwr021/yappr/releases/latest/download/Yappr-macos.zip}"

say() { printf "\033[1;36m[yappr]\033[0m %s\n" "$*"; }
die() { printf "\033[1;31m[yappr] %s\033[0m\n" "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "Yappr is macOS only."

install_zip() {
  command -v curl >/dev/null || return 1
  command -v ditto >/dev/null || return 1
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  say "downloading latest release"
  curl -fsSL "$ZIP_URL" -o "$tmp/Yappr.zip" || return 1
  ditto -x -k "$tmp/Yappr.zip" "$tmp"
  [ -d "$tmp/Yappr.app" ] || return 1
  say "installing $APP_DEST"
  pkill -f "Yappr.app/Contents/MacOS/Yappr" 2>/dev/null || true
  rm -rf "$APP_DEST"
  cp -R "$tmp/Yappr.app" "$APP_DEST"
  open "$APP_DEST"
}

install_source() {
  command -v git >/dev/null || die "git is required for source install."
  command -v cargo >/dev/null || die "Rust/Cargo is required for source install."

  if [ -d "$DEST/.git" ]; then
    say "updating $DEST"
    git -C "$DEST" pull --ff-only
  else
    say "cloning into $DEST"
    mkdir -p "$(dirname "$DEST")"
    git clone --depth 1 "$REPO" "$DEST"
  fi

  cd "$DEST"
  say "building"
  ./scripts/build_app.sh
  say "installing and launching"
  ./scripts/run.sh
}

if install_zip; then
  say "installed release build"
else
  say "release install unavailable; falling back to source build"
  install_source
fi

cat <<'EOF'

[yappr] Grant Yappr in System Settings > Privacy & Security:
  - Input Monitoring
  - Accessibility
  - Microphone

Hold Right Option to dictate. Hold Cmd+Right Option to chat.
Logs: tail -f ~/.yappr/yappr.log
EOF
