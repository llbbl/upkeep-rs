#!/usr/bin/env bash
set -euo pipefail

REPO="llbbl/upkeep-rs"
BIN_NAME="cargo-upkeep"
VERSION="${VERSION:-latest}"
SKILLS_DIR="${SKILLS_DIR:-$HOME/.claude/skills}"
SKILLS=("upkeep-rs-deps" "upkeep-rs-audit" "upkeep-rs-quality")

usage() {
  cat <<'EOF'
Install cargo-upkeep binary and Claude Code skills.

Usage:
  curl -fsSL https://raw.githubusercontent.com/llbbl/upkeep-rs/main/scripts/install.sh | bash

Environment variables:
  VERSION      Release tag for the binary and skills (default: latest)
  INSTALL_DIR  Install directory (default: prefers ~/.cargo/bin, then /usr/local/bin, then ~/.local/bin)
  SKILLS_DIR   Claude Code skills directory (default: ~/.claude/skills)
  SKIP_SKILLS  Set to 1 to skip skills installation

Examples:
  VERSION=v0.1.0 INSTALL_DIR="$HOME/.local/bin" bash install.sh
  SKIP_SKILLS=1 bash install.sh  # Binary only
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

resolve_release_ref() {
  local requested_version="$1"
  local latest_url
  local release_ref

  if [[ "$requested_version" != "latest" ]]; then
    printf '%s\n' "$requested_version"
    return
  fi

  latest_url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
    "https://github.com/${REPO}/releases/latest") || fail "failed to resolve latest release"
  latest_url="${latest_url%/}"

  if [[ "$latest_url" != "https://github.com/${REPO}/releases/tag/"* ]]; then
    fail "latest release resolved to an unexpected URL: $latest_url"
  fi

  release_ref="${latest_url##*/}"
  if [[ -z "$release_ref" || "$release_ref" == "latest" ]]; then
    fail "latest release did not resolve to a tag"
  fi

  printf '%s\n' "$release_ref"
}

print_skill_group() {
  local heading="$1"
  shift

  printf '%s:\n' "$heading"
  if [[ "$#" -eq 0 ]]; then
    printf '  (none)\n'
    return
  fi

  local skill
  for skill in "$@"; do
    printf '  /%s\n' "$skill"
  done
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

platform=$(detect_platform)
release_ref=$(resolve_release_ref "$VERSION")
if [[ "$VERSION" == "latest" ]]; then
  printf 'Resolved latest release to %s.\n' "$release_ref"
fi

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
url="https://github.com/${REPO}/releases/download/${release_ref}/${archive}"

tmp_dir=$(mktemp -d)
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

checksum_file="${archive}.sha256"
checksum_url="https://github.com/${REPO}/releases/download/${release_ref}/${checksum_file}"

printf 'Downloading %s...\n' "$url"
curl -fsSL "$url" -o "$tmp_dir/$archive" || fail "download failed"

printf 'Downloading checksum %s...\n' "$checksum_url"
curl -fsSL "$checksum_url" -o "$tmp_dir/$checksum_file" || fail "checksum download failed"

printf 'Verifying checksum...\n'
expected_checksum=$(awk '{print $1}' "$tmp_dir/$checksum_file")
if command -v sha256sum >/dev/null 2>&1; then
  actual_checksum=$(sha256sum "$tmp_dir/$archive" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  actual_checksum=$(shasum -a 256 "$tmp_dir/$archive" | awk '{print $1}')
else
  fail "no sha256sum or shasum found; cannot verify checksum"
fi

if [[ "$expected_checksum" != "$actual_checksum" ]]; then
  fail "checksum mismatch: expected $expected_checksum, got $actual_checksum"
fi
printf 'Checksum verified.\n'

tar -xzf "$tmp_dir/$archive" -C "$tmp_dir" || fail "failed to extract archive"

if [[ ! -f "$tmp_dir/$BIN_NAME" ]]; then
  fail "extracted binary not found"
fi

chmod +x "$tmp_dir/$BIN_NAME" || fail "failed to mark binary executable"

mv "$tmp_dir/$BIN_NAME" "$install_dir/$BIN_NAME" || fail "failed to install binary"

printf 'Installed to %s/%s\n' "$install_dir" "$BIN_NAME"
"$install_dir/$BIN_NAME" --version || fail "installed binary failed to run"

# Install Claude Code skills from the same release ref as the binary.
installed_skills=()
skipped_skills=()
failed_skills=()

if [[ "${SKIP_SKILLS:-}" != "1" ]]; then
  printf '\nInstalling Claude Code skills to %s...\n' "$SKILLS_DIR"

  skills_base_url="https://raw.githubusercontent.com/${REPO}/${release_ref}/skills"

  for skill in "${SKILLS[@]}"; do
    skill_dir="$SKILLS_DIR/$skill"
    skill_download="$tmp_dir/${skill}.SKILL.md"

    if ! mkdir -p "$skill_dir"; then
      printf '  Warning: failed to create directory for %s\n' "$skill"
      failed_skills+=("$skill")
      continue
    fi

    printf '  Installing %s...\n' "$skill"
    if ! curl -fsSL "$skills_base_url/$skill/SKILL.md" -o "$skill_download"; then
      printf '  Warning: failed to download %s\n' "$skill"
      failed_skills+=("$skill")
      continue
    fi

    if ! mv "$skill_download" "$skill_dir/SKILL.md"; then
      printf '  Warning: failed to install %s\n' "$skill"
      failed_skills+=("$skill")
      continue
    fi

    installed_skills+=("$skill")
  done
else
  skipped_skills=("${SKILLS[@]}")
fi

printf '\n'
print_skill_group "Skills installed" "${installed_skills[@]}"
print_skill_group "Skills skipped" "${skipped_skills[@]}"
print_skill_group "Skills failed" "${failed_skills[@]}"

if [[ "${#failed_skills[@]}" -gt 0 ]]; then
  fail "${#failed_skills[@]} skill installation(s) failed; the binary remains installed"
fi

printf '\nDone. Ensure %s is on your PATH.\n' "$install_dir"
