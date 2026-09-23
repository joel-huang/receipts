#!/bin/sh
# Receipts installer.
#
#   curl -fsSL https://github.com/joel-huang/receipts/releases/latest/download/install.sh | sh
#
# Environment:
#   RECEIPTS_VERSION      tag to install, e.g. v0.1.0 (default: latest)
#   RECEIPTS_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
set -eu

REPO="${RECEIPTS_REPO:-joel-huang/receipts}"
BIN="receipts"
INSTALL_DIR="${RECEIPTS_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${RECEIPTS_VERSION:-latest}"

say() { printf '\033[1mreceipts:\033[0m %s\n' "$1"; }
err() { printf '\033[1;31mreceipts:\033[0m %s\n' "$1" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || err "need '$1' (command not found)"; }

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) err "unsupported architecture: $arch" ;;
  esac
  case "$os" in
    Darwin)
      # Rosetta shells report x86_64 on Apple Silicon; prefer the native build.
      if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
        arch=aarch64
      fi
      echo "$arch-apple-darwin" ;;
    Linux) echo "$arch-unknown-linux-musl" ;;
    MINGW* | MSYS* | CYGWIN*)
      err "on Windows, run in PowerShell: irm https://github.com/$REPO/releases/latest/download/install.ps1 | iex" ;;
    *) err "unsupported OS: $os" ;;
  esac
}

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    err "need curl or wget"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

main() {
  need uname
  need tar
  target=$(detect_target)
  if [ "$VERSION" = latest ]; then
    base="https://github.com/$REPO/releases/latest/download"
  else
    base="https://github.com/$REPO/releases/download/$VERSION"
  fi
  archive="$BIN-$target.tar.gz"

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  say "downloading $archive ($VERSION)"
  download "$base/$archive" "$tmp/$archive" || err "download failed: $base/$archive"
  download "$base/$archive.sha256" "$tmp/$archive.sha256" || err "checksum download failed"

  expected=$(cut -d' ' -f1 < "$tmp/$archive.sha256")
  actual=$(sha256 "$tmp/$archive")
  [ "$expected" = "$actual" ] || err "checksum mismatch (expected $expected, got $actual)"

  tar -xzf "$tmp/$archive" -C "$tmp"
  mkdir -p "$INSTALL_DIR"
  install -m 755 "$tmp/$BIN" "$INSTALL_DIR/$BIN" 2>/dev/null || {
    cp "$tmp/$BIN" "$INSTALL_DIR/$BIN" && chmod 755 "$INSTALL_DIR/$BIN"
  }

  say "installed $("$INSTALL_DIR/$BIN" --version) to $INSTALL_DIR/$BIN"

  case ":$PATH:" in
    *":$INSTALL_DIR:"*) say "run 'receipts' to open your chat history" ;;
    *)
      say "$INSTALL_DIR is not on your PATH. Add this to your shell profile:"
      printf '\n    export PATH="%s:$PATH"\n\n' "$INSTALL_DIR"
      ;;
  esac
}

main "$@"
