#!/usr/bin/env bash
set -euo pipefail

REPO="llbbl/cargo-upkeep"
BIN_NAME="cargo-upkeep"
VERSION="${VERSION:-latest}"

usage() {
  cat <<'EOF'
Install cargo-upkeep binary.

Usage:
  curl -fsSL https://raw.githubusercontent.com/llbbl/cargo-upkeep/main/scripts/install.sh | bash

Environment variables:
  VERSION      Release tag (default: latest)
  INSTALL_DIR  Install directory (default: prefers ~/.cargo/bin, then /usr/local/bin, then ~/.local/bin)

Examples:
  VERSION=v0.1.0 INSTALL_DIR="$HOME/.local/bin" bash install.sh
EOF
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

ensure_writable_dir() {
  local dir="$1"

  if [[ -d "$dir" ]]; then
    [[ -w "$dir" ]]
    return
  fi

  mkdir -p "$dir" 2>/dev/null && [[ -w "$dir" ]]
}

default_install_dir() {
  local cargo_bin="$HOME/.cargo/bin"
  local usr_local_bin="/usr/local/bin"
  local local_bin="$HOME/.local/bin"

  if ensure_writable_dir "$cargo_bin"; then
    printf '%s\n' "$cargo_bin"
    return
  fi

  if [[ -d "$usr_local_bin" && -w "$usr_local_bin" ]]; then
    printf '%s\n' "$usr_local_bin"
    return
  fi

  if ensure_writable_dir "$local_bin"; then
    printf '%s\n' "$local_bin"
    return
  fi

  fail "install dir is not writable: $local_bin"
}

detect_platform() {
  local os
  local arch
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)

  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) fail "unsupported architecture: $arch" ;;
  esac

  case "$os" in
    linux) os="unknown-linux-gnu" ;;
    darwin) os="apple-darwin" ;;
    *) fail "unsupported operating system: $os" ;;
  esac

  printf '%s-%s\n' "$arch" "$os"
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

platform=$(detect_platform)
if [[ -n "${INSTALL_DIR:-}" ]]; then
  install_dir="$INSTALL_DIR"
  if [[ ! -d "$install_dir" ]]; then
    mkdir -p "$install_dir" || fail "failed to create install dir: $install_dir"
  fi

  if [[ ! -w "$install_dir" ]]; then
    fail "install dir is not writable: $install_dir"
  fi
else
  install_dir=$(default_install_dir)
fi

archive="${BIN_NAME}-${platform}.tar.gz"

if [[ "$VERSION" == "latest" ]]; then
  url="https://github.com/${REPO}/releases/latest/download/${archive}"
else
  url="https://github.com/${REPO}/releases/download/${VERSION}/${archive}"
fi

tmp_dir=$(mktemp -d)
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

printf 'Downloading %s...\n' "$url"
curl -fsSL "$url" -o "$tmp_dir/$archive" || fail "download failed"
tar -xzf "$tmp_dir/$archive" -C "$tmp_dir" || fail "failed to extract archive"

if [[ ! -f "$tmp_dir/$BIN_NAME" ]]; then
  fail "extracted binary not found"
fi

chmod +x "$tmp_dir/$BIN_NAME" || fail "failed to mark binary executable"

mv "$tmp_dir/$BIN_NAME" "$install_dir/$BIN_NAME" || fail "failed to install binary"

printf 'Installed to %s/%s\n' "$install_dir" "$BIN_NAME"
"$install_dir/$BIN_NAME" --version || fail "installed binary failed to run"

printf 'Done. Ensure %s is on your PATH.\n' "$install_dir"
