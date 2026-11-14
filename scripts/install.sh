#!/bin/sh
# Vixen installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/kleyt0n/vixen/main/scripts/install.sh | sh
#
# Installs the `vixen` CLI to ~/.local/bin and, on macOS, Vixen.app to /Applications
# (or ~/Applications when /Applications isn't writable).
#
# Environment:
#   VIXEN_VERSION      release tag to install (default: latest), e.g. v0.1.0
#   VIXEN_INSTALL_DIR  where the CLI goes (default: ~/.local/bin)
#   VIXEN_NO_APP=1     skip the macOS app
#   VIXEN_REPO         GitHub repo to download from (default: kleyt0n/vixen)
#   VIXEN_BASE_URL     download from a mirror (or file:// directory) instead of GitHub
set -eu

REPO="${VIXEN_REPO:-kleyt0n/vixen}"
VERSION="${VIXEN_VERSION:-latest}"
BIN_DIR="${VIXEN_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf 'vixen: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "needs '$1'"; }

need curl
need tar
need uname

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-musl ;;
  *) die "unsupported OS $(uname -s); on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) die "unsupported CPU $(uname -m)" ;;
esac
target="$arch-$os"

if [ -n "${VIXEN_BASE_URL:-}" ]; then
  base="$VIXEN_BASE_URL"
elif [ "$VERSION" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fetch() { # fetch <asset> → $tmp/<asset>, verified against SHA256SUMS when present
  curl -fsSL --retry 3 -o "$tmp/$1" "$base/$1" || die "download failed: $base/$1"
  if [ -f "$tmp/SHA256SUMS" ]; then
    want="$(awk -v f="$1" '$2 == f || $2 == "*"f {print $1}' "$tmp/SHA256SUMS")"
    [ -n "$want" ] || return 0
    if command -v sha256sum >/dev/null 2>&1; then got="$(sha256sum "$tmp/$1" | cut -d' ' -f1)"
    else got="$(shasum -a 256 "$tmp/$1" | cut -d' ' -f1)"; fi
    [ "$got" = "$want" ] || die "checksum mismatch for $1"
  fi
}

say "Installing vixen ($VERSION, $target)"
curl -fsSL --retry 3 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" 2>/dev/null || rm -f "$tmp/SHA256SUMS"

fetch "vixen-$target.tar.gz"
tar -xzf "$tmp/vixen-$target.tar.gz" -C "$tmp"
mkdir -p "$BIN_DIR"
install -m 755 "$tmp/vixen" "$BIN_DIR/vixen"
say "  CLI  → $BIN_DIR/vixen"

if [ "$os" = apple-darwin ] && [ "${VIXEN_NO_APP:-0}" != 1 ]; then
  fetch "Vixen-$arch.app.tar.gz"
  apps=/Applications
  [ -w "$apps" ] || { apps="$HOME/Applications"; mkdir -p "$apps"; }
  pkill -x vixen-app 2>/dev/null || true
  rm -rf "$apps/Vixen.app"
  tar -xzf "$tmp/Vixen-$arch.app.tar.gz" -C "$apps"
  xattr -dr com.apple.quarantine "$apps/Vixen.app" 2>/dev/null || true
  say "  App  → $apps/Vixen.app"
fi

"$BIN_DIR/vixen" init >/dev/null 2>&1 || true

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) printf '\nAdd %s to your PATH:\n  echo '\''export PATH="%s:$PATH"'\'' >> ~/.%src\n' \
       "$BIN_DIR" "$BIN_DIR" "$(basename "${SHELL:-sh}")" ;;
esac

say "Done. Edit ~/.config/vixen/config.toml (or Vixen → Settings), then run: vixen"
