#!/usr/bin/env bash
set -euo pipefail

EXPECTED_GIT_CLIFF="git-cliff 2.13.1"
ACTUAL_GIT_CLIFF=$(git cliff --version)
if [[ "$ACTUAL_GIT_CLIFF" != "$EXPECTED_GIT_CLIFF" ]]; then
  printf 'expected %s, found %s\n' "$EXPECTED_GIT_CLIFF" "$ACTUAL_GIT_CLIFF" >&2
  exit 1
fi

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/.." && pwd)
TEMP_ROOT=${TMPDIR:-/tmp}
TEMP_ROOT=${TEMP_ROOT%/}
POLICY_TMP=$(mktemp -d "$TEMP_ROOT/cargo-upkeep-release-policy.XXXXXX")

cleanup() {
  case "$POLICY_TMP" in
    "$TEMP_ROOT"/cargo-upkeep-release-policy.*)
      if [[ -d "$POLICY_TMP" ]]; then
        rm -rf -- "$POLICY_TMP"
      fi
      ;;
    *)
      printf 'refusing to remove unexpected test path: %s\n' "$POLICY_TMP" >&2
      ;;
  esac
}
trap cleanup EXIT

assert_bump() {
  local case_name=$1
  local starting_tag=$2
  local subject=$3
  local body=$4
  local expected=$5
  local case_repo="$POLICY_TMP/$case_name"
  local actual

  git init --quiet "$case_repo"
  git -C "$case_repo" config user.name "Release Policy Test"
  git -C "$case_repo" config user.email "release-policy@example.invalid"
  git -C "$case_repo" commit --quiet --allow-empty -m "chore: establish baseline"
  git -C "$case_repo" tag "$starting_tag"
  if [[ -n "$body" ]]; then
    git -C "$case_repo" commit --quiet --allow-empty -m "$subject" -m "$body"
  else
    git -C "$case_repo" commit --quiet --allow-empty -m "$subject"
  fi

  actual=$(git cliff \
    --config "$REPO_ROOT/cliff.toml" \
    --repository "$case_repo" \
    --bumped-version \
    2>/dev/null | tail -n 1)
  if [[ "$actual" != "$expected" ]]; then
    printf '%s: expected %s, found %s\n' "$case_name" "$expected" "$actual" >&2
    exit 1
  fi
  printf '%s -> %s\n' "$case_name" "$actual"
}

assert_bump "pre1-feat" "v0.4.0" "feat: add a maintenance capability" "" "v0.4.1"
assert_bump "pre1-breaking-feat" "v0.4.0" "feat!: replace a public interface" "" "v0.5.0"
assert_bump \
  "pre1-breaking-footer" \
  "v0.4.0" \
  "feat: replace a public interface" \
  "BREAKING CHANGE: callers must use the new interface" \
  "v0.5.0"
assert_bump "pre1-fix" "v0.4.0" "fix: correct an audit result" "" "v0.4.1"
assert_bump "pre1-ci" "v0.4.0" "ci: tune the build matrix" "" "v0.4.0"
assert_bump "post1-feat" "v1.2.3" "feat: add a maintenance capability" "" "v1.3.0"
