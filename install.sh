#!/usr/bin/env bash
set -euo pipefail

REPO="${YAPPR_REPO:-https://github.com/rpwr021/yappr.git}"
DEST="${YAPPR_DEST:-$HOME/.yappr/app}"
APP_DEST="/Applications/Yappr.app"
ZIP_URL="${YAPPR_ZIP_URL:-https://github.com/rpwr021/yappr/releases/latest/download/Yappr-macos.zip}"

say() { printf "\033[1;36m[yappr]\033[0m %s\n" "$*"; }
die() { printf "\033[1;31m[yappr] %s\033[0m\n" "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "Yappr is macOS only."

ARCH="$(uname -m)"
NO_PREBUILT=
case "$ARCH" in
  arm64) ;;          # Apple Silicon: prebuilt release is native.
  x86_64)
    # The prebuilt release is arm64-only, so Intel Macs must build from source,
    # which needs the Rust toolchain and git.
    say "Intel Mac detected; the prebuilt release is Apple Silicon only."
    say "Yappr will build from source instead (requires Rust and git)."
    NO_PREBUILT=1
    ;;
  *)
    die "unsupported architecture '$ARCH'. Yappr supports Apple Silicon (arm64) and Intel (x86_64) Macs."
    ;;
esac

# Report a missing build dependency with install instructions, then exit.
require_dep() {
  command -v "$1" >/dev/null && return 0
  printf "\033[1;31m[yappr] %s is required but not installed.\033[0m\n" "$2" >&2
  printf "\033[1;31m[yappr] install it with: %s\033[0m\n" "$3" >&2
  exit 1
}

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
  require_dep git "git" "xcode-select --install"
  require_dep cargo "Rust/Cargo" "curl https://sh.rustup.rs -sSf | sh   (or: brew install rust)"

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

if [ -z "$NO_PREBUILT" ] && install_zip; then
  say "installed release build"
else
  [ -n "$NO_PREBUILT" ] || say "release install unavailable; falling back to source build"
  install_source
fi

cat <<'EOF'

[yappr] Installed and launched.

Controls:
  - Hold Right Option: dictate into the active app
  - Hold Cmd + Right Option: ask a spoken question
  - Menu-bar icon: microphone, model, language, copy transcript, quit

Grant Yappr in System Settings > Privacy & Security:
  - Input Monitoring: global hotkey
  - Accessibility: paste dictated text
  - Microphone: record while the hotkey is held

Config: ~/.yappr/config.ini
Check:  /Applications/Yappr.app/Contents/MacOS/Yappr --check
Logs:   tail -f ~/.yappr/yappr.log
Disable logs: set [logging] enabled = false in ~/.yappr/config.ini, then relaunch.
EOF
