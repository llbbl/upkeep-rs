#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TEST_TMP=$(mktemp -d)
trap 'rm -rf "$TEST_TMP"' EXIT

MOCK_BIN="$TEST_TMP/mock-bin"
FIXTURE_DIR="$TEST_TMP/fixture"
mkdir -p "$MOCK_BIN" "$FIXTURE_DIR"

case "$(uname -m)" in
  x86_64|amd64) FIXTURE_ARCH="x86_64" ;;
  aarch64|arm64) FIXTURE_ARCH="aarch64" ;;
  *) printf 'unsupported test architecture\n' >&2; exit 1 ;;
esac

case "$(uname -s)" in
  Linux) FIXTURE_OS="unknown-linux-gnu" ;;
  Darwin) FIXTURE_OS="apple-darwin" ;;
  *) printf 'unsupported test operating system\n' >&2; exit 1 ;;
esac

FIXTURE_ARCHIVE="$FIXTURE_DIR/cargo-upkeep-${FIXTURE_ARCH}-${FIXTURE_OS}.tar.gz"
FIXTURE_CHECKSUM="$FIXTURE_ARCHIVE.sha256"

mkdir -p "$FIXTURE_DIR/archive"
cat > "$FIXTURE_DIR/archive/cargo-upkeep" <<'EOF'
#!/usr/bin/env bash
printf 'cargo-upkeep 9.9.9\n'
EOF
chmod +x "$FIXTURE_DIR/archive/cargo-upkeep"
tar -czf "$FIXTURE_ARCHIVE" -C "$FIXTURE_DIR/archive" cargo-upkeep

if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$FIXTURE_ARCHIVE" > "$FIXTURE_CHECKSUM"
else
  shasum -a 256 "$FIXTURE_ARCHIVE" > "$FIXTURE_CHECKSUM"
fi

cat > "$MOCK_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

output=""
url=""
previous=""
for argument in "$@"; do
  if [[ "$previous" == "-o" ]]; then
    output="$argument"
  fi
  if [[ "$argument" == http* ]]; then
    url="$argument"
  fi
  previous="$argument"
done

printf '%s\n' "$url" >> "$MOCK_CURL_LOG"

case "$url" in
  https://github.com/llbbl/upkeep-rs/releases/latest)
    printf 'https://github.com/llbbl/upkeep-rs/releases/tag/v9.9.9'
    ;;
  */cargo-upkeep-*.tar.gz)
    cp "$MOCK_ARCHIVE" "$output"
    ;;
  */cargo-upkeep-*.tar.gz.sha256)
    cp "$MOCK_CHECKSUM" "$output"
    ;;
  https://raw.githubusercontent.com/*/skills/*/SKILL.md)
    if [[ "$url" == *"/skills/${MOCK_FAIL_SKILL:-__none__}/SKILL.md" ]]; then
      exit 22
    fi
    printf '%s\n' "$url" > "$output"
    ;;
  *)
    printf 'unexpected curl URL: %s\n' "$url" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$MOCK_BIN/curl"

assert_contains() {
  local file="$1"
  local expected="$2"
  if ! grep -Fq "$expected" "$file"; then
    printf 'expected %s to contain: %s\n' "$file" "$expected" >&2
    return 1
  fi
}

assert_not_contains() {
  local file="$1"
  local unexpected="$2"
  if grep -Fq "$unexpected" "$file"; then
    printf 'expected %s not to contain: %s\n' "$file" "$unexpected" >&2
    return 1
  fi
}

run_installer() {
  local case_name="$1"
  local version="$2"
  local failed_skill="$3"
  local skip_skills="$4"
  local case_dir="$TEST_TMP/$case_name"

  mkdir -p "$case_dir/install"
  : > "$case_dir/curl.log"

  local status
  if env \
    PATH="$MOCK_BIN:$PATH" \
    VERSION="$version" \
    INSTALL_DIR="$case_dir/install" \
    SKILLS_DIR="$case_dir/skills" \
    SKIP_SKILLS="$skip_skills" \
    MOCK_ARCHIVE="$FIXTURE_ARCHIVE" \
    MOCK_CHECKSUM="$FIXTURE_CHECKSUM" \
    MOCK_CURL_LOG="$case_dir/curl.log" \
    MOCK_FAIL_SKILL="$failed_skill" \
    bash "$ROOT_DIR/scripts/install.sh" > "$case_dir/output.log" 2>&1; then
    status=0
  else
    status=$?
  fi

  printf '%s\n' "$status" > "$case_dir/status"
}

run_installer latest-success latest "" ""
[[ "$(<"$TEST_TMP/latest-success/status")" == "0" ]]
assert_contains "$TEST_TMP/latest-success/curl.log" "/releases/latest"
assert_contains "$TEST_TMP/latest-success/curl.log" "/releases/download/v9.9.9/cargo-upkeep-"
assert_contains "$TEST_TMP/latest-success/curl.log" "/v9.9.9/skills/upkeep-rs-deps/SKILL.md"
assert_not_contains "$TEST_TMP/latest-success/curl.log" "/main/skills/"

run_installer pinned-success v1.2.3 "" ""
[[ "$(<"$TEST_TMP/pinned-success/status")" == "0" ]]
assert_not_contains "$TEST_TMP/pinned-success/curl.log" "/releases/latest"
assert_contains "$TEST_TMP/pinned-success/curl.log" "/releases/download/v1.2.3/cargo-upkeep-"
assert_contains "$TEST_TMP/pinned-success/curl.log" "/v1.2.3/skills/upkeep-rs-quality/SKILL.md"

mkdir -p "$TEST_TMP/partial-failure/skills/upkeep-rs-quality"
printf 'existing skill content\n' > "$TEST_TMP/partial-failure/skills/upkeep-rs-quality/SKILL.md"
run_installer partial-failure v1.2.3 upkeep-rs-quality ""
[[ "$(<"$TEST_TMP/partial-failure/status")" != "0" ]]
assert_contains "$TEST_TMP/partial-failure/output.log" "Skills installed:"
assert_contains "$TEST_TMP/partial-failure/output.log" "  /upkeep-rs-deps"
assert_contains "$TEST_TMP/partial-failure/output.log" "Skills failed:"
assert_contains "$TEST_TMP/partial-failure/output.log" "  /upkeep-rs-quality"
[[ -f "$TEST_TMP/partial-failure/install/cargo-upkeep" ]]
assert_contains "$TEST_TMP/partial-failure/skills/upkeep-rs-quality/SKILL.md" "existing skill content"

run_installer skip-skills v1.2.3 "" 1
[[ "$(<"$TEST_TMP/skip-skills/status")" == "0" ]]
assert_not_contains "$TEST_TMP/skip-skills/curl.log" "raw.githubusercontent.com"
assert_contains "$TEST_TMP/skip-skills/output.log" "Skills skipped:"
assert_contains "$TEST_TMP/skip-skills/output.log" "  /upkeep-rs-quality"

printf 'installer tests passed\n'
